//! The host side of a connection: handshake, video pump, keepalive.
//!
//! ```text
//! client                         host
//!   |-- Hello ------------------->|     (both directions, any order)
//!   |<------------------- Hello --|
//!   |-- ClientCaps -------------->|
//!   |<--------------- HostCaps ---|
//!   |<------------- StreamConfig -|     negotiate(ClientCaps, HostCaps, Prefs)
//!   |-- ConfigAck (decoder ready)>|
//!   |<== CODEC_CONFIG, keyframe ==|     source.start() runs after the ack
//!   |<==== video / cursor ========|     Ping every 500 ms, RequestKeyframe on demand
//!   |-- Bye / EOF --------------->|
//! ```
//! A rejected handshake is answered with `Reject` and returned as an error. When the host shuts
//! down ([`SessionOptions::shutdown`]) it sends `Bye` and stops the source before returning.

use crate::input::{
    InputBackend, InputCmd, InputContext, InputStats, LiveLatency, PenMode, SharedOffset,
    input_thread,
};
use crate::source::{CursorSink, SourceError, VideoSource};
use crate::time::now_us;
use ezd_link::{Link, MuxClosed, MuxError, MuxOptions, MuxReceiver, MuxSender, VideoCredit};
use ezd_proto::{
    Bye, ClientCaps, ClientOptions, ClientStats, ClockSync, DEFAULT_CHUNK_LEN, Hello, HostCaps,
    InputStatus, Message, Ping, Pong, Prefs, ProtoError, Reject, Role, StreamConfig, VideoFrame,
    negotiate,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;

/// Tunables of a host session.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// Interval between host-initiated pings (default 500 ms).
    pub ping_interval: Duration,
    /// Mux fragment size (default 64 KiB).
    pub chunk_len: usize,
    /// Video credit policy (default: ack-based, window 1, 500 ms timeout).
    pub video_credit: VideoCredit,
    /// Maximum wait for each handshake step (default 10 s).
    pub handshake_timeout: Duration,
    /// How often the video thread re-checks for shutdown while idle (default 100 ms).
    pub poll_interval: Duration,
    /// Sent in `Hello.app_version`.
    pub app_version: String,
    /// Sent in `Hello.device_name`.
    pub device_name: String,
    /// Ends the session once it holds `true` (default: none). The client is sent `Bye` and the
    /// session returns after the source has stopped, so the process can exit safely: tearing a
    /// hardware encoder down while the process exits can deadlock in the driver (NVENC).
    pub shutdown: Option<watch::Receiver<bool>>,
    /// Creates the input sink of each session; `None` disables input (the client is told).
    pub input: Option<Arc<dyn InputBackend>>,
    /// How the pen is injected (passed to the input backend).
    pub pen_mode: PenMode,
    /// Ask the client to show its telemetry overlay (`ClientOptions::TELEMETRY`).
    pub telemetry: bool,
    /// Receives live [`SessionEvent`]s (default: none). Send errors are ignored.
    pub events: Option<tokio::sync::mpsc::UnboundedSender<SessionEvent>>,
}

/// How often [`SessionEvent::Stats`] is emitted.
const STATS_INTERVAL: Duration = Duration::from_secs(1);

/// Progress of a running session, see [`SessionOptions::events`].
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// The handshake finished; the stream is about to start.
    Configured {
        /// The client's `Hello`.
        client_hello: Hello,
        /// Negotiated stream configuration.
        stream: StreamConfig,
    },
    /// Measurements of the last window, once per second.
    Stats(LiveStats),
    /// The input backend reported its status (sent to the client as well).
    Input(InputStatus),
}

/// Measurements over the last [`window`](Self::window) of a session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveStats {
    /// Length of the window the counters below cover.
    pub window: Duration,
    /// Video frames sent in the window (excluding codec-config frames).
    pub frames: u64,
    /// Video payload bytes handed to the mux in the window.
    pub bytes: u64,
    /// Latest `ClientStats` received, if any.
    pub client: Option<ClientStats>,
    /// Median link transfer time of the frames acked in the window, microseconds.
    pub transfer_us_p50: Option<u32>,
    /// 95th percentile of the link transfer time in the window.
    pub transfer_us_p95: Option<u32>,
    /// Median client-send to host-injected input latency in the window (0.25 ms resolution).
    pub input_latency_p50_us: Option<u32>,
    /// 95th percentile of the input latency in the window.
    pub input_latency_p95_us: Option<u32>,
    /// Video frames whose ack timed out, since the session started.
    pub ack_timeouts: u64,
    /// Best RTT in microseconds.
    pub rtt_us: Option<i64>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            ping_interval: Duration::from_millis(500),
            chunk_len: DEFAULT_CHUNK_LEN,
            video_credit: VideoCredit::default(),
            handshake_timeout: Duration::from_secs(10),
            poll_interval: Duration::from_millis(100),
            app_version: env!("CARGO_PKG_VERSION").into(),
            device_name: std::env::var("HOSTNAME").unwrap_or_else(|_| "ezdisplay-host".into()),
            shutdown: None,
            input: None,
            pen_mode: PenMode::default(),
            telemetry: false,
            events: None,
        }
    }
}

