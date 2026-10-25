//! MediaCodec decoder driven by two threads.
//!
//! The *worker* thread owns the codec lifecycle and is the input side: it receives frames from
//! the client core over a channel and queues each one as soon as it arrives. A per-codec
//! *output* thread blocks in `dequeue_output_buffer` with a timeout and releases every buffer
//! with `render = true` immediately. Only synchronous API-26 calls are used.

use crate::caps::choose_decoder;
use crate::color::codec_color;
use crate::status::{Phase, Shared};
use crate::tracker::Tracker;
use ezd_client_core::{ClientHandle, now_us};
use ezd_proto::{ClientCaps, Codec, StreamColor, StreamConfig, VideoFrame};
use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;
use ndk::native_window::NativeWindow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BUFFER_FLAG_KEY_FRAME: u32 = 1;
const BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
const OUTPUT_POLL: Duration = Duration::from_millis(10);
const INPUT_WAIT: Duration = Duration::from_millis(30);
const CREATE_RETRY: Duration = Duration::from_millis(500);
const KEYFRAME_RETRY: Duration = Duration::from_millis(100);
const MAX_CREATE_FAILURES: u32 = 3;

/// `ANativeWindow` is reference counted and usable from any thread.
pub struct SendWindow(pub NativeWindow);
// SAFETY: ANativeWindow is thread-safe; acquire/release are atomic.
unsafe impl Send for SendWindow {}

/// `AMediaCodec` supports input and output being driven from different threads.
struct SyncCodec(MediaCodec);
// SAFETY: see above; we only use one thread per direction and tear down after joining both.
unsafe impl Send for SyncCodec {}
unsafe impl Sync for SyncCodec {}

/// Commands for the worker thread.
pub enum Cmd {
    /// New stream negotiated: (re)create the decoder.
    Stream(StreamConfig, StreamColor),
    /// Parameter sets.
    Config(Vec<u8>),
    /// One encoded frame.
    Frame(VideoFrame),
    /// A new output surface, or none (destroyed). The sender is signalled when the codec no
    /// longer touches the previous surface.
    Surface(Option<SendWindow>, Option<Sender<()>>),
    /// Output thread hit an error (generation of the codec that failed).
    Error(u64),
    /// Tear everything down and exit.
    Shutdown(Option<Sender<()>>),
}

/// Handle to the worker thread.
pub struct DecoderCtl {
    tx: Sender<Cmd>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl DecoderCtl {
    /// Spawns the worker.
    pub fn spawn(
        caps: ClientCaps,
        handle: ClientHandle,
        shared: Arc<Shared>,
        window: Option<SendWindow>,
    ) -> Arc<DecoderCtl> {
        let (tx, rx) = channel();
        let mut w = Worker {
            caps,
            handle,
            shared,
            tx: tx.clone(),
            stream: None,
            color: StreamColor::default(),
            config: None,
            window,
            running: None,
            generation: 0,
            need_keyframe: true,
            last_kf_request: Instant::now() - Duration::from_secs(1),
            next_create: Instant::now(),
            create_failures: 0,
            tracker: Arc::new(Tracker::default()),
        };
        let join = std::thread::Builder::new()
            .name("ezd-decoder".into())
            .spawn(move || w.run(rx))
            .expect("spawn decoder thread");
        Arc::new(DecoderCtl {
            tx,
            join: Mutex::new(Some(join)),
        })
    }

    /// Queues a command; false if the worker is gone.
    pub fn send(&self, cmd: Cmd) -> bool {
        self.tx.send(cmd).is_ok()
    }

    /// Sends a command carrying an ack channel and waits for the ack.
    pub fn send_and_wait(&self, make: impl FnOnce(Sender<()>) -> Cmd, wait: Duration) -> bool {
        let (ack_tx, ack_rx) = channel();
        self.send(make(ack_tx)) && ack_rx.recv_timeout(wait).is_ok()
    }

    /// Shuts the worker down (idempotent) and waits up to `wait` for the codec to be released.
    pub fn shutdown(&self, wait: Duration) {
        self.send_and_wait(|a| Cmd::Shutdown(Some(a)), wait);
        let j = self.join.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(j) = j {
            let _ = j.join();
        }
    }
}

struct Running {
    codec: Arc<SyncCodec>,
    stop: Arc<AtomicBool>,
    out_thread: Option<JoinHandle<()>>,
    outputs: Arc<AtomicU64>,
    generation: u64,
}

struct Worker {
    caps: ClientCaps,
    handle: ClientHandle,
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    stream: Option<StreamConfig>,
    color: StreamColor,
    config: Option<Vec<u8>>,
    window: Option<SendWindow>,
    running: Option<Running>,
    generation: u64,
    need_keyframe: bool,
    last_kf_request: Instant,
    next_create: Instant,
    create_failures: u32,
    tracker: Arc<Tracker>,
}

fn mime(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "video/avc",
        Codec::Hevc => "video/hevc",
        Codec::Av1 => "video/av01",
    }
}

