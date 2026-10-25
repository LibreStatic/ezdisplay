//! Handshake and receive loop.

use crate::{ClientHandle, now_us};
use ezd_link::{Link, MuxError, MuxReceiver, MuxSender};
use ezd_proto::{
    Bye, ClientCaps, ClientHdr, CursorShape, CursorState, FrameAck, Hello, Message, Ping, Pong,
    Role, StreamColor, StreamConfig, VideoFrame,
};
use std::time::Duration;
use tokio::time::{MissedTickBehavior, interval, timeout};

/// Error type returned by [`ClientHandler`] callbacks.
pub type HandlerError = Box<dyn std::error::Error + Send + Sync>;

/// Result of a [`ClientHandler`] callback.
pub type HandlerResult = Result<Flow, HandlerError>;

/// What the loop does after a handler callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep running.
    Continue,
    /// End the session cleanly (a `Bye` is sent): [`ClientEnd::Stopped`].
    Stop,
}

/// Receives the decoded-side events of the session.
///
/// Called on the receive loop: do not block. Hand work to another thread. An
/// `Err` aborts the session with [`ClientError::Handler`].
pub trait ClientHandler: Send {
    /// The stream was negotiated. Create the decoder here; `ConfigAck` is sent
    /// right after this returns `Ok`.
    fn on_stream_config(&mut self, cfg: &StreamConfig) -> HandlerResult;

    /// The colour description of the stream, called right before [`on_stream_config`]
    /// (BT.709 8-bit when the host sent none). Also readable with
    /// [`ClientHandle::stream_color`](crate::ClientHandle::stream_color).
    ///
    /// [`on_stream_config`]: ClientHandler::on_stream_config
    fn on_stream_color(&mut self, _color: &StreamColor) {}

    /// A `CODEC_CONFIG` frame (parameter sets). Never acked.
    fn on_codec_config(&mut self, frame: VideoFrame) -> HandlerResult;

    /// A fully reassembled non-config video frame. It was already acked.
    /// `received_us` is its [`now_us`] at reassembly.
    fn on_video_frame(&mut self, frame: VideoFrame, received_us: u64) -> HandlerResult;

    /// A new cursor shape (already cached in the handle).
    fn on_cursor_shape(&mut self, _shape: &CursorShape) {}

    /// A cursor state (already stored in the handle).
    fn on_cursor_state(&mut self, _state: &CursorState) {}
}

/// Settings of a client session.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `Hello.device_name`.
    pub device_name: String,
    /// `Hello.app_version`.
    pub app_version: String,
    /// Capabilities sent in `ClientCaps`.
    pub caps: ClientCaps,
    /// HDR10 capabilities, supplied by the platform. Whenever the host speaks protocol 0.5 a
    /// `ClientHdr` is sent right after the host `Hello`; `None` sends [`ClientHdr::none`]
    /// (no HDR10), so the host always gets an answer and never waits for it in vain.
    pub hdr: Option<ClientHdr>,
    /// Interval of `ClientStats` messages.
    pub stats_interval: Duration,
    /// Interval of client-initiated pings (clock sync).
    pub ping_interval: Duration,
    /// Give up if the session does not end within this time.
    pub max_duration: Option<Duration>,
    /// End the session when nothing at all arrives from the host for this long. The host answers
    /// every ping, so silence means it is gone; a USB accessory link never reports that itself.
    pub host_silence: Option<Duration>,
    /// Handshake step timeout.
    pub handshake_timeout: Duration,
    /// Reason in the `Bye` sent when the client ends the session.
    pub bye_reason: String,
    /// Protocol minor to advertise in `Hello` instead of ours (to test older clients).
    pub proto_minor: Option<u16>,
}

impl ClientConfig {
    /// Defaults (1 s stats, 500 ms pings, 5 s host silence, 10 s handshake steps) for `caps`.
    pub fn new(device_name: impl Into<String>, caps: ClientCaps) -> Self {
        ClientConfig {
            device_name: device_name.into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            caps,
            hdr: None,
            stats_interval: Duration::from_secs(1),
            ping_interval: Duration::from_millis(500),
            max_duration: None,
            host_silence: Some(Duration::from_secs(5)),
            handshake_timeout: Duration::from_secs(10),
            bye_reason: "client done".into(),
            proto_minor: None,
        }
    }
}

/// Why the session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEnd {
    /// A handler returned [`Flow::Stop`] or [`ClientHandle::stop`] was called.
    Stopped,
    /// Host sent `Bye` or `Reject`.
    HostBye(String),
    /// Host closed the connection.
    HostEof,
    /// `max_duration` elapsed.
    TimedOut,
    /// Nothing arrived from the host for `host_silence`.
    HostSilent,
}

