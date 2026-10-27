//! System audio capture through PipeWire.
//!
//! `start` runs a PipeWire main loop on its own thread ("ezd-audio-pw") that
//!
//! 1. creates a null sink named `ezdisplay` (`object.linger=false`, so the daemon removes it as
//!    soon as our connection closes, also after a crash),
//! 2. makes it the configured default output through the `default` metadata object, remembering
//!    the previous value so `stop` can put it back (after a crash WirePlumber falls back by itself
//!    because the node vanishes), and
//! 3. captures the sink's monitor with a stream and hands the samples to `next_samples` through a
//!    bounded channel.
//!
//! The sink's volume is a control value only: the monitor is captured before the volume is applied
//! (`monitor.channel-volumes` is left off), so the stream always carries full-level audio. The
//! thread watches the sink's `Props` and reports `channelVolumes`/`mute` through
//! `take_volume_change`; `set_volume` writes them back (the PC then starts at the tablet's volume).
//!
//! Nothing touches PipeWire before `start`.

use ezd_host_core::{AudioBlock, AudioSource, SourceError, now_us};
use ezd_proto::AudioVolume;
use pipewire as pw;
use pw::properties::properties;
use pw::proxy::ProxyT;
use pw::spa::pod::Pod;
use pw::spa::sys as spa_sys;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Name of the null sink, also the `target.object` of the capture stream.
const SINK_NAME: &str = "ezdisplay";
const SAMPLE_RATE: u32 = 48_000;
/// Metadata key that holds the user's chosen default output.
const DEFAULT_SINK_KEY: &str = "default.configured.audio.sink";
const JSON_TYPE: &str = "Spa:String:JSON";
/// `node.latency` of the sink and the capture stream: 480 samples, 10 ms like our packets.
const NODE_LATENCY: &str = "480/48000";
/// Blocks queued between the PipeWire thread and `next_samples` before new ones are dropped.
const QUEUE_BLOCKS: usize = 50;
/// How long `start` waits for the sink and the stream to come up.
const START_TIMEOUT: Duration = Duration::from_secs(3);
/// How long `stop` waits for the PipeWire thread to restore the default and exit.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// If the daemon never answers the sync after a failure, the loop quits after this.
const STOP_SYNC_TIMEOUT: Duration = Duration::from_millis(1000);
/// How long `stop` waits for the playback streams to leave the sink after the default is restored.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the drain checks whether the streams have left.
const DRAIN_POLL: Duration = Duration::from_millis(50);

/// The capture source returned by [`audio_source`].
pub struct PipeWireAudio {
    running: Option<Running>,
}

struct Running {
    tx: pw::channel::Sender<Cmd>,
    rx: Receiver<AudioBlock>,
    thread: JoinHandle<()>,
    /// Signalled by the thread right before it ends.
    exited: Receiver<()>,
    error: Arc<Mutex<Option<String>>>,
    /// The latest volume change seen on the sink, until `take_volume_change` takes it.
    volume: Arc<Mutex<Option<AudioVolume>>>,
}

enum Cmd {
    Stop,
    SetVolume(AudioVolume),
}

/// Creates the (idle) source; PipeWire is only contacted by `start`.
pub fn audio_source() -> Option<Box<dyn AudioSource>> {
    Some(Box::new(PipeWireAudio { running: None }))
}