/// Why a session that got past the handshake ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The client sent `Bye` (or `Reject`) with this reason.
    ClientBye(String),
    /// The client closed the connection without `Bye`.
    ClientEof,
    /// Transport or protocol error.
    LinkError(String),
    /// The video source failed; the client was sent `Bye`.
    SourceFailed(String),
    /// The video source stopped producing frames; the client was sent `Bye`.
    SourceStopped,
    /// [`SessionOptions::shutdown`] was set; the client was sent `Bye`.
    HostShutdown,
}

/// Result of a finished session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// The client's `Hello`.
    pub client_hello: Hello,
    /// Negotiated stream configuration.
    pub stream: StreamConfig,
    /// Why the session ended.
    pub end: EndReason,
    /// Wall time from handshake start to the end.
    pub duration: Duration,
    /// Video frames sent (excluding codec-config frames).
    pub frames_sent: u64,
    /// Of which keyframes.
    pub keyframes_sent: u64,
    /// Codec-config frames sent.
    pub codec_configs_sent: u64,
    /// Total video payload bytes handed to the mux.
    pub video_bytes: u64,
    /// `RequestKeyframe` messages received.
    pub keyframe_requests: u64,
    /// Last `ClientStats` received.
    pub last_client_stats: Option<ClientStats>,
    /// Median link transfer time (host encode done to client fully received),
    /// microseconds; needs clock sync and `FrameAck`s.
    pub transfer_us_p50: Option<u32>,
    /// 95th percentile of the link transfer time.
    pub transfer_us_p95: Option<u32>,
    /// Video frames whose ack timed out.
    pub ack_timeouts: u64,
    /// Estimated `client_clock - host_clock` in microseconds.
    pub clock_offset_us: Option<i64>,
    /// Best RTT in microseconds.
    pub rtt_us: Option<i64>,
    /// Input messages injected into the host.
    pub input_events: u64,
    /// Median client-send to host-injected latency (0.25 ms resolution); needs clock sync.
    pub input_latency_p50_us: Option<u32>,
    /// 95th percentile of the input latency.
    pub input_latency_p95_us: Option<u32>,
}

/// Handshake and setup failures.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Link error.
    #[error(transparent)]
    Mux(#[from] MuxError),
    /// IO error (e.g. spawning the video thread).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A handshake step timed out.
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    /// The peer sent something other than the expected handshake message.
    #[error("unexpected message during handshake (expected {expected}): {got}")]
    Unexpected {
        /// What we were waiting for.
        expected: &'static str,
        /// What arrived.
        got: String,
    },
    /// The connection closed during the handshake.
    #[error("connection closed during handshake")]
    Closed,
    /// The client refused or left during the handshake.
    #[error("client rejected the session: {0}")]
    Rejected(String),
    /// Incompatible protocol version or no usable stream configuration.
    #[error("negotiation failed: {0}")]
    Negotiation(#[from] ProtoError),
}

#[derive(Default)]
struct VideoShared {
    stop: AtomicBool,
    want_keyframe: AtomicBool,
    frames: AtomicU64,
    keyframes: AtomicU64,
    configs: AtomicU64,
    bytes: AtomicU64,
    /// `(frame_id, capture_us, encoded_us)` of sent frames awaiting their ack (bounded).
    sent: std::sync::Mutex<std::collections::VecDeque<(u64, u64, u64)>>,
}

struct StopOnDrop(Arc<VideoShared>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop.store(true, Ordering::Release);
    }
}

/// Runs one host session over `link` with default [`SessionOptions`].
///
/// See the module docs for the sequence. Returns when the client leaves, the
/// link fails, the source stops or the host shuts down. Dropping the future
/// aborts the session and tells the video thread to stop without waiting for it.
pub async fn run_host_session(
    link: Link,
    source: Box<dyn VideoSource>,
    prefs: Prefs,
) -> Result<SessionSummary, SessionError> {
    run_host_session_with(link, source, prefs, SessionOptions::default()).await
}