fn build_format(stream: &StreamConfig, color: Option<&StreamColor>, full: bool) -> MediaFormat {
    let mut f = MediaFormat::new();
    f.set_str("mime", mime(stream.codec));
    f.set_i32("width", stream.width as i32);
    f.set_i32("height", stream.height as i32);
    let max_input = (stream.width as i64 * stream.height as i64 * 3 / 2)
        .clamp(512 * 1024, 8 * 1024 * 1024) as i32;
    f.set_i32("max-input-size", max_input);
    // Colour keys (API 28+; older releases ignore unknown keys). The SurfaceView needs no colour mode:
    // the compositor switches the panel to HDR from the buffer dataspace the codec derives from these.
    if let Some(color) = color {
        let k = codec_color(stream.codec, color);
        f.set_i32("color-standard", k.standard);
        f.set_i32("color-transfer", k.transfer);
        f.set_i32("color-range", k.range);
        if let Some(p) = k.profile {
            f.set_i32("profile", p);
        }
        if let Some(info) = &k.hdr_static_info {
            f.set_buffer("hdr-static-info", info);
        }
    }
    if full {
        let fps = (stream.refresh_mhz / 1000).max(1) as i32;
        f.set_i32("low-latency", 1);
        f.set_i32("priority", 0);
        f.set_i32("operating-rate", fps);
        // Vendor low-latency switches; components that do not know a key ignore it.
        f.set_i32("vendor.qti-ext-dec-low-latency.enable", 1);
        f.set_i32("vendor.rtc-ext-dec-low-latency.enable", 1);
        f.set_i32("vendor.low-latency.enable", 1);
        f.set_i32(
            "vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req",
            1,
        );
        f.set_i32(
            "vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy",
            -1,
        );
        f.set_i32("vdec-lowlatency", 1);
    }
    f
}

fn create_codec(
    name: Option<&str>,
    stream: &StreamConfig,
    color: &StreamColor,
    window: &NativeWindow,
) -> Result<(MediaCodec, String), String> {
    let mut last_err = String::new();
    // The last attempt drops the colour keys too, in case a decoder rejects them; the bitstream
    // still carries the colour description.
    for (full, color) in [(true, Some(color)), (false, Some(color)), (false, None)] {
        let (codec, used) = match name.and_then(|n| MediaCodec::from_codec_name(n).map(|c| (c, n)))
        {
            Some(x) => x,
            None => (
                MediaCodec::from_decoder_type(mime(stream.codec))
                    .ok_or_else(|| format!("no decoder for {}", mime(stream.codec)))?,
                "default",
            ),
        };
        let fmt = build_format(stream, color, full);
        match codec.configure(&fmt, Some(window), MediaCodecDirection::Decoder) {
            Ok(()) => match codec.start() {
                Ok(()) => {
                    return Ok((codec, used.to_string()));
                }
                Err(e) => last_err = format!("start: {e}"),
            },
            Err(e) => last_err = format!("configure (full={full}, color={}): {e}", color.is_some()),
        }
        tracing::warn!("decoder setup failed: {last_err}");
    }
    Err(last_err)
}

enum QueueErr {
    Busy,
    TooBig,
    Fatal(String),
}