impl AudioSource for PipeWireAudio {
    fn start(&mut self, channels: u8) -> Result<(), SourceError> {
        self.stop();
        let channels = channels.clamp(1, 2);
        let (cmd_tx, cmd_rx) = pw::channel::channel::<Cmd>();
        let (block_tx, block_rx) = sync_channel::<AudioBlock>(QUEUE_BLOCKS);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel::<()>();
        let error = Arc::new(Mutex::new(None));
        let volume = Arc::new(Mutex::new(None));
        let thread = {
            let (error, volume) = (error.clone(), volume.clone());
            std::thread::Builder::new()
                .name("ezd-audio-pw".into())
                .spawn(move || {
                    if let Err(e) = run(channels, block_tx, cmd_rx, &ready_tx, &volume) {
                        tracing::warn!("audio capture failed: {e}");
                        let _ = ready_tx.send(Err(e.clone()));
                        *error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                    }
                    let _ = exit_tx.send(());
                })
                .map_err(|e| SourceError::msg(format!("cannot spawn audio thread: {e}")))?
        };
        self.running = Some(Running {
            tx: cmd_tx,
            rx: block_rx,
            thread,
            exited: exit_rx,
            error,
            volume,
        });
        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                self.stop();
                Err(SourceError::msg(e))
            }
            Err(_) => {
                self.stop();
                Err(SourceError::msg(
                    "timed out waiting for the PipeWire audio sink and stream",
                ))
            }
        }
    }

    fn next_samples(&mut self, timeout: Duration) -> Result<Option<AudioBlock>, SourceError> {
        let Some(r) = &self.running else {
            return Err(SourceError::msg("audio capture is not started"));
        };
        match r.rx.recv_timeout(timeout) {
            Ok(b) => Ok(Some(b)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                let e = r.error.lock().unwrap_or_else(|e| e.into_inner()).clone();
                Err(SourceError::msg(e.unwrap_or_else(|| {
                    "the PipeWire audio thread ended".into()
                })))
            }
        }
    }

    fn take_volume_change(&mut self) -> Option<AudioVolume> {
        let r = self.running.as_ref()?;
        r.volume.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn set_volume(&mut self, volume: AudioVolume) {
        if let Some(r) = &self.running {
            let _ = r.tx.send(Cmd::SetVolume(volume));
        }
    }

    fn stop(&mut self) {
        let Some(r) = self.running.take() else {
            return;
        };
        // The thread restores the default output before it drops the sink.
        let _ = r.tx.send(Cmd::Stop);
        match r.exited.recv_timeout(STOP_TIMEOUT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = r.thread.join();
            }
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!("the PipeWire audio thread did not stop in time; detaching it");
            }
        }
        // Drain so a stale block never reaches a later session.
        while !matches!(
            r.rx.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Disconnected)
        ) {}
    }
}

impl Drop for PipeWireAudio {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `audio.position` value for the null sink.
fn position_prop(channels: u8) -> &'static str {
    if channels == 1 { "MONO" } else { "FL,FR" }
}

/// SPA channel ids for the stream format, padded with zeros.
fn position_ids(channels: u8) -> [u32; spa_sys::SPA_AUDIO_MAX_CHANNELS as usize] {
    let mut p = [0; spa_sys::SPA_AUDIO_MAX_CHANNELS as usize];
    if channels == 1 {
        p[0] = spa_sys::SPA_AUDIO_CHANNEL_MONO;
    } else {
        p[0] = spa_sys::SPA_AUDIO_CHANNEL_FL;
        p[1] = spa_sys::SPA_AUDIO_CHANNEL_FR;
    }
    p
}

/// Linear (mixer) gain to the perceptual 0..=1 value a volume slider shows (the cube-root
/// convention of PulseAudio and pavucontrol). Boosts above 100 % clamp to 1.
fn linear_to_perceptual(linear: f32) -> f32 {
    if linear.is_nan() {
        return 0.0;
    }
    linear.clamp(0.0, 1.0).cbrt()
}

/// Inverse of [`linear_to_perceptual`].
fn perceptual_to_linear(perceptual: f32) -> f32 {
    if perceptual.is_nan() {
        return 0.0;
    }
    perceptual.clamp(0.0, 1.0).powi(3)
}

/// What a `Props` param said about the volume; a field is `None` when the param left it out.
#[derive(Debug, Default, PartialEq)]
struct PropsInfo {
    /// Loudest channel, linear.
    linear: Option<f32>,
    mute: Option<bool>,
}

/// Reads `channelVolumes` and `mute` out of a serialized `Props` object.
fn parse_props(bytes: &[u8]) -> Option<PropsInfo> {
    use pw::spa::pod::{Value, ValueArray, deserialize::PodDeserializer};
    let (_, Value::Object(obj)) = PodDeserializer::deserialize_any_from(bytes).ok()? else {
        return None;
    };
    let mut info = PropsInfo::default();
    for p in obj.properties {
        match (p.key, p.value) {
            (spa_sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(v))) => {
                info.linear = v.into_iter().reduce(f32::max);
            }
            (spa_sys::SPA_PROP_mute, Value::Bool(m)) => info.mute = Some(m),
            _ => {}
        }
    }
    Some(info)
}