/// Like [`run_host_session`] with explicit options.
pub async fn run_host_session_with(
    link: Link,
    source: Box<dyn VideoSource>,
    prefs: Prefs,
    opts: SessionOptions,
) -> Result<SessionSummary, SessionError> {
    let peer = link.peer.clone();
    let started = Instant::now();
    let (tx, mut rx, task) = link.into_mux_with(MuxOptions {
        chunk_len: opts.chunk_len,
        credit: opts.video_credit,
    });
    tracing::info!(%peer, "session started");

    let host_caps = HostCaps {
        os: format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH),
        gpu: source.gpu_description(),
        encoders: source.encoders(),
    };
    let setup = handshake(&tx, &mut rx, &host_caps, &prefs, &opts).await;
    let (client_hello, client_caps, cfg) = match setup {
        Ok(v) => v,
        Err(e) => {
            tx.close();
            let _ = timeout(Duration::from_secs(2), task.join()).await;
            return Err(e);
        }
    };
    tracing::info!(?cfg, client = %client_hello.device_name, "stream configured");
    emit(
        &opts.events,
        SessionEvent::Configured {
            client_hello: client_hello.clone(),
            stream: cfg.clone(),
        },
    );

    // Video thread.
    let shared = Arc::new(VideoShared::default());
    let _guard = StopOnDrop(shared.clone());
    let (done_tx, mut done_rx) = oneshot::channel::<Result<(), SourceError>>();

    // Input thread.
    let supports_input = client_hello.supports_input();
    let offset = Arc::new(SharedOffset::default());
    let live_latency = Arc::new(LiveLatency::default());
    let (status_tx, status_rx) = tokio::sync::mpsc::unbounded_channel::<InputStatus>();
    let mut status_rx = Some(status_rx);
    let mut input_tx: Option<std::sync::mpsc::Sender<InputCmd>> = None;
    let mut input_join: Option<std::thread::JoinHandle<InputStats>> = None;
    if supports_input {
        let mut flags = 0;
        if opts.telemetry {
            flags |= ClientOptions::TELEMETRY;
        }
        tx.send_control(Message::ClientOptions(ClientOptions { flags }))?;
        match &opts.input {
            Some(backend) => {
                let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
                let ctx = InputContext {
                    backend: backend.clone(),
                    client: client_caps,
                    stream: cfg.clone(),
                    pen_mode: opts.pen_mode,
                    offset: offset.clone(),
                    live_latency: live_latency.clone(),
                    status_tx,
                };
                input_join = Some(
                    std::thread::Builder::new()
                        .name("ezd-input".into())
                        .spawn(move || input_thread(cmd_rx, ctx))?,
                );
                input_tx = Some(cmd_tx);
            }
            None => tx.send_control(Message::InputStatus(InputStatus {
                caps: 0,
                note: "input is disabled on the host".into(),
            }))?,
        }
    }
    let mut input_dropped_logged = false;

    {
        let (shared, tx, cfg) = (shared.clone(), tx.clone(), cfg.clone());
        let input_for_video = input_tx.clone();
        let handle = tokio::runtime::Handle::current();
        let poll = opts.poll_interval;
        std::thread::Builder::new()
            .name("ezd-video".into())
            .spawn(move || {
                let mut source = source;
                let cursor = CursorSink::new(tx.clone());
                let r = match source.start(&cfg, cursor) {
                    Ok(()) => {
                        if let Some(t) = &input_for_video {
                            let _ = t.send(InputCmd::Output(source.output()));
                        }
                        video_loop(source.as_mut(), &tx, &shared, &handle, poll)
                    }
                    Err(e) => Err(e),
                };
                source.stop();
                drop(source);
                let _ = done_tx.send(r);
            })?;
    }

    // Control loop.
    let mut clock = ClockSync::default();
    let mut ping_seq = 0u32;
    let mut keyframe_requests = 0u64;
    let mut last_stats = None;
    let mut transfer_all: Vec<u32> = Vec::new();
    let mut transfer_win: Vec<u32> = Vec::new();
    // Capture to encoded on the host, for the acked frames of the current window.
    let mut host_win: Vec<u32> = Vec::new();
    let mut video_done = false;
    let mut ping = tokio::time::interval(opts.ping_interval);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Live measurements: the window restarts at every `Stats` event.
    let mut stats_tick =
        tokio::time::interval_at(tokio::time::Instant::now() + STATS_INTERVAL, STATS_INTERVAL);
    stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut live_transfer: Vec<u32> = Vec::new();
    let (mut live_frames, mut live_bytes, mut live_since) = (0u64, 0u64, Instant::now());
    let mut shutdown = opts.shutdown;
    let end = loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(Some(Message::Pong(p))) => {
                    let t3 = now_us();
                    if let Some(s) = clock.add_sample(p.t0_us, p.t1_us, p.t2_us, t3) {
                        tracing::trace!(rtt_us = s.rtt_us, offset_us = s.offset_us, "pong");
                    }
                    if let Some(off) = clock.offset_us() {
                        offset.set(off);
                    }
                }
                Ok(Some(Message::Ping(p))) => {
                    let t1 = now_us();
                    let pong = Pong { seq: p.seq, t0_us: p.t0_us, t1_us: t1, t2_us: now_us() };
                    if tx.send_control(Message::Pong(pong)).is_err() {
                        break EndReason::LinkError("mux closed".into());
                    }
                }
                Ok(Some(Message::RequestKeyframe)) => {
                    keyframe_requests += 1;
                    shared.want_keyframe.store(true, Ordering::Release);
                }
                Ok(Some(Message::FrameAck(a))) => {
                    tx.ack_video(a.frame_id);
                    let encoded = {
                        let mut q = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
                        let mut found = None;
                        while let Some(&(id, cap, enc)) = q.front() {
                            if id > a.frame_id {
                                break;
                            }
                            q.pop_front();
                            if id == a.frame_id {
                                found = Some((cap, enc));
                            }
                        }
                        found
                    };
                    if let Some((cap, enc)) = encoded {
                        host_win.push(enc.saturating_sub(cap).min(u32::MAX as u64) as u32);
                    }
                    if let (Some((_, enc)), Some(off)) = (encoded, clock.offset_us()) {
                        // offset = client - host, so client time maps to host time as t - offset.
                        let t = (a.received_us as i64 - off) - enc as i64;
                        let t = t.max(0) as u32;
                        if transfer_all.len() < 100_000 {
                            transfer_all.push(t);
                        }
                        transfer_win.push(t);
                        if live_transfer.len() < 100_000 {
                            live_transfer.push(t);
                        }
                    }
                }
                Ok(Some(Message::ClientStats(s))) => {
                    tracing::info!(
                        transfer_p50_us = percentile(&transfer_win, 50),
                        transfer_p95_us = percentile(&transfer_win, 95),
                        acks = transfer_win.len(),
                        capture_encoded_p50_us = percentile(&host_win, 50),
                        capture_encoded_p95_us = percentile(&host_win, 95),
                        "link transfer"
                    );
                    transfer_win.clear();
                    host_win.clear();
                    tracing::info!(
                        recv = s.frames_received, decoded = s.frames_decoded, dropped = s.frames_dropped,
                        decode_p50_us = s.decode_us_p50, decode_p95_us = s.decode_us_p95,
                        e2e_p50_us = s.e2e_us_p50, e2e_p95_us = s.e2e_us_p95,
                        "client stats"
                    );
                    last_stats = Some(s);
                }
                Ok(Some(
                    m @ (Message::Pen(_)
                    | Message::Touch(_)
                    | Message::PointerButton(_)
                    | Message::Scroll(_)
                    | Message::Key(_)),
                )) => match &input_tx {
                    Some(t) => {
                        let _ = t.send(InputCmd::Msg(m));
                    }
                    None => {
                        if !input_dropped_logged {
                            input_dropped_logged = true;
                            tracing::debug!("dropping input: no input sink in this session");
                        }
                    }
                },
                Ok(Some(Message::Bye(b))) => break EndReason::ClientBye(b.reason),
                Ok(Some(Message::Reject(r))) => break EndReason::ClientBye(r.reason),
                Ok(Some(other)) => tracing::debug!(?other, "ignoring message"),
                Ok(None) => break EndReason::ClientEof,
                Err(e) => break EndReason::LinkError(e.to_string()),
            },
            _ = ping.tick() => {
                ping_seq = ping_seq.wrapping_add(1);
                let p = Ping { seq: ping_seq, t0_us: now_us() };
                if tx.send_control(Message::Ping(p)).is_err() {
                    break EndReason::LinkError("mux closed".into());
                }
            }
            _ = stats_tick.tick() => {
                let (frames, bytes) = (
                    shared.frames.load(Ordering::Relaxed),
                    shared.bytes.load(Ordering::Relaxed),
                );
                let (input_p50, input_p95) = live_latency.take();
                let stats = LiveStats {
                    window: live_since.elapsed(),
                    frames: frames - live_frames,
                    bytes: bytes - live_bytes,
                    client: last_stats,
                    transfer_us_p50: (!live_transfer.is_empty())
                        .then(|| percentile(&live_transfer, 50)),
                    transfer_us_p95: (!live_transfer.is_empty())
                        .then(|| percentile(&live_transfer, 95)),
                    input_latency_p50_us: input_p50,
                    input_latency_p95_us: input_p95,
                    ack_timeouts: tx.stats().ack_timeouts,
                    rtt_us: clock.rtt_us(),
                };
                (live_frames, live_bytes, live_since) = (frames, bytes, Instant::now());
                live_transfer.clear();
                emit(&opts.events, SessionEvent::Stats(stats));
            }
            Some(st) = next_status(&mut status_rx) => {
                emit(&opts.events, SessionEvent::Input(st.clone()));
                if tx.send_control(Message::InputStatus(st)).is_err() {
                    break EndReason::LinkError("mux closed".into());
                }
            }
            () = shutdown_requested(&mut shutdown) => break EndReason::HostShutdown,
            r = &mut done_rx => {
                video_done = true;
                break match r {
                    Ok(Ok(())) => EndReason::SourceStopped,
                    Ok(Err(e)) => EndReason::SourceFailed(e.to_string()),
                    Err(_) => EndReason::SourceFailed("video thread terminated unexpectedly".into()),
                };
            }
        }
    };
    tracing::info!(?end, "session ending");

    // Shutdown: stop the video thread, say goodbye if we are the one leaving.
    shared.stop.store(true, Ordering::Release);
    if let Some(t) = &input_tx {
        let _ = t.send(InputCmd::Stop);
    }
    match &end {
        EndReason::SourceFailed(r) => {
            let _ = tx.send_control(Message::Bye(Bye { reason: r.clone() }));
        }
        EndReason::SourceStopped => {
            let _ = tx.send_control(Message::Bye(Bye {
                reason: "video source stopped".into(),
            }));
        }
        EndReason::HostShutdown => {
            let _ = tx.send_control(Message::Bye(Bye {
                reason: "host shutting down".into(),
            }));
        }
        _ => {}
    }
    tx.close();
    if !video_done && timeout(Duration::from_secs(5), &mut done_rx).await.is_err() {
        tracing::warn!("video source did not stop in time");
    }
    // The input thread releases everything that is down before we return.
    let input_stats = match input_join {
        Some(j) => match timeout(
            Duration::from_secs(2),
            tokio::task::spawn_blocking(|| j.join()),
        )
        .await
        {
            Ok(Ok(Ok(stats))) => stats,
            Ok(_) => {
                tracing::warn!("input thread panicked");
                InputStats::default()
            }
            Err(_) => {
                tracing::warn!("input thread did not stop in time");
                InputStats::default()
            }
        },
        None => InputStats::default(),
    };
    let ack_timeouts = tx.stats().ack_timeouts;
    drop(tx);
    if timeout(Duration::from_secs(2), task.join()).await.is_err() {
        tracing::warn!("mux writer did not drain in time");
    }

    Ok(SessionSummary {
        client_hello,
        stream: cfg,
        end,
        duration: started.elapsed(),
        frames_sent: shared.frames.load(Ordering::Relaxed),
        keyframes_sent: shared.keyframes.load(Ordering::Relaxed),
        codec_configs_sent: shared.configs.load(Ordering::Relaxed),
        video_bytes: shared.bytes.load(Ordering::Relaxed),
        keyframe_requests,
        last_client_stats: last_stats,
        transfer_us_p50: (!transfer_all.is_empty()).then(|| percentile(&transfer_all, 50)),
        transfer_us_p95: (!transfer_all.is_empty()).then(|| percentile(&transfer_all, 95)),
        ack_timeouts,
        clock_offset_us: clock.offset_us(),
        rtt_us: clock.rtt_us(),
        input_events: input_stats.events,
        input_latency_p50_us: input_stats.latency_p50_us,
        input_latency_p95_us: input_stats.latency_p95_us,
    })
}