/// Result of a finished session.
#[derive(Debug, Clone)]
pub struct ClientOutcome {
    /// Negotiated configuration.
    pub stream: StreamConfig,
    /// Colour description of the stream (BT.709 8-bit when the host sent none).
    pub color: StreamColor,
    /// Why it ended.
    pub end: ClientEnd,
    /// Estimated `host_clock - client_clock` in microseconds.
    pub clock_offset_us: Option<i64>,
    /// Best ping RTT in microseconds.
    pub rtt_us: Option<i64>,
}

/// Errors of [`run_client`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Link failure.
    #[error(transparent)]
    Link(#[from] MuxError),
    /// The host refused or the handshake went wrong.
    #[error("handshake failed: {0}")]
    Handshake(String),
    /// A handler callback failed.
    #[error("handler failed: {0}")]
    Handler(HandlerError),
}

async fn wait_for<T>(
    rx: &mut MuxReceiver,
    tx: &MuxSender,
    what: &str,
    wait: Duration,
    mut pick: impl FnMut(Message) -> Result<T, Message>,
) -> Result<T, ClientError> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let m = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => {
                return Err(ClientError::Handshake(format!(
                    "timed out waiting for {what}"
                )));
            }
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(None)) => {
                return Err(ClientError::Handshake(format!(
                    "connection closed waiting for {what}"
                )));
            }
            Ok(Ok(Some(m))) => m,
        };
        match pick(m) {
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
            Err(Message::Pong(_)) | Err(Message::CursorShape(_)) | Err(Message::CursorState(_)) => {
            }
            Err(Message::Reject(r)) => {
                return Err(ClientError::Handshake(format!("rejected: {}", r.reason)));
            }
            Err(Message::Bye(b)) => {
                return Err(ClientError::Handshake(format!(
                    "host said bye: {}",
                    b.reason
                )));
            }
            Err(other) => {
                return Err(ClientError::Handshake(format!(
                    "expected {what}, got {other:?}"
                )));
            }
        }
    }
}