/// A serialized `Props` object that sets the volume of every channel and the mute flag.
fn props_pod(volume: AudioVolume, channels: u8) -> Result<Vec<u8>, String> {
    use pw::spa::pod::{Object, Property, Value, ValueArray, serialize::PodSerializer};
    let linear = perceptual_to_linear(volume.volume);
    let obj = Value::Object(Object {
        type_: spa_sys::SPA_TYPE_OBJECT_Props,
        id: spa_sys::SPA_PARAM_Props,
        properties: vec![
            Property::new(
                spa_sys::SPA_PROP_channelVolumes,
                Value::ValueArray(ValueArray::Float(vec![linear; channels.max(1) as usize])),
            ),
            Property::new(spa_sys::SPA_PROP_mute, Value::Bool(volume.muted)),
        ],
    });
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &obj)
        .map(|(c, _)| c.into_inner())
        .map_err(|e| format!("pipewire volume pod: {e:?}"))
}

/// Turns the `Props` events of the sink into volume changes.
///
/// The first event is the state the sink had when we subscribed, not a change by the user, so it
/// is remembered without being reported (the tablet's volume must not be overwritten by it).
#[derive(Default)]
struct VolumeTracker {
    last: Option<AudioVolume>,
}

impl VolumeTracker {
    /// The new volume when `info` changes it.
    fn observe(&mut self, info: PropsInfo) -> Option<AudioVolume> {
        let known = self.last.unwrap_or(AudioVolume {
            volume: 1.0,
            muted: false,
        });
        let now = AudioVolume {
            volume: info.linear.map_or(known.volume, linear_to_perceptual),
            muted: info.mute.unwrap_or(known.muted),
        };
        let first = self.last.is_none();
        let changed = !known.is_same(&now);
        self.last = Some(now);
        (!first && changed).then_some(now)
    }
}

/// The `link.input.node` property of a Link global: the node the link feeds.
fn link_input_node(value: Option<&str>) -> Option<u32> {
    value?.trim().parse().ok()
}

/// Number of links that feed `sink` (playback streams into it). Links that start at the sink
/// (our own monitor capture) do not count.
fn links_into(links: &HashMap<u32, Option<u32>>, sink: u32) -> usize {
    links.values().filter(|i| **i == Some(sink)).count()
}

/// `{"name":"x"}` as stored in the `default.configured.audio.sink` metadata.
fn sink_json(name: &str) -> String {
    serde_json::json!({ "name": name }).to_string()
}

/// The sink name inside a `default.configured.*` metadata value.
fn parse_sink_name(value: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(value).ok()?;
    v.get("name")?.as_str().map(str::to_owned)
}

/// Capture time of the first sample of a block that has just been delivered: the process
/// callback runs when the block is complete, so the block started `samples / channels / rate`
/// earlier. Ignores the (small) graph latency after the capture.
fn block_pts_us(now: u64, samples: usize, channels: usize) -> u64 {
    let frames = (samples / channels.max(1)) as u64;
    now.saturating_sub(frames * 1_000_000 / SAMPLE_RATE as u64)
}

/// The `type`/`value` of the configured default sink as the daemon reports it.
type MetaValue = (Option<String>, String);

/// Tracks `default.configured.audio.sink` of subject 0 from the metadata events.
#[derive(Default)]
struct DefaultSink {
    current: Option<MetaValue>,
    /// Set once we replaced the default: the value to put back.
    saved: Option<Option<MetaValue>>,
}

/// Puts the configured default sink back, once. Returns true if it sent anything. Leaves the
/// value alone when something else (the user, another app) changed it after we did.
fn restore_default(meta: Option<&Meta>, ds: &mut DefaultSink) -> bool {
    let (Some(meta), Some(previous)) = (meta, ds.saved.take()) else {
        return false;
    };
    let ours = ds
        .current
        .as_ref()
        .is_some_and(|(_, v)| parse_sink_name(v).as_deref() == Some(SINK_NAME));
    if !ours {
        tracing::debug!("the default sink changed meanwhile; not restoring it");
        return false;
    }
    match previous {
        Some((t, v)) => meta.proxy.set_property(
            0,
            DEFAULT_SINK_KEY,
            t.as_deref().or(Some(JSON_TYPE)),
            Some(&v),
        ),
        None => meta.proxy.set_property(0, DEFAULT_SINK_KEY, None, None),
    }
    true
}

struct Meta {
    proxy: pw::metadata::Metadata,
    _listener: pw::metadata::MetadataListener,
}