fn emit(events: &Option<tokio::sync::mpsc::UnboundedSender<SessionEvent>>, ev: SessionEvent) {
    if let Some(tx) = events {
        let _ = tx.send(ev);
    }
}

/// Next status from the input thread; never resolves without a receiver or once it is gone.
async fn next_status(
    rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<InputStatus>>,
) -> Option<InputStatus> {
    if let Some(r) = rx
        && let Some(st) = r.recv().await
    {
        return Some(st);
    }
    std::future::pending().await
}

/// Resolves once `rx` holds `true`; never without a receiver or after its sender is gone.
async fn shutdown_requested(rx: &mut Option<watch::Receiver<bool>>) {
    if let Some(rx) = rx
        && rx.wait_for(|v| *v).await.is_ok()
    {
        return;
    }
    std::future::pending().await
}

fn percentile(samples: &[u32], p: usize) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    let mut v = samples.to_vec();
    v.sort_unstable();
    v[(v.len() - 1) * p / 100]
}

/// Waits for a message selected by `pick`, answering pings meanwhile.
async fn expect<T>(
    rx: &mut MuxReceiver,
    tx: &MuxSender,
    what: &'static str,
    wait: Duration,
    mut pick: impl FnMut(Message) -> Result<T, Message>,
) -> Result<T, SessionError> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let msg = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => return Err(SessionError::Timeout(what)),
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(None)) => return Err(SessionError::Closed),
            Ok(Ok(Some(m))) => m,
        };
        match pick(msg) {
            Ok(v) => return Ok(v),
            Err(Message::Ping(p)) => {
                let t = now_us();
                let _ = tx.send_control(Message::Pong(Pong {
                    seq: p.seq,
                    t0_us: p.t0_us,
                    t1_us: t,
                    t2_us: t,
                }));
            }
            Err(Message::Pong(_)) => {}
            Err(Message::Reject(r)) => return Err(SessionError::Rejected(r.reason)),
            Err(Message::Bye(b)) => return Err(SessionError::Rejected(b.reason)),
            Err(other) => {
                return Err(SessionError::Unexpected {
                    expected: what,
                    got: format!("{other:?}"),
                });
            }
        }
    }
}