/// Runs the client over `link`: handshake, then the receive loop until a
/// handler/handle stop, a host `Bye`/EOF or `max_duration`.
pub async fn run_client<H: ClientHandler>(
    link: Link,
    cfg: ClientConfig,
    handle: &ClientHandle,
    handler: &mut H,
) -> Result<ClientOutcome, ClientError> {
    let (tx, mut rx, task) = link.into_mux(ezd_proto::DEFAULT_CHUNK_LEN);
    let wait = cfg.handshake_timeout;

    // Handshake.
    let mut my_hello = Hello::new(Role::Client, &cfg.app_version, &cfg.device_name);
    if let Some(minor) = cfg.proto_minor {
        my_hello.proto_minor = minor;
    }
    let my_minor_supports_hdr = my_hello.supports_hdr();
    tx.send_control(Message::Hello(my_hello))?;
    tx.send_control(Message::ClientCaps(cfg.caps.clone()))?;
    let hello = wait_for(&mut rx, &tx, "Hello", wait, |m| match m {
        Message::Hello(h) => Ok(h),
        o => Err(o),
    })
    .await?;
    hello
        .check_compatible()
        .map_err(|e| ClientError::Handshake(e.to_string()))?;
    if hello.supports_hdr() && my_minor_supports_hdr {
        tx.send_control(Message::ClientHdr(
            cfg.hdr.clone().unwrap_or_else(ClientHdr::none),
        ))?;
    }
    wait_for(&mut rx, &tx, "HostCaps", wait, |m| match m {
        Message::HostCaps(h) => Ok(h),
        o => Err(o),
    })
    .await?;
    // `StreamColor` (0.5) may precede `StreamConfig`; without it the stream is BT.709 8-bit.
    let mut color = StreamColor::sdr_bt709();
    let stream = loop {
        let next = wait_for(&mut rx, &tx, "StreamConfig", wait, |m| match m {
            Message::StreamConfig(c) => Ok(Some(c)),
            Message::StreamColor(c) => {
                color = c;
                Ok(None)
            }
            o => Err(o),
        })
        .await?;
        if let Some(c) = next {
            break c;
        }
    };
    handle.store_stream_color(color);
    handler.on_stream_color(&color);
    if let Err(e) = handler.on_stream_config(&stream) {
        let _ = tx.send_control(Message::Bye(Bye {
            reason: format!("decoder init failed: {e}"),
        }));
        tx.close();
        let _ = timeout(Duration::from_secs(1), task.join()).await;
        return Err(ClientError::Handler(e));
    }
    tx.send_control(Message::ConfigAck)?;
    if hello.supports_input() {
        handle.set_input_sender(Some(tx.clone()));
    }

    // Streaming.
    let mut ping_seq = 0u32;
    let mut ping_t = interval(cfg.ping_interval);
    ping_t.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut stats_t = interval(cfg.stats_interval);
    stats_t.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_stats = std::time::Instant::now();
    let deadline = cfg.max_duration.map(|d| tokio::time::Instant::now() + d);
    let mut loop_err: Option<ClientError> = None;
    let mut last_rx = tokio::time::Instant::now();

    let end = loop {
        let sleep_until =
            deadline.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));
        let silent_at = last_rx + cfg.host_silence.unwrap_or(Duration::from_secs(3600));
        let step: Result<Option<ClientEnd>, ClientError> = async {
            tokio::select! {
                _ = tokio::time::sleep_until(sleep_until), if deadline.is_some() => {
                    Ok(Some(ClientEnd::TimedOut))
                }
                _ = tokio::time::sleep_until(silent_at), if cfg.host_silence.is_some() => {
                    Ok(Some(ClientEnd::HostSilent))
                }
                _ = handle.0.notify.notified() => {
                    if handle.is_stopped() {
                        return Ok(Some(ClientEnd::Stopped));
                    }
                    if handle.take_keyframe_request() {
                        tx.send_control(Message::RequestKeyframe)?;
                    }
                    Ok(None)
                }
                _ = ping_t.tick() => {
                    ping_seq = ping_seq.wrapping_add(1);
                    tx.send_control(Message::Ping(Ping { seq: ping_seq, t0_us: now_us() }))?;
                    Ok(None)
                }
                _ = stats_t.tick() => {
                    let elapsed = last_stats.elapsed();
                    last_stats = std::time::Instant::now();
                    let st = handle.roll_window(elapsed.as_millis() as u32);
                    tx.send_control(Message::ClientStats(st))?;
                    Ok(None)
                }
                msg = rx.recv() => {
                    let Some(msg) = msg? else { return Ok(Some(ClientEnd::HostEof)) };
                    last_rx = tokio::time::Instant::now();
                    on_message(msg, &tx, handle, handler)
                }
            }
        }
        .await;
        match step {
            Ok(Some(end)) => break end,
            Ok(None) => {}
            Err(e) => {
                loop_err = Some(e);
                break ClientEnd::Stopped;
            }
        }
    };

    handle.set_input_sender(None);
    let outcome = ClientOutcome {
        stream,
        color,
        end,
        clock_offset_us: handle.clock_offset_us(),
        rtt_us: handle.rtt_us(),
    };

    // Clean shutdown: say goodbye, flush, then wait for the host to hang up.
    if !matches!(outcome.end, ClientEnd::HostBye(_) | ClientEnd::HostEof) {
        let _ = tx.send_control(Message::Bye(Bye {
            reason: cfg.bye_reason.clone(),
        }));
    }
    tx.close();
    let _ = timeout(Duration::from_secs(2), task.join()).await;
    if let Some(e) = loop_err {
        return Err(e);
    }
    if !matches!(outcome.end, ClientEnd::HostEof) {
        let _ = timeout(Duration::from_secs(2), async {
            while let Ok(Some(_)) = rx.recv().await {}
        })
        .await;
    }
    Ok(outcome)
}