impl Worker {
    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            // While the decoder is missing, wake up to retry creation.
            let cmd = if self.running.is_none() && self.stream.is_some() && self.window.is_some() {
                match rx.recv_timeout(CREATE_RETRY) {
                    Ok(c) => Some(c),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                    Err(_) => break,
                }
            } else {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => break,
                }
            };
            match cmd {
                None => self.ensure_decoder(true),
                Some(Cmd::Stream(cfg, color)) => {
                    self.teardown();
                    self.stream = Some(cfg);
                    self.color = color;
                    self.config = None;
                    self.need_keyframe = true;
                    self.create_failures = 0;
                    self.ensure_decoder(false);
                }
                Some(Cmd::Config(data)) => {
                    if self.running.is_some()
                        && let Err(e) = self.queue(&data, 0, BUFFER_FLAG_CODEC_CONFIG)
                    {
                        self.queue_failed("codec config", e);
                    }
                    self.config = Some(data);
                }
                Some(Cmd::Frame(v)) => self.on_frame(v),
                Some(Cmd::Surface(win, ack)) => {
                    self.teardown();
                    self.window = win;
                    self.create_failures = 0;
                    if self.window.is_some() {
                        self.ensure_decoder(true);
                    }
                    if let Some(a) = ack {
                        let _ = a.send(());
                    }
                }
                Some(Cmd::Error(gen_)) => {
                    if self.running.as_ref().is_some_and(|r| r.generation == gen_) {
                        tracing::warn!("decoder error, recreating");
                        self.recover();
                    }
                }
                Some(Cmd::Shutdown(ack)) => {
                    self.teardown();
                    self.window = None;
                    if let Some(a) = ack {
                        let _ = a.send(());
                    }
                    break;
                }
            }
        }
        self.teardown();
    }

    fn request_keyframe(&mut self) {
        if self.last_kf_request.elapsed() >= KEYFRAME_RETRY {
            self.last_kf_request = Instant::now();
            self.handle.request_keyframe();
        }
    }

    fn drop_frame(&mut self) {
        self.handle.report_dropped(1);
    }

    fn on_frame(&mut self, v: VideoFrame) {
        if self.running.is_none() {
            // No surface (or decoder failed): we cannot decode; the next keyframe is needed later.
            self.drop_frame();
            self.need_keyframe = true;
            return;
        }
        if self.need_keyframe && !v.is_keyframe() {
            self.drop_frame();
            self.request_keyframe();
            return;
        }
        if v.is_keyframe() {
            self.need_keyframe = false;
        }
        let flags = if v.is_keyframe() {
            BUFFER_FLAG_KEY_FRAME
        } else {
            0
        };
        self.tracker.insert(v.capture_us, now_us());
        if let Err(e) = self.queue(&v.data, v.capture_us, flags) {
            self.tracker.take(v.capture_us);
            self.drop_frame();
            self.queue_failed("frame", e);
        }
    }

    fn queue_failed(&mut self, what: &str, e: QueueErr) {
        match e {
            QueueErr::Busy => {
                // Dropping a P-frame corrupts what follows; resync on a keyframe.
                tracing::warn!("decoder input busy, dropped {what}");
                self.need_keyframe = true;
                self.request_keyframe();
            }
            QueueErr::TooBig => {
                tracing::warn!("{what} larger than the decoder input buffer");
                self.need_keyframe = true;
                self.request_keyframe();
            }
            QueueErr::Fatal(m) => {
                tracing::warn!("queueing {what} failed: {m}");
                self.recover();
            }
        }
    }

    fn queue(&self, data: &[u8], pts: u64, flags: u32) -> Result<(), QueueErr> {
        let r = self.running.as_ref().ok_or(QueueErr::Busy)?;
        let codec = &r.codec.0;
        match codec.dequeue_input_buffer(INPUT_WAIT) {
            Ok(DequeuedInputBufferResult::Buffer(mut ib)) => {
                let buf = ib.buffer_mut();
                if buf.len() < data.len() {
                    // Hand the buffer back empty so it is not leaked.
                    let _ = codec.queue_input_buffer(ib, 0, 0, 0, 0);
                    return Err(QueueErr::TooBig);
                }
                // SAFETY: `buf` has at least `data.len()` bytes; u8 and MaybeUninit<u8> have the
                // same layout and the regions cannot overlap.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        buf.as_mut_ptr().cast::<u8>(),
                        data.len(),
                    );
                }
                codec
                    .queue_input_buffer(ib, 0, data.len(), pts, flags)
                    .map_err(|e| QueueErr::Fatal(e.to_string()))
            }
            Ok(DequeuedInputBufferResult::TryAgainLater) => Err(QueueErr::Busy),
            Err(e) => Err(QueueErr::Fatal(e.to_string())),
        }
    }

    /// Rebuilds the decoder after an error and asks the host for a keyframe.
    fn recover(&mut self) {
        let outputs = self
            .running
            .as_ref()
            .map_or(0, |r| r.outputs.load(Ordering::Relaxed));
        if outputs > 0 {
            self.create_failures = 0;
        } else {
            self.create_failures += 1;
        }
        self.teardown();
        self.need_keyframe = true;
        if self.create_failures >= MAX_CREATE_FAILURES {
            self.give_up("decoder keeps failing");
            return;
        }
        self.next_create = Instant::now();
        self.ensure_decoder(true);
    }

    fn give_up(&mut self, why: &str) {
        tracing::error!("giving up: {why}");
        self.shared.set_phase(Phase::Failed, why);
        self.handle.stop();
    }

    fn ensure_decoder(&mut self, recreated: bool) {
        if self.running.is_some() || Instant::now() < self.next_create {
            return;
        }
        let (Some(stream), Some(window)) = (self.stream.clone(), self.window.as_ref()) else {
            return;
        };
        let name = choose_decoder(&self.caps, stream.codec).map(|d| d.name.clone());
        match create_codec(name.as_deref(), &stream, &self.color, &window.0) {
            Ok((codec, actual)) => {
                tracing::info!(
                    "decoder {actual} started for {}x{}",
                    stream.width,
                    stream.height
                );
                self.shared.set_decoder(Some(actual));
                self.generation += 1;
                let codec = Arc::new(SyncCodec(codec));
                let stop = Arc::new(AtomicBool::new(false));
                let outputs = Arc::new(AtomicU64::new(0));
                self.tracker.clear();
                let out_thread = {
                    let (codec, stop, outputs) = (codec.clone(), stop.clone(), outputs.clone());
                    let (tracker, handle, tx, gen_) = (
                        self.tracker.clone(),
                        self.handle.clone(),
                        self.tx.clone(),
                        self.generation,
                    );
                    std::thread::Builder::new()
                        .name("ezd-decoder-out".into())
                        .spawn(move || {
                            output_loop(&codec, &stop, &outputs, &tracker, &handle, &tx, gen_)
                        })
                        .ok()
                };
                self.running = Some(Running {
                    codec,
                    stop,
                    out_thread,
                    outputs,
                    generation: self.generation,
                });
                self.need_keyframe = true;
                if let Some(cfg) = self.config.clone()
                    && let Err(e) = self.queue(&cfg, 0, BUFFER_FLAG_CODEC_CONFIG)
                {
                    tracing::warn!("re-queueing codec config failed");
                    self.queue_failed("codec config", e);
                }
                if recreated {
                    // The host will not resend parameter sets or an IDR on its own.
                    self.last_kf_request = Instant::now() - KEYFRAME_RETRY;
                    self.request_keyframe();
                }
            }
            Err(e) => {
                self.create_failures += 1;
                self.next_create = Instant::now() + CREATE_RETRY;
                tracing::error!("decoder creation failed ({}): {e}", self.create_failures);
                if self.create_failures >= MAX_CREATE_FAILURES {
                    self.give_up(&format!("decoder: {e}"));
                }
            }
        }
    }

    fn teardown(&mut self) {
        if let Some(mut r) = self.running.take() {
            r.stop.store(true, Ordering::Release);
            if let Some(j) = r.out_thread.take() {
                let _ = j.join();
            }
            if let Err(e) = r.codec.0.stop() {
                tracing::debug!("codec stop: {e}");
            }
            // `r.codec` drops here (deletes the AMediaCodec) once the output thread is gone.
            self.shared.set_decoder(None);
        }
        self.tracker.clear();
    }
}