async fn handshake(
    tx: &MuxSender,
    rx: &mut MuxReceiver,
    host_caps: &HostCaps,
    prefs: &Prefs,
    opts: &SessionOptions,
) -> Result<(Hello, ClientCaps, StreamConfig), SessionError> {
    let wait = opts.handshake_timeout;
    tx.send_control(Message::Hello(Hello::new(
        Role::Host,
        &opts.app_version,
        &opts.device_name,
    )))?;
    let hello = expect(rx, tx, "Hello", wait, |m| match m {
        Message::Hello(h) => Ok(h),
        o => Err(o),
    })
    .await?;
    if let Err(e) = hello.check_compatible() {
        reject(tx, e.to_string());
        return Err(e.into());
    }
    let caps: ClientCaps = expect(rx, tx, "ClientCaps", wait, |m| match m {
        Message::ClientCaps(c) => Ok(c),
        o => Err(o),
    })
    .await?;
    tx.send_control(Message::HostCaps(host_caps.clone()))?;
    let cfg = match negotiate(&caps, host_caps, prefs) {
        Ok(c) => c,
        Err(e) => {
            reject(tx, e.to_string());
            return Err(e.into());
        }
    };
    tx.send_control(Message::StreamConfig(cfg.clone()))?;
    expect(rx, tx, "ConfigAck", wait, |m| match m {
        Message::ConfigAck => Ok(()),
        o => Err(o),
    })
    .await?;
    Ok((hello, caps, cfg))
}

fn reject(tx: &MuxSender, reason: String) {
    let _ = tx.send_control(Message::Reject(Reject { reason }));
}