fn on_message<H: ClientHandler>(
    msg: Message,
    tx: &MuxSender,
    handle: &ClientHandle,
    handler: &mut H,
) -> Result<Option<ClientEnd>, ClientError> {
    let flow = |r: HandlerResult| -> Result<Option<ClientEnd>, ClientError> {
        match r.map_err(ClientError::Handler)? {
            Flow::Continue => Ok(None),
            Flow::Stop => Ok(Some(ClientEnd::Stopped)),
        }
    };
    match msg {
        Message::Ping(p) => {
            let t1 = now_us();
            tx.send_control(Message::Pong(Pong {
                seq: p.seq,
                t0_us: p.t0_us,
                t1_us: t1,
                t2_us: now_us(),
            }))?;
            Ok(None)
        }
        Message::Pong(p) => {
            handle.add_clock_sample(p.t0_us, p.t1_us, p.t2_us, now_us());
            Ok(None)
        }
        Message::CursorShape(s) => {
            let s = handle.store_shape(s);
            handler.on_cursor_shape(&s);
            Ok(None)
        }
        Message::CursorState(s) => {
            handle.store_cursor_state(s);
            handler.on_cursor_state(&s);
            Ok(None)
        }
        Message::VideoFrame(v) if v.is_codec_config() => flow(handler.on_codec_config(v)),
        Message::VideoFrame(v) => {
            // Ack on reception, before decoding: releases the host's video credit.
            let received_us = now_us();
            tx.send_control(Message::FrameAck(FrameAck {
                frame_id: v.frame_id,
                received_us,
            }))?;
            handle.count_received(v.data.len());
            flow(handler.on_video_frame(v, received_us))
        }
        Message::InputStatus(st) => {
            handle.store_input_status(st);
            Ok(None)
        }
        Message::ClientOptions(o) => {
            handle.store_client_options(o);
            Ok(None)
        }
        Message::Bye(b) => Ok(Some(ClientEnd::HostBye(b.reason))),
        Message::Reject(r) => Ok(Some(ClientEnd::HostBye(r.reason))),
        other => {
            tracing::debug!(?other, "ignoring message");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezd_proto::{ClientCaps, Codec, DisplayInfo, EncoderCaps, HostCaps};

    struct Nop;

    impl ClientHandler for Nop {
        fn on_stream_config(&mut self, _: &StreamConfig) -> HandlerResult {
            Ok(Flow::Continue)
        }
        fn on_codec_config(&mut self, _: VideoFrame) -> HandlerResult {
            Ok(Flow::Continue)
        }
        fn on_video_frame(&mut self, _: VideoFrame, _: u64) -> HandlerResult {
            Ok(Flow::Continue)
        }
    }

    fn caps() -> ClientCaps {
        ClientCaps {
            display: DisplayInfo {
                width_px: 1280,
                height_px: 800,
                density_dpi: 160,
                refresh_rates_mhz: vec![60000],
                current_refresh_mhz: 60000,
            },
            decoders: vec![],
            stylus: false,
            max_touch_points: 0,
        }
    }

    /// A host that completes the handshake, answers pings while `answer` holds, then goes quiet
    /// without closing the link, like a host process that died behind a USB accessory.
    async fn host(link: Link, answer: Duration) {
        let (tx, mut rx, _task) = link.into_mux(ezd_proto::DEFAULT_CHUNK_LEN);
        tx.send_control(Message::Hello(Hello::new(Role::Host, "0.1.0", "pc")))
            .unwrap();
        tx.send_control(Message::HostCaps(HostCaps {
            os: "test".into(),
            gpu: "none".into(),
            encoders: vec![EncoderCaps {
                codec: Codec::H264,
                name: "test".into(),
                hardware: false,
            }],
        }))
        .unwrap();
        tx.send_control(Message::StreamConfig(StreamConfig {
            width: 1280,
            height: 800,
            refresh_mhz: 60000,
            codec: Codec::H264,
            bitrate_kbps: 1000,
            encoder_name: "test".into(),
        }))
        .unwrap();
        let quiet_at = tokio::time::Instant::now() + answer;
        while let Ok(Some(m)) = rx.recv().await {
            if let Message::Ping(p) = m
                && tokio::time::Instant::now() < quiet_at
            {
                let _ = tx.send_control(Message::Pong(Pong {
                    seq: p.seq,
                    t0_us: p.t0_us,
                    t1_us: 0,
                    t2_us: 0,
                }));
            }
        }
    }

    fn cfg() -> ClientConfig {
        let mut cfg = ClientConfig::new("tablet", caps());
        cfg.ping_interval = Duration::from_millis(20);
        cfg.host_silence = Some(Duration::from_millis(300));
        cfg
    }

    #[tokio::test]
    async fn a_host_that_goes_quiet_ends_the_session() {
        let (client, server) = Link::duplex_pair(64 * 1024);
        let host = tokio::spawn(host(server, Duration::from_millis(500)));
        let start = tokio::time::Instant::now();
        let out = run_client(client, cfg(), &ClientHandle::new(), &mut Nop)
            .await
            .unwrap();
        assert_eq!(out.end, ClientEnd::HostSilent);
        // Pongs kept it alive until the host went quiet.
        assert!(start.elapsed() >= Duration::from_millis(700));
        host.abort();
    }

    #[tokio::test]
    async fn no_silence_limit_keeps_waiting() {
        let (client, server) = Link::duplex_pair(64 * 1024);
        let host = tokio::spawn(host(server, Duration::ZERO));
        let mut cfg = cfg();
        cfg.host_silence = None;
        cfg.max_duration = Some(Duration::from_millis(600));
        let out = run_client(client, cfg, &ClientHandle::new(), &mut Nop)
            .await
            .unwrap();
        assert_eq!(out.end, ClientEnd::TimedOut);
        host.abort();
    }
}