fn run(
    channels: u8,
    blocks: SyncSender<AudioBlock>,
    cmds: pw::channel::Receiver<Cmd>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
    volume_out: &Arc<Mutex<Option<AudioVolume>>>,
) -> Result<(), String> {
    let e = |what: &str, err: pw::Error| format!("pipewire {what}: {err}");
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|x| e("main loop", x))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|x| e("context", x))?;
    let core = context.connect_rc(None).map_err(|x| e("connect", x))?;

    let failure: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    // Sync sequence the main loop is waiting for (see `roundtrip` and the stop handler).
    let wait_seq: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let _core_listener = {
        let (ml, failure, wait_seq) = (mainloop.clone(), failure.clone(), wait_seq.clone());
        core.add_listener_local()
            .done(move |id, seq| {
                if id == pw::core::PW_ID_CORE && wait_seq.get() == Some(seq.seq()) {
                    wait_seq.set(None);
                    ml.quit();
                }
            })
            .error({
                let ml = mainloop.clone();
                move |id, _seq, res, msg| {
                    // Errors on other objects (the node proxy) are reported by their own listeners.
                    if id == pw::core::PW_ID_CORE {
                        *failure.borrow_mut() = Some(format!("pipewire core error {res}: {msg}"));
                        ml.quit();
                    }
                }
            })
            .register()
    };
    let roundtrip = || -> Result<(), String> {
        let seq = core.sync(0).map_err(|x| e("sync", x))?;
        wait_seq.set(Some(seq.seq()));
        mainloop.run();
        match failure.borrow().clone() {
            Some(f) => Err(f),
            None => Ok(()),
        }
    };

    // 1. The null sink.
    let mut sink_props = properties! {
        "factory.name" => "support.null-audio-sink",
        "node.name" => SINK_NAME,
        "node.description" => SINK_NAME,
        "media.class" => "Audio/Sink",
        "object.linger" => "false",
        "audio.position" => position_prop(channels),
        // One quantum of our 10 ms packets; at the default 128 the capture node xruns under load.
        *pw::keys::NODE_LATENCY => NODE_LATENCY,
    };
    sink_props.insert("audio.rate", SAMPLE_RATE.to_string());
    let sink: Rc<pw::node::Node> = Rc::new(
        core.create_object("adapter", &sink_props)
            .map_err(|x| e("create sink", x))?,
    );
    let _sink_listener = {
        let (ml, failure) = (mainloop.clone(), failure.clone());
        sink.upcast_ref()
            .add_listener_local()
            .error(move |seq, res, msg| {
                *failure.borrow_mut() = Some(format!("pipewire sink error {res} ({seq}): {msg}"));
                ml.quit();
            })
            .register()
    };
    // The global id of our sink, from its own info: matching the registry by name could pick
    // the sink of a previous session that the daemon has not removed yet (ids are reused).
    let sink_global: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    // The volume: report every change of the sink's Props.
    let _sink_volume_listener = {
        let (out, tracker, global) = (
            volume_out.clone(),
            Rc::new(RefCell::new(VolumeTracker::default())),
            sink_global.clone(),
        );
        let l = sink
            .add_listener_local()
            .info(move |info| global.set(Some(info.id())))
            .param(move |_seq, id, _index, _next, param| {
                if id != pw::spa::param::ParamType::Props {
                    return;
                }
                let Some(info) = param.and_then(|p| parse_props(p.as_bytes())) else {
                    return;
                };
                if let Some(v) = tracker.borrow_mut().observe(info) {
                    *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
                }
            })
            .register();
        sink.subscribe_params(&[pw::spa::param::ParamType::Props]);
        l
    };
    roundtrip()?;

    // 2. The default output.
    let default_sink = Rc::new(RefCell::new(DefaultSink::default()));
    let registry = core.get_registry_rc().map_err(|x| e("registry", x))?;
    let meta_found: Rc<RefCell<Option<u32>>> = Rc::new(RefCell::new(None));
    // Links by id with the node they feed.
    let links: Rc<RefCell<HashMap<u32, Option<u32>>>> = Rc::new(RefCell::new(HashMap::new()));
    let _reg_listener = {
        let (meta_found, links, links_rm) = (meta_found.clone(), links.clone(), links.clone());
        registry
            .add_listener_local()
            .global(move |g| {
                if g.type_ == pw::types::ObjectType::Link {
                    let input = link_input_node(g.props.and_then(|p| p.get("link.input.node")));
                    links.borrow_mut().insert(g.id, input);
                }
                if g.type_ == pw::types::ObjectType::Metadata
                    && g.props
                        .is_some_and(|p| p.get("metadata.name") == Some("default"))
                {
                    meta_found.borrow_mut().get_or_insert(g.id);
                }
            })
            .global_remove(move |id| {
                links_rm.borrow_mut().remove(&id);
            })
            .register()
    };
    roundtrip()?;
    let meta = match *meta_found.borrow() {
        Some(id) => {
            let obj = pw::registry::GlobalObject {
                id,
                permissions: pw::permissions::PermissionFlags::all(),
                type_: pw::types::ObjectType::Metadata,
                version: 0,
                props: None::<&pw::spa::utils::dict::DictRef>,
            };
            let proxy: pw::metadata::Metadata =
                registry.bind(&obj).map_err(|x| e("bind metadata", x))?;
            let listener = {
                let ds = default_sink.clone();
                proxy
                    .add_listener_local()
                    .property(move |subject, key, type_, value| {
                        if subject == 0 && key.is_none_or(|k| k == DEFAULT_SINK_KEY) {
                            ds.borrow_mut().current =
                                value.map(|v| (type_.map(str::to_owned), v.to_owned()));
                        }
                        0
                    })
                    .register()
            };
            Some(Rc::new(Meta {
                proxy,
                _listener: listener,
            }))
        }
        None => {
            tracing::warn!("no default metadata object; the default output is left unchanged");
            None
        }
    };
    if let Some(m) = &meta {
        // The properties are replayed on bind; the roundtrip makes sure they arrived.
        roundtrip()?;
        let mut ds = default_sink.borrow_mut();
        let previous = ds.current.clone();
        tracing::debug!(?previous, "configured default sink before ezdisplay");
        m.proxy.set_property(
            0,
            DEFAULT_SINK_KEY,
            Some(JSON_TYPE),
            Some(&sink_json(SINK_NAME)),
        );
        ds.saved = Some(previous);
    }

    // 3. The monitor stream.
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        "target.object" => SINK_NAME,
        *pw::keys::STREAM_CAPTURE_SINK => "true",
        *pw::keys::NODE_LATENCY => NODE_LATENCY,
        *pw::keys::NODE_DONT_RECONNECT => "true",
    };
    let stream = pw::stream::StreamRc::new(core.clone(), "ezdisplay audio", props)
        .map_err(|x| e("stream", x))?;
    let ch = channels as usize;
    let stream_listener = {
        let (ml, failure, ready) = (mainloop.clone(), failure.clone(), ready.clone());
        stream
            .add_local_listener_with_user_data(blocks)
            .state_changed(move |_, _, old, new| {
                tracing::debug!("audio stream state {old:?} -> {new:?}");
                match new {
                    pw::stream::StreamState::Streaming | pw::stream::StreamState::Paused => {
                        let _ = ready.send(Ok(()));
                    }
                    pw::stream::StreamState::Error(msg) => {
                        *failure.borrow_mut() = Some(format!("pipewire audio stream error: {msg}"));
                        ml.quit();
                    }
                    pw::stream::StreamState::Unconnected
                        if matches!(
                            old,
                            pw::stream::StreamState::Streaming | pw::stream::StreamState::Paused
                        ) =>
                    {
                        // The sink went away under us.
                        *failure.borrow_mut() = Some("the audio stream was disconnected".into());
                        ml.quit();
                    }
                    _ => {}
                }
            })
            .process(move |stream, blocks| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(data) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
                let Some(bytes) = data.data() else {
                    return;
                };
                let Some(bytes) = bytes.get(offset..offset.saturating_add(size).min(bytes.len()))
                else {
                    return;
                };
                let samples: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                if samples.is_empty() {
                    return;
                }
                let pts_us = block_pts_us(now_us(), samples.len(), ch);
                // Full queue: the consumer is stalled, drop rather than block the graph.
                let _ = blocks.try_send(AudioBlock { pts_us, samples });
            })
            .register()
            .map_err(|x| e("stream listener", x))?
    };
    let mut info = pw::spa::param::audio::AudioInfoRaw::new();
    info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(channels as u32);
    info.set_position(position_ids(channels));
    let format = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: pw::spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )
    .map_err(|x| format!("pipewire audio format: {x:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&format).ok_or("invalid audio format pod")?];
    stream
        .connect(
            pw::spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|x| e("stream connect", x))?;

    // Stop: put the default output back, wait until the daemon has it, then leave.
    let _cmds = {
        let (ml, ds, wait_seq, core) = (
            mainloop.clone(),
            default_sink.clone(),
            wait_seq.clone(),
            core.clone(),
        );
        let meta = meta.clone();
        let sink = Rc::downgrade(&sink);
        cmds.attach(mainloop.loop_(), move |cmd| match cmd {
            Cmd::Stop => {
                restore_default(meta.as_deref(), &mut ds.borrow_mut());
                match core.sync(0) {
                    Ok(seq) => {
                        wait_seq.set(Some(seq.seq()));
                    }
                    Err(_) => ml.quit(),
                }
            }
            Cmd::SetVolume(v) => {
                let Some(sink) = sink.upgrade() else {
                    return;
                };
                match props_pod(v, channels) {
                    Ok(bytes) => match Pod::from_bytes(&bytes) {
                        Some(pod) => sink.set_param(pw::spa::param::ParamType::Props, 0, pod),
                        None => tracing::warn!("invalid volume pod"),
                    },
                    Err(e) => tracing::warn!("{e}"),
                }
            }
        })
    };
    mainloop.run();

    // Also on the error paths: never leave our sink as the default.
    if restore_default(meta.as_deref(), &mut default_sink.borrow_mut())
        && let Ok(seq) = core.sync(0)
    {
        wait_seq.set(Some(seq.seq()));
        let ml = mainloop.clone();
        let timer = mainloop.loop_().add_timer(move |_| ml.quit());
        let _ = timer.update_timer(Some(STOP_SYNC_TIMEOUT), None);
        mainloop.run();
    }
    // The daemon has the default back, but WirePlumber moves the playback streams off our sink
    // asynchronously; removing the sink first yanks them and stalls players. Wait for them.
    let sink_id = sink_global.get().unwrap_or_else(|| sink.upcast_ref().id());
    drain_links(&mainloop, &links, sink_id);
    drop(stream_listener);
    let _ = stream.disconnect();
    drop(sink);
    match failure.take() {
        Some(f) => Err(f),
        None => Ok(()),
    }
}

/// Runs the main loop until no playback link feeds `sink` any more, or [`DRAIN_TIMEOUT`].
fn drain_links(
    mainloop: &pw::main_loop::MainLoopRc,
    links: &Rc<RefCell<HashMap<u32, Option<u32>>>>,
    sink: u32,
) {
    let before = links_into(&links.borrow(), sink);
    if before == 0 {
        tracing::debug!("no playback stream on the audio sink ({sink})");
        return;
    }
    let start = std::time::Instant::now();
    let timed_out = Rc::new(Cell::new(false));
    let timer = {
        let (ml, links, timed_out) = (mainloop.clone(), links.clone(), timed_out.clone());
        mainloop.loop_().add_timer(move |_| {
            if links_into(&links.borrow(), sink) == 0 {
                ml.quit();
            } else if start.elapsed() >= DRAIN_TIMEOUT {
                timed_out.set(true);
                ml.quit();
            }
        })
    };
    let _ = timer.update_timer(Some(DRAIN_POLL), Some(DRAIN_POLL));
    mainloop.run();
    let left = links_into(&links.borrow(), sink);
    if timed_out.get() {
        tracing::warn!(
            "{left} playback stream(s) still linked to the audio sink after {:?}",
            start.elapsed()
        );
    } else {
        tracing::debug!(
            "{before} playback link(s) left the audio sink in {:?}",
            start.elapsed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn parses_sink_name_from_metadata_json() {
        assert_eq!(
            parse_sink_name(r#"{"name":"alsa_output.pci-0000_00_1f.3.analog-stereo"}"#).as_deref(),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo")
        );
        assert_eq!(
            parse_sink_name(r#"{ "name" : "x", "other": 1 }"#).as_deref(),
            Some("x")
        );
        assert_eq!(parse_sink_name("not json"), None);
        assert_eq!(parse_sink_name(r#"{"name":3}"#), None);
        assert_eq!(parse_sink_name("{}"), None);
    }

    #[test]
    fn sink_json_round_trips_through_the_parser() {
        let json = sink_json(SINK_NAME);
        assert_eq!(json, r#"{"name":"ezdisplay"}"#);
        assert_eq!(parse_sink_name(&json).as_deref(), Some(SINK_NAME));
        // Quotes in a name are escaped, not injected.
        assert_eq!(
            parse_sink_name(&sink_json(r#"a"b"#)).as_deref(),
            Some(r#"a"b"#)
        );
    }

    #[test]
    fn channel_positions() {
        assert_eq!(position_prop(1), "MONO");
        assert_eq!(position_prop(2), "FL,FR");
        let p = position_ids(2);
        assert_eq!(
            &p[..3],
            &[
                spa_sys::SPA_AUDIO_CHANNEL_FL,
                spa_sys::SPA_AUDIO_CHANNEL_FR,
                0
            ]
        );
        assert_eq!(position_ids(1)[0], spa_sys::SPA_AUDIO_CHANNEL_MONO);
    }

    #[test]
    fn volume_conversion_is_cubic_and_clamped() {
        assert_eq!(linear_to_perceptual(0.0), 0.0);
        assert_eq!(linear_to_perceptual(1.0), 1.0);
        assert!((linear_to_perceptual(0.125) - 0.5).abs() < 1e-6);
        assert_eq!(linear_to_perceptual(1.5), 1.0, "boost clamps");
        assert_eq!(linear_to_perceptual(-1.0), 0.0);
        assert_eq!(linear_to_perceptual(f32::NAN), 0.0);
        assert!((perceptual_to_linear(0.5) - 0.125).abs() < 1e-6);
        assert_eq!(perceptual_to_linear(2.0), 1.0);
        for i in 0..=100 {
            let p = i as f32 / 100.0;
            assert!((linear_to_perceptual(perceptual_to_linear(p)) - p).abs() < 1e-5);
        }
    }

    #[test]
    fn props_pod_round_trips_through_the_parser() {
        let v = AudioVolume {
            volume: 0.5,
            muted: true,
        };
        let bytes = props_pod(v, 2).unwrap();
        let info = parse_props(&bytes).unwrap();
        assert_eq!(info.mute, Some(true));
        assert!((info.linear.unwrap() - 0.125).abs() < 1e-6);
        assert!(Pod::from_bytes(&bytes).is_some());
        let mono = parse_props(&props_pod(v, 1).unwrap()).unwrap();
        assert_eq!(mono.linear, info.linear);
        assert_eq!(parse_props(b"junk"), None);
    }

    #[test]
    fn props_without_volume_fields_parse_to_none() {
        use pw::spa::pod::{Object, Property, Value, serialize::PodSerializer};
        let obj = Value::Object(Object {
            type_: spa_sys::SPA_TYPE_OBJECT_Props,
            id: spa_sys::SPA_PARAM_Props,
            properties: vec![Property::new(spa_sys::SPA_PROP_mute, Value::Bool(false))],
        });
        let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &obj)
            .unwrap()
            .0
            .into_inner();
        assert_eq!(
            parse_props(&bytes),
            Some(PropsInfo {
                linear: None,
                mute: Some(false)
            })
        );
    }

    #[test]
    fn tracker_skips_the_initial_state_and_reports_changes() {
        let info = |linear, mute| PropsInfo {
            linear: Some(linear),
            mute: Some(mute),
        };
        let mut t = VolumeTracker::default();
        // The state at subscription time is not a user change.
        assert_eq!(t.observe(info(1.0, false)), None);
        assert_eq!(t.observe(info(1.0, false)), None, "unchanged");
        let v = t.observe(info(0.125, false)).unwrap();
        assert!((v.volume - 0.5).abs() < 1e-6 && !v.muted);
        assert_eq!(t.observe(info(0.125, false)), None);
        // A mute toggle keeps the volume; a Props event without fields changes nothing.
        let m = t.observe(info(0.125, true)).unwrap();
        assert!((m.volume - 0.5).abs() < 1e-6 && m.muted);
        assert_eq!(t.observe(PropsInfo::default()), None);
        let u = t
            .observe(PropsInfo {
                linear: None,
                mute: Some(false),
            })
            .unwrap();
        assert!(!u.muted && (u.volume - 0.5).abs() < 1e-6);
    }

    #[test]
    fn link_input_node_parses_the_property() {
        assert_eq!(link_input_node(Some("42")), Some(42));
        assert_eq!(link_input_node(Some(" 7 ")), Some(7));
        assert_eq!(link_input_node(Some("x")), None);
        assert_eq!(link_input_node(None), None);
    }

    #[test]
    fn only_links_into_the_sink_count() {
        let mut links = HashMap::new();
        links.insert(1, Some(10)); // a playback stream into the sink
        links.insert(2, Some(10));
        links.insert(3, Some(20)); // our monitor capture feeds another node
        links.insert(4, None);
        assert_eq!(links_into(&links, 10), 2);
        links.remove(&1);
        links.remove(&2);
        assert_eq!(links_into(&links, 10), 0);
        assert_eq!(links_into(&links, 20), 1);
    }

    #[test]
    fn block_pts_subtracts_the_block_duration() {
        // 480 stereo frames are 10 ms.
        assert_eq!(block_pts_us(1_000_000, 960, 2), 990_000);
        assert_eq!(block_pts_us(1_000_000, 480, 1), 990_000);
        // Never underflows right after the clock starts.
        assert_eq!(block_pts_us(5, 960, 2), 0);
        assert_eq!(block_pts_us(100, 0, 0), 100);
    }

    #[test]
    fn restore_leaves_a_changed_default_alone() {
        let mut ds = DefaultSink {
            current: Some((None, r#"{"name":"other"}"#.into())),
            saved: Some(None),
        };
        // No metadata proxy and a foreign value: nothing to send either way.
        assert!(!restore_default(None, &mut ds));
    }

    /// The current `default.configured.audio.sink` value as `pw-metadata` prints it.
    fn default_sink_value() -> Option<String> {
        let out = Command::new("pw-metadata")
            .args(["-n", "default", "0", DEFAULT_SINK_KEY])
            .output()
            .expect("pw-metadata");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let rest = text.split("value:'").nth(1)?;
        Some(
            rest.rsplit_once("' type:")
                .map_or(rest, |(v, _)| v)
                .to_owned(),
        )
    }

    /// Puts the user's default output back even if the test panics.
    struct RestoreDefault(Option<String>);
    impl Drop for RestoreDefault {
        fn drop(&mut self) {
            let mut c = Command::new("pw-metadata");
            c.args(["-n", "default"]);
            match &self.0 {
                Some(v) => c.args(["0", DEFAULT_SINK_KEY, v, JSON_TYPE]),
                None => c.args(["-d", "0", DEFAULT_SINK_KEY]),
            };
            let _ = c.output();
        }
    }

    fn sink_node_exists() -> bool {
        let out = Command::new("pw-dump").output().expect("pw-dump");
        let dump: serde_json::Value = serde_json::from_slice(&out.stdout).expect("pw-dump json");
        dump.as_array().is_some_and(|objs| {
            objs.iter().any(|o| {
                o.pointer("/info/props/node.name").and_then(|v| v.as_str()) == Some(SINK_NAME)
            })
        })
    }

    /// Needs a running PipeWire + WirePlumber session:
    /// `EZD_PIPEWIRE_AUDIO_TEST=1 cargo test -p ezd-host-linux pipewire_audio`.
    #[test]
    fn pipewire_audio_capture_and_default_restore() {
        if std::env::var_os("EZD_PIPEWIRE_AUDIO_TEST").is_none_or(|v| v != "1") {
            eprintln!("skipped: set EZD_PIPEWIRE_AUDIO_TEST=1 (needs PipeWire and WirePlumber)");
            return;
        }
        let before = default_sink_value();
        let _guard = RestoreDefault(before.clone());
        assert!(!sink_node_exists(), "a stale ezdisplay sink exists");

        let mut src = audio_source().expect("audio source");
        src.start(2).expect("start");
        assert!(sink_node_exists(), "the ezdisplay sink was not created");
        let during = default_sink_value().expect("default sink while running");
        assert_eq!(parse_sink_name(&during).as_deref(), Some(SINK_NAME));

        let mut got = None;
        for _ in 0..20 {
            if let Some(b) = src
                .next_samples(Duration::from_millis(250))
                .expect("next_samples")
            {
                got = Some(b);
                break;
            }
        }
        let block = got.expect("no audio block arrived from the sink monitor");
        assert!(!block.samples.is_empty() && block.samples.len() % 2 == 0);
        assert!(block.pts_us <= now_us());

        src.stop();
        src.stop(); // idempotent
        assert_eq!(
            default_sink_value(),
            before,
            "the previous default was not restored"
        );
        assert!(!sink_node_exists(), "the ezdisplay sink outlived stop");
    }
}