fn video_loop(
    source: &mut dyn VideoSource,
    tx: &MuxSender,
    sh: &VideoShared,
    handle: &tokio::runtime::Handle,
    poll: Duration,
) -> Result<(), SourceError> {
    let mut last_config: Option<Vec<u8>> = None;
    let mut next_id = 0u64;
    // Diagnostics: `EZD_DIAG_DUMP=<file>` writes the stream as sent (codec config and frames).
    let mut dump =
        std::env::var_os("EZD_DIAG_DUMP").and_then(|p| match std::fs::File::create(&p) {
            Ok(f) => Some(f),
            Err(e) => {
                tracing::warn!("cannot create {}: {e}", p.to_string_lossy());
                None
            }
        });
    let mut write_dump = |data: &[u8]| {
        if let Some(f) = dump.as_mut()
            && let Err(e) = std::io::Write::write_all(f, data)
        {
            tracing::warn!("stream dump stopped: {e}");
            dump = None;
        }
    };
    loop {
        if sh.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        // Only pull content when the link can take a frame: this is the coalescing point.
        match handle.block_on(async { timeout(poll, tx.wait_video_slot()).await }) {
            Err(_) => continue,
            Ok(Err(MuxClosed)) => return Ok(()),
            Ok(Ok(())) => {}
        }
        if sh.want_keyframe.swap(false, Ordering::AcqRel) {
            source.request_keyframe();
        }
        let Some(f) = source.next_frame(poll)? else {
            continue;
        };
        let id = f.frame_id.unwrap_or(next_id);
        next_id = id + 1;
        if let Some(cfg) = f.codec_config
            && last_config.as_ref() != Some(&cfg)
        {
            let frame = VideoFrame {
                frame_id: id,
                flags: VideoFrame::CODEC_CONFIG,
                capture_us: f.capture_us,
                encoded_us: f.encoded_us,
                data: cfg.clone(),
            };
            write_dump(&cfg);
            if tx.send_video(frame).is_err() {
                return Ok(());
            }
            last_config = Some(cfg);
            sh.configs.fetch_add(1, Ordering::Relaxed);
        }
        sh.bytes.fetch_add(f.data.len() as u64, Ordering::Relaxed);
        {
            let mut q = sh.sent.lock().unwrap_or_else(|e| e.into_inner());
            if q.len() >= 256 {
                q.pop_front();
            }
            q.push_back((id, f.capture_us, f.encoded_us));
        }
        if f.keyframe {
            sh.keyframes.fetch_add(1, Ordering::Relaxed);
        }
        write_dump(&f.data);
        let frame = VideoFrame {
            frame_id: id,
            flags: if f.keyframe { VideoFrame::KEYFRAME } else { 0 },
            capture_us: f.capture_us,
            encoded_us: f.encoded_us,
            data: f.data,
        };
        if tx.send_video(frame).is_err() {
            return Ok(());
        }
        sh.frames.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::EncodedFrame;
    use crate::testing::ScriptedSource;
    use ezd_proto::{Codec, DecoderCaps, DisplayInfo};

    fn caps() -> ClientCaps {
        ClientCaps {
            display: DisplayInfo {
                width_px: 1280,
                height_px: 720,
                density_dpi: 160,
                refresh_rates_mhz: vec![60000],
                current_refresh_mhz: 60000,
            },
            decoders: vec![DecoderCaps {
                codec: Codec::H264,
                name: "d".into(),
                hardware: false,
                low_latency: true,
                max_width: 0,
                max_height: 0,
                max_fps: 0,
            }],
            stylus: false,
            max_touch_points: 0,
        }
    }

    fn fast_opts() -> SessionOptions {
        SessionOptions {
            ping_interval: Duration::from_millis(50),
            ..Default::default()
        }
    }

    async fn recv(rx: &mut MuxReceiver) -> Message {
        timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn full_session_against_scripted_source() {
        let (host_link, client_link) = Link::duplex_pair(1 << 20);
        let src = ScriptedSource::new(Duration::from_millis(5), 3000);
        let host = tokio::spawn(run_host_session_with(
            host_link,
            Box::new(src),
            Prefs::default(),
            fast_opts(),
        ));
        let (tx, mut rx, _t) = client_link.into_mux(DEFAULT_CHUNK_LEN);
        tx.send_control(Message::Hello(Hello::new(Role::Client, "t", "client")))
            .unwrap();
        tx.send_control(Message::ClientCaps(caps())).unwrap();
        assert!(matches!(recv(&mut rx).await, Message::Hello(h) if h.role == Role::Host));
        assert!(matches!(recv(&mut rx).await, Message::HostCaps(_)));
        let cfg = match recv(&mut rx).await {
            Message::StreamConfig(c) => c,
            m => panic!("{m:?}"),
        };
        assert_eq!(
            (
                cfg.width,
                cfg.height,
                cfg.refresh_mhz,
                cfg.encoder_name.as_str()
            ),
            (1280, 720, 60000, "scripted")
        );
        tx.send_control(Message::ConfigAck).unwrap();

        let (
            mut frames,
            mut saw_config_first,
            mut kf_after_request,
            mut cursor_shape,
            mut cursor_state,
        ) = (0u32, false, false, false, false);
        let (mut options, mut status) = (false, false);
        let mut requested = false;
        let mut last_id = None;
        while !kf_after_request {
            match recv(&mut rx).await {
                Message::VideoFrame(v) if v.is_codec_config() => {
                    assert_eq!(frames, 0, "config must precede the first keyframe");
                    saw_config_first = true;
                }
                Message::VideoFrame(v) => {
                    tx.send_control(Message::FrameAck(ezd_proto::FrameAck {
                        frame_id: v.frame_id,
                        received_us: now_us(),
                    }))
                    .unwrap();
                    if frames == 0 {
                        assert!(v.is_keyframe() && saw_config_first);
                    }
                    if let Some(l) = last_id {
                        assert!(v.frame_id > l);
                    }
                    last_id = Some(v.frame_id);
                    frames += 1;
                    if requested && v.is_keyframe() {
                        kf_after_request = true;
                    }
                    if frames == 10 && !requested {
                        requested = true;
                        tx.send_control(Message::RequestKeyframe).unwrap();
                    }
                    assert!(frames < 500, "keyframe never arrived");
                }
                Message::CursorShape(_) => cursor_shape = true,
                Message::CursorState(_) => cursor_state = true,
                Message::Ping(p) => {
                    let t = now_us();
                    tx.send_control(Message::Pong(Pong {
                        seq: p.seq,
                        t0_us: p.t0_us,
                        t1_us: t,
                        t2_us: t,
                    }))
                    .unwrap();
                }
                Message::ClientOptions(o) => {
                    assert_eq!(o.flags, 0);
                    options = true;
                }
                Message::InputStatus(st) => {
                    assert_eq!(st.caps, 0, "input is disabled by default");
                    status = true;
                }
                m => panic!("{m:?}"),
            }
        }
        assert!(cursor_shape && cursor_state && options && status);
        tx.send_control(Message::Bye(Bye {
            reason: "test over".into(),
        }))
        .unwrap();
        let s = timeout(Duration::from_secs(5), host)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(s.end, EndReason::ClientBye("test over".into()));
        assert_eq!(s.codec_configs_sent, 1);
        assert!(s.keyframes_sent >= 2);
        assert_eq!(s.keyframe_requests, 1);
        assert!(
            s.clock_offset_us.is_some(),
            "pong should have produced a clock sample"
        );
    }

    #[tokio::test]
    async fn version_mismatch_is_rejected() {
        let (host_link, client_link) = Link::duplex_pair(1 << 16);
        let src = ScriptedSource::new(Duration::from_millis(5), 10);
        let host = tokio::spawn(run_host_session_with(
            host_link,
            Box::new(src),
            Prefs::default(),
            fast_opts(),
        ));
        let (tx, mut rx, _t) = client_link.into_mux(1024);
        let mut h = Hello::new(Role::Client, "t", "c");
        h.proto_major += 1;
        tx.send_control(Message::Hello(h)).unwrap();
        assert!(matches!(recv(&mut rx).await, Message::Hello(_)));
        assert!(matches!(recv(&mut rx).await, Message::Reject(_)));
        assert!(matches!(
            host.await.unwrap(),
            Err(SessionError::Negotiation(
                ProtoError::VersionMismatch { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn no_common_codec_is_rejected() {
        let (host_link, client_link) = Link::duplex_pair(1 << 16);
        let src = ScriptedSource::new(Duration::from_millis(5), 10);
        let host = tokio::spawn(run_host_session_with(
            host_link,
            Box::new(src),
            Prefs::default(),
            fast_opts(),
        ));
        let (tx, mut rx, _t) = client_link.into_mux(1024);
        let mut c = caps();
        c.decoders[0].codec = Codec::Av1;
        tx.send_control(Message::Hello(Hello::new(Role::Client, "t", "c")))
            .unwrap();
        tx.send_control(Message::ClientCaps(c)).unwrap();
        assert!(matches!(recv(&mut rx).await, Message::Hello(_)));
        assert!(matches!(recv(&mut rx).await, Message::HostCaps(_)));
        assert!(matches!(recv(&mut rx).await, Message::Reject(_)));
        assert!(matches!(
            host.await.unwrap(),
            Err(SessionError::Negotiation(ProtoError::NoCommonCodec))
        ));
    }

    #[tokio::test]
    async fn handshake_timeout() {
        let (host_link, _client_link) = Link::duplex_pair(1 << 16);
        let src = ScriptedSource::new(Duration::from_millis(5), 10);
        let opts = SessionOptions {
            handshake_timeout: Duration::from_millis(100),
            ..fast_opts()
        };
        let r = run_host_session_with(host_link, Box::new(src), Prefs::default(), opts).await;
        assert!(matches!(r, Err(SessionError::Timeout(_))));
    }

    /// A source whose `stop` is slow, like a hardware encoder's teardown, and records that it ran.
    struct SlowStop(ScriptedSource, Arc<AtomicBool>);
    impl VideoSource for SlowStop {
        fn encoders(&self) -> Vec<ezd_proto::EncoderCaps> {
            self.0.encoders()
        }
        fn start(&mut self, cfg: &StreamConfig, cursor: CursorSink) -> Result<(), SourceError> {
            self.0.start(cfg, cursor)
        }
        fn next_frame(&mut self, t: Duration) -> Result<Option<EncodedFrame>, SourceError> {
            self.0.next_frame(t)
        }
        fn request_keyframe(&mut self) {
            self.0.request_keyframe()
        }
        fn stop(&mut self) {
            self.0.stop();
            std::thread::sleep(Duration::from_millis(300));
            self.1.store(true, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn shutdown_says_bye_and_waits_for_the_source() {
        let (host_link, client_link) = Link::duplex_pair(1 << 20);
        let stopped = Arc::new(AtomicBool::new(false));
        let src = SlowStop(
            ScriptedSource::new(Duration::from_millis(5), 100),
            stopped.clone(),
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let opts = SessionOptions {
            shutdown: Some(shutdown_rx),
            ..fast_opts()
        };
        let host = tokio::spawn(run_host_session_with(
            host_link,
            Box::new(src),
            Prefs::default(),
            opts,
        ));
        let (tx, mut rx, _t) = client_link.into_mux(1024);
        tx.send_control(Message::Hello(Hello::new(Role::Client, "t", "c")))
            .unwrap();
        tx.send_control(Message::ClientCaps(caps())).unwrap();
        for _ in 0..3 {
            recv(&mut rx).await;
        }
        tx.send_control(Message::ConfigAck).unwrap();
        loop {
            if matches!(recv(&mut rx).await, Message::VideoFrame(_)) {
                break;
            }
        }
        shutdown_tx.send(true).unwrap();
        let bye = loop {
            if let Message::Bye(b) = recv(&mut rx).await {
                break b;
            }
        };
        assert_eq!(bye.reason, "host shutting down");
        let s = timeout(Duration::from_secs(5), host)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(s.end, EndReason::HostShutdown);
        assert!(
            stopped.load(Ordering::Acquire),
            "the session returned before the source stopped"
        );
    }

    #[tokio::test]
    async fn client_eof_ends_session() {
        let (host_link, client_link) = Link::duplex_pair(1 << 20);
        let src = ScriptedSource::new(Duration::from_millis(5), 100);
        let host = tokio::spawn(run_host_session_with(
            host_link,
            Box::new(src),
            Prefs::default(),
            fast_opts(),
        ));
        let (tx, mut rx, _t) = client_link.into_mux(1024);
        tx.send_control(Message::Hello(Hello::new(Role::Client, "t", "c")))
            .unwrap();
        tx.send_control(Message::ClientCaps(caps())).unwrap();
        for _ in 0..3 {
            recv(&mut rx).await;
        }
        tx.send_control(Message::ConfigAck).unwrap();
        // Wait for video to flow, then vanish.
        loop {
            if matches!(recv(&mut rx).await, Message::VideoFrame(_)) {
                break;
            }
        }
        drop(tx);
        drop(rx);
        let s = timeout(Duration::from_secs(5), host)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(s.end, EndReason::ClientEof | EndReason::LinkError(_)),
            "{:?}",
            s.end
        );
    }

    #[tokio::test]
    async fn live_events_report_configuration_then_stats() {
        let (host_link, client_link) = Link::duplex_pair(1 << 20);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
        let host = tokio::spawn(run_host_session_with(
            host_link,
            Box::new(ScriptedSource::new(Duration::from_millis(5), 100)),
            Prefs::default(),
            SessionOptions {
                events: Some(ev_tx),
                ..fast_opts()
            },
        ));
        let (tx, mut rx, _t) = client_link.into_mux(DEFAULT_CHUNK_LEN);
        tx.send_control(Message::Hello(Hello::new(Role::Client, "t", "tablet")))
            .unwrap();
        tx.send_control(Message::ClientCaps(caps())).unwrap();
        for _ in 0..3 {
            recv(&mut rx).await;
        }
        tx.send_control(Message::ConfigAck).unwrap();
        // Keep the client side flowing: ack frames and answer pings.
        let acker = tx.clone();
        tokio::spawn(async move {
            while let Ok(Some(m)) = rx.recv().await {
                match m {
                    Message::VideoFrame(v) => {
                        let _ = acker.send_control(Message::FrameAck(ezd_proto::FrameAck {
                            frame_id: v.frame_id,
                            received_us: now_us(),
                        }));
                    }
                    Message::Ping(p) => {
                        let t = now_us();
                        let _ = acker.send_control(Message::Pong(Pong {
                            seq: p.seq,
                            t0_us: p.t0_us,
                            t1_us: t,
                            t2_us: t,
                        }));
                    }
                    _ => {}
                }
            }
        });
        let first = timeout(Duration::from_secs(10), ev_rx.recv())
            .await
            .unwrap()
            .unwrap();
        match first {
            SessionEvent::Configured {
                client_hello,
                stream,
            } => {
                assert_eq!(client_hello.device_name, "tablet");
                assert_eq!(stream.width, 1280);
            }
            e => panic!("{e:?}"),
        }
        let stats = loop {
            match timeout(Duration::from_secs(10), ev_rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                SessionEvent::Stats(s) => break s,
                SessionEvent::Input(_) => {}
                e => panic!("{e:?}"),
            }
        };
        assert!(stats.frames > 0 && stats.bytes > 0, "{stats:?}");
        assert!(stats.window >= Duration::from_millis(500), "{stats:?}");
        tx.send_control(Message::Bye(Bye {
            reason: "test over".into(),
        }))
        .unwrap();
        let s = timeout(Duration::from_secs(5), host)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(s.end, EndReason::ClientBye("test over".into()));
    }
}