fn output_loop(
    codec: &SyncCodec,
    stop: &AtomicBool,
    outputs: &AtomicU64,
    tracker: &Tracker,
    handle: &ClientHandle,
    tx: &Sender<Cmd>,
    generation: u64,
) {
    while !stop.load(Ordering::Acquire) {
        match codec.0.dequeue_output_buffer(OUTPUT_POLL) {
            Ok(DequeuedOutputBufferInfoResult::Buffer(buf)) => {
                let (pts, size) = (buf.info().presentation_time_us(), buf.info().size());
                let render = size > 0;
                let res = codec.0.release_output_buffer(buf, render);
                let done = now_us();
                if let Err(e) = res {
                    tracing::warn!("release_output_buffer: {e}");
                    let _ = tx.send(Cmd::Error(generation));
                    return;
                }
                if render {
                    outputs.fetch_add(1, Ordering::Relaxed);
                    if let Some(queued) = tracker.take(pts as u64) {
                        let dus = done.saturating_sub(queued).min(u32::MAX as u64) as u32;
                        handle.report_decoded(dus, pts as u64, done);
                    }
                }
            }
            Ok(DequeuedOutputBufferInfoResult::TryAgainLater) => {}
            Ok(DequeuedOutputBufferInfoResult::OutputFormatChanged) => {
                tracing::info!("decoder output format changed");
            }
            Ok(DequeuedOutputBufferInfoResult::OutputBuffersChanged) => {}
            Err(e) => {
                if !stop.load(Ordering::Acquire) {
                    tracing::warn!("dequeue_output_buffer: {e}");
                    let _ = tx.send(Cmd::Error(generation));
                }
                return;
            }
        }
    }
}
