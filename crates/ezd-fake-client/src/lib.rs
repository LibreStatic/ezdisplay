//! Desktop stand-in for the Android client.
//!
//! [`run_fake_client`] performs the client side of the handshake, decodes the
//! video with [`ezd_ffmpeg::Decoder`], answers/sends pings, tracks cursor
//! messages, measures capture-to-decoded latency and reports statistics.
//! It is used by the integration tests and by the `ezd-fake-client` binary.

#[macro_use]
pub mod i18n;
mod script;
#[cfg(feature = "wifi")]
pub mod wifi;

use ezd_client_core::{
    ClientConfig, ClientError as CoreError, ClientHandle, ClientHandler, Flow, HandlerResult,
    now_us, run_client,
};
use ezd_ffmpeg::{Decoder, FfmpegError};
use ezd_host_core::synthetic::read_frame_number;
use ezd_link::{Link, MuxError};
use ezd_proto::{
    ClientCaps, ClientOptions, Codec, CursorShape, CursorState, DecoderCaps, DisplayInfo,
    InputStatus, StreamConfig, VideoFrame,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use ezd_client_core::percentile;
pub use script::{InputScript, play_script, script_messages};

/// Settings of the fake client.
#[derive(Debug, Clone)]
pub struct FakeClientConfig {
    /// `Hello.device_name`.
    pub device_name: String,
    /// Reported display width.
    pub width: u32,
    /// Reported display height.
    pub height: u32,
    /// Reported refresh rates (millihertz).
    pub refresh_rates_mhz: Vec<u32>,
    /// Exit after this many decoded frames.
    pub frames: Option<u64>,
    /// Write the Nth decoded frame (1-based) as an RGB PPM file.
    pub dump_frame: Option<(u64, PathBuf)>,
    /// Send `RequestKeyframe` once after this many decoded frames (for tests).
    pub request_keyframe_after: Option<u64>,
    /// Interpret frames as `SyntheticFrames` output and read back the frame number.
    pub verify_synthetic: bool,
    /// Interval of `ClientStats` messages.
    pub stats_interval: Duration,
    /// Interval of client-initiated pings (clock sync).
    pub ping_interval: Duration,
    /// Give up if the session does not end within this time.
    pub max_duration: Option<Duration>,
    /// Handshake step timeout.
    pub handshake_timeout: Duration,
    /// Input sequence to play once, starting at the first received video frame.
    pub input_script: Option<InputScript>,
    /// End the session shortly after the input script finished (`frames` still applies).
    pub stop_after_script: bool,
    /// Protocol minor to advertise in `Hello` instead of ours (to test older clients).
    pub proto_minor: Option<u16>,
}

impl Default for FakeClientConfig {
    fn default() -> Self {
        FakeClientConfig {
            device_name: "ezd-fake-client".into(),
            width: 1920,
            height: 1080,
            refresh_rates_mhz: vec![60_000, 120_000],
            frames: None,
            dump_frame: None,
            request_keyframe_after: None,
            verify_synthetic: true,
            stats_interval: Duration::from_secs(1),
            ping_interval: Duration::from_millis(500),
            max_duration: None,
            handshake_timeout: Duration::from_secs(10),
            input_script: None,
            stop_after_script: false,
            proto_minor: None,
        }
    }
}

impl FakeClientConfig {
    /// The `ClientCaps` this client advertises: one software H.264 decoder named "ffmpeg".
    pub fn caps(&self) -> ClientCaps {
        ClientCaps {
            display: DisplayInfo {
                width_px: self.width,
                height_px: self.height,
                density_dpi: 160,
                refresh_rates_mhz: self.refresh_rates_mhz.clone(),
                current_refresh_mhz: self.refresh_rates_mhz.first().copied().unwrap_or(60_000),
            },
            decoders: vec![DecoderCaps {
                codec: Codec::H264,
                name: "ffmpeg".into(),
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
}

/// Why the client run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEnd {
    /// Reached the requested number of frames and said goodbye.
    FramesReached,
    /// Host sent `Bye`.
    HostBye(String),
    /// Host closed the connection.
    HostEof,
    /// `max_duration` elapsed.
    TimedOut,
    /// The host stopped answering.
    HostSilent,
}

/// Everything the client observed.
#[derive(Debug, Clone)]
pub struct ClientReport {
    /// Negotiated configuration.
    pub stream: StreamConfig,
    /// Why it ended.
    pub end: ClientEnd,
    /// Video frames received (excluding codec-config frames).
    pub frames_received: u64,
    /// Frames decoded to a picture.
    pub frames_decoded: u64,
    /// Frames that could not be decoded or were skipped.
    pub frames_dropped: u64,
    /// Keyframes received.
    pub keyframes: u64,
    /// Codec-config frames received.
    pub codec_configs: u64,
    /// True if every codec-config frame preceded the first keyframe of the stream.
    pub config_before_first_keyframe: bool,
    /// `frame_id` of each received video frame, in order (capped at 100k entries).
    pub frame_ids: Vec<u64>,
    /// `frame_id` of each received keyframe.
    pub keyframe_ids: Vec<u64>,
    /// Frame number read back from each decoded picture (when `verify_synthetic`).
    pub frame_numbers: Vec<Option<u64>>,
    /// Cursor shapes received.
    pub cursor_shapes: u64,
    /// Cursor states received.
    pub cursor_states: u64,
    /// Cursor states referencing a shape that was never received.
    pub cursor_missing_shape: u64,
    /// Last cursor state.
    pub last_cursor: Option<CursorState>,
    /// Estimated `host_clock - client_clock` in microseconds.
    pub clock_offset_us: Option<i64>,
    /// Best ping RTT in microseconds.
    pub rtt_us: Option<i64>,
    /// Median decode time (microseconds) over the whole run.
    pub decode_us_p50: u32,
    /// 95th percentile decode time.
    pub decode_us_p95: u32,
    /// Median capture-to-decoded latency (microseconds) over the whole run.
    pub e2e_us_p50: u32,
    /// 95th percentile capture-to-decoded latency.
    pub e2e_us_p95: u32,
    /// Size in bytes of each received video frame, summed.
    pub video_bytes: u64,
    /// Latest `InputStatus` from the host.
    pub input_status: Option<InputStatus>,
    /// Latest `ClientOptions` from the host.
    pub client_options: Option<ClientOptions>,
    /// Input messages sent to the host.
    pub inputs_sent: u64,
}

impl ClientReport {
    /// Multi-line human readable summary.
    pub fn summary(&self) -> String {
        let s = &self.stream;
        // Every value is passed pre-formatted so Fluent does not apply locale number grouping.
        [
            fl!(
                "summary-stream",
                width = s.width.to_string(),
                height = s.height.to_string(),
                hz = format!("{:.3}", s.refresh_mhz as f64 / 1000.0),
                codec = format!("{:?}", s.codec),
                kbps = s.bitrate_kbps.to_string(),
                encoder = s.encoder_name.to_string()
            ),
            fl!("summary-end", end = format!("{:?}", self.end)),
            fl!(
                "summary-video",
                received = self.frames_received.to_string(),
                decoded = self.frames_decoded.to_string(),
                dropped = self.frames_dropped.to_string(),
                keyframes = self.keyframes.to_string(),
                configs = self.codec_configs.to_string(),
                bytes = self.video_bytes.to_string()
            ),
            fl!(
                "summary-cursor",
                shapes = self.cursor_shapes.to_string(),
                states = self.cursor_states.to_string(),
                missing = self.cursor_missing_shape.to_string(),
                last = format!("{:?}", self.last_cursor)
            ),
            fl!(
                "summary-clock",
                offset = format!("{:?}", self.clock_offset_us),
                rtt = format!("{:?}", self.rtt_us)
            ),
            fl!(
                "summary-decode",
                p50 = self.decode_us_p50.to_string(),
                p95 = self.decode_us_p95.to_string()
            ),
            fl!(
                "summary-e2e",
                p50 = self.e2e_us_p50.to_string(),
                p95 = self.e2e_us_p95.to_string()
            ),
            fl!(
                "summary-input",
                status = format!("{:?}", self.input_status),
                options = format!("{:?}", self.client_options),
                sent = self.inputs_sent.to_string()
            ),
        ]
        .join("\n")
    }
}

/// Errors of [`run_fake_client`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Link failure.
    #[error(transparent)]
    Link(#[from] MuxError),
    /// Client core failure.
    #[error(transparent)]
    Core(#[from] CoreError),
    /// IO failure (e.g. writing the PPM dump).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// FFmpeg failure.
    #[error(transparent)]
    Ffmpeg(#[from] FfmpegError),
    /// The host refused or the handshake went wrong.
    #[error("handshake failed: {0}")]
    Handshake(String),
}

/// Writes `rgb` (width*height*3) as a binary PPM.
pub fn write_ppm(path: &Path, width: u32, height: u32, rgb: &[u8]) -> std::io::Result<()> {
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    out.extend_from_slice(rgb);
    std::fs::write(path, out)
}

struct Handler {
    cfg: FakeClientConfig,
    handle: ClientHandle,
    decoder: Option<Decoder>,
    report: ClientReport,
    all_decode: Vec<u32>,
    all_e2e: Vec<u32>,
    have_keyframe: bool,
    keyframe_requested: bool,
    kf_request_sent: bool,
    /// Fires the input script on the first received video frame.
    start_script: Option<tokio::sync::oneshot::Sender<()>>,
}

fn boxed(e: impl Into<ClientError>) -> ezd_client_core::HandlerError {
    Box::new(e.into())
}

impl Handler {
    fn decode_frame(&mut self, v: &VideoFrame) -> Result<Flow, ClientError> {
        let decoder = self
            .decoder
            .as_mut()
            .expect("decoder created on StreamConfig");
        let t = std::time::Instant::now();
        match decoder.decode(&v.data, v.frame_id as i64) {
            Ok(pics) => {
                let done = now_us();
                let dus = t.elapsed().as_micros() as u32;
                for pic in pics {
                    self.report.frames_decoded += 1;
                    self.all_decode.push(dus);
                    if let Some(e2e) = self.handle.report_decoded(dus, v.capture_us, done) {
                        self.all_e2e.push(e2e);
                    }
                    if self.cfg.verify_synthetic {
                        self.report.frame_numbers.push(read_frame_number(
                            &pic.y,
                            pic.width as usize,
                            pic.width,
                            pic.height,
                        ));
                    }
                    if let Some((n, path)) = &self.cfg.dump_frame
                        && *n == self.report.frames_decoded
                    {
                        write_ppm(path, pic.width, pic.height, &pic.to_rgb24())?;
                        tracing::info!(path = %path.display(), "dumped frame {n}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!("decode failed for frame {}: {e}", v.frame_id);
                self.drop_frame();
            }
        }
        Ok(Flow::Continue)
    }

    fn drop_frame(&mut self) {
        self.report.frames_dropped += 1;
        self.handle.report_dropped(1);
        if !self.keyframe_requested {
            self.keyframe_requested = true;
            self.handle.request_keyframe();
        }
    }
}

impl ClientHandler for Handler {
    fn on_stream_config(&mut self, cfg: &StreamConfig) -> HandlerResult {
        self.report.stream = cfg.clone();
        self.decoder = Some(Decoder::new(cfg.codec).map_err(boxed)?);
        Ok(Flow::Continue)
    }

    fn on_codec_config(&mut self, v: VideoFrame) -> HandlerResult {
        self.report.codec_configs += 1;
        if self.have_keyframe {
            // A change of parameter sets mid-stream is legal; only the first one is checked.
            tracing::debug!("codec config after first keyframe");
        }
        if let Some(d) = self.decoder.as_mut() {
            d.push_config(&v.data).map_err(boxed)?;
        }
        Ok(Flow::Continue)
    }

    fn on_video_frame(&mut self, v: VideoFrame, _received_us: u64) -> HandlerResult {
        let report = &mut self.report;
        report.frames_received += 1;
        if let Some(s) = self.start_script.take() {
            let _ = s.send(());
        }
        report.video_bytes += v.data.len() as u64;
        if report.frame_ids.len() < 100_000 {
            report.frame_ids.push(v.frame_id);
        }
        if v.is_keyframe() {
            report.keyframes += 1;
            report.keyframe_ids.push(v.frame_id);
            if !self.have_keyframe && report.codec_configs == 0 {
                report.config_before_first_keyframe = false;
            }
            self.have_keyframe = true;
            self.keyframe_requested = false;
        } else if !self.have_keyframe {
            self.drop_frame();
            return Ok(Flow::Continue);
        }
        self.decode_frame(&v).map_err(boxed)?;
        if let Some(n) = self.cfg.request_keyframe_after
            && !self.kf_request_sent
            && self.report.frames_decoded >= n
        {
            self.kf_request_sent = true;
            self.handle.request_keyframe();
        }
        if let Some(n) = self.cfg.frames
            && self.report.frames_decoded >= n
        {
            return Ok(Flow::Stop);
        }
        Ok(Flow::Continue)
    }

    fn on_cursor_shape(&mut self, _shape: &CursorShape) {
        self.report.cursor_shapes += 1;
    }

    fn on_cursor_state(&mut self, s: &CursorState) {
        self.report.cursor_states += 1;
        if s.visible && self.handle.cursor_shape(s.shape_id).is_none() {
            self.report.cursor_missing_shape += 1;
        }
        self.report.last_cursor = Some(*s);
    }
}

/// Runs the client over `link` until the configured frame count, a host `Bye`/EOF,
/// or `max_duration`.
pub async fn run_fake_client(
    link: Link,
    cfg: FakeClientConfig,
) -> Result<ClientReport, ClientError> {
    let mut core_cfg = ClientConfig::new(cfg.device_name.clone(), cfg.caps());
    core_cfg.stats_interval = cfg.stats_interval;
    core_cfg.ping_interval = cfg.ping_interval;
    core_cfg.max_duration = cfg.max_duration;
    core_cfg.handshake_timeout = cfg.handshake_timeout;
    core_cfg.app_version = env!("CARGO_PKG_VERSION").into();

    core_cfg.proto_minor = cfg.proto_minor;

    let handle = ClientHandle::new();
    let (start_script, script_task) = match cfg.input_script {
        Some(script) => {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let (handle, stop) = (handle.clone(), cfg.stop_after_script);
            let task = tokio::spawn(async move {
                // The sender is dropped without firing if the session ends first.
                if rx.await.is_err() {
                    return 0;
                }
                let sent = play_script(script, &handle).await;
                // Let the host drain the input queue before a possible `Bye`.
                tokio::time::sleep(Duration::from_millis(200)).await;
                if stop {
                    handle.stop();
                }
                sent
            });
            (Some(tx), Some(task))
        }
        None => (None, None),
    };
    let mut h = Handler {
        cfg,
        handle: handle.clone(),
        decoder: None,
        report: ClientReport {
            stream: StreamConfig {
                width: 0,
                height: 0,
                refresh_mhz: 0,
                codec: Codec::H264,
                bitrate_kbps: 0,
                encoder_name: String::new(),
            },
            end: ClientEnd::HostEof,
            frames_received: 0,
            frames_decoded: 0,
            frames_dropped: 0,
            keyframes: 0,
            codec_configs: 0,
            config_before_first_keyframe: true,
            frame_ids: Vec::new(),
            keyframe_ids: Vec::new(),
            frame_numbers: Vec::new(),
            cursor_shapes: 0,
            cursor_states: 0,
            cursor_missing_shape: 0,
            last_cursor: None,
            clock_offset_us: None,
            rtt_us: None,
            decode_us_p50: 0,
            decode_us_p95: 0,
            e2e_us_p50: 0,
            e2e_us_p95: 0,
            video_bytes: 0,
            input_status: None,
            client_options: None,
            inputs_sent: 0,
        },
        all_decode: Vec::new(),
        all_e2e: Vec::new(),
        have_keyframe: false,
        keyframe_requested: false,
        kf_request_sent: false,
        start_script,
    };

    let result = run_client(link, core_cfg, &handle, &mut h).await;
    if let Some(t) = script_task {
        t.abort();
    }
    let outcome = match result {
        Ok(o) => o,
        Err(CoreError::Handler(e)) => {
            return Err(match e.downcast::<ClientError>() {
                Ok(own) => *own,
                Err(other) => ClientError::Handshake(other.to_string()),
            });
        }
        Err(e) => return Err(e.into()),
    };

    let mut report = h.report;
    report.end = match outcome.end {
        ezd_client_core::ClientEnd::Stopped => ClientEnd::FramesReached,
        ezd_client_core::ClientEnd::HostBye(r) => ClientEnd::HostBye(r),
        ezd_client_core::ClientEnd::HostEof => ClientEnd::HostEof,
        ezd_client_core::ClientEnd::TimedOut => ClientEnd::TimedOut,
        ezd_client_core::ClientEnd::HostSilent => ClientEnd::HostSilent,
    };
    report.clock_offset_us = outcome.clock_offset_us;
    report.rtt_us = outcome.rtt_us;
    let snap = handle.snapshot();
    report.input_status = snap.input_status;
    report.client_options = snap.client_options;
    report.inputs_sent = snap.inputs_sent;
    report.decode_us_p50 = percentile(&h.all_decode, 50);
    report.decode_us_p95 = percentile(&h.all_decode, 95);
    report.e2e_us_p50 = percentile(&h.all_e2e, 50);
    report.e2e_us_p95 = percentile(&h.all_e2e, 95);
    Ok(report)
}
