//! One connection attempt to the host: client core on a small tokio runtime plus the decoder.

use crate::caps::ParsedCaps;
use crate::decoder::{Cmd, DecoderCtl, SendWindow};
use crate::status::{Phase, Shared};
use ezd_client_core::{
    ClientConfig, ClientEnd, ClientHandle, ClientHandler, CursorSnapshot, Flow, HandlerResult,
    run_client,
};
use ezd_link::Link;
use ezd_proto::{CursorShape, StreamConfig, VideoFrame};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A running (or finished) session.
pub struct Session {
    handle: ClientHandle,
    shared: Arc<Shared>,
    dec: Arc<DecoderCtl>,
    thread: Option<JoinHandle<()>>,
}

struct Handler {
    shared: Arc<Shared>,
    dec: Arc<DecoderCtl>,
}

impl ClientHandler for Handler {
    fn on_stream_config(&mut self, cfg: &StreamConfig) -> HandlerResult {
        tracing::info!(
            "stream {}x{} @ {:.3} Hz {:?} {} kbps ({})",
            cfg.width,
            cfg.height,
            cfg.refresh_mhz as f64 / 1000.0,
            cfg.codec,
            cfg.bitrate_kbps,
            cfg.encoder_name
        );
        self.shared.set_stream(cfg.clone());
        if !self.dec.send(Cmd::Stream(cfg.clone())) {
            return Err("decoder thread is gone".into());
        }
        self.shared.set_phase(Phase::Streaming, "");
        Ok(Flow::Continue)
    }

    fn on_codec_config(&mut self, frame: VideoFrame) -> HandlerResult {
        self.dec.send(Cmd::Config(frame.data));
        Ok(Flow::Continue)
    }

    fn on_video_frame(&mut self, frame: VideoFrame, _received_us: u64) -> HandlerResult {
        self.dec.send(Cmd::Frame(frame));
        Ok(Flow::Continue)
    }

    fn on_cursor_shape(&mut self, shape: &CursorShape) {
        tracing::debug!(
            "cursor shape {} {}x{}",
            shape.shape_id,
            shape.width,
            shape.height
        );
    }
}

impl Session {
    /// Starts connecting to `host:port`. `window` may be `None` until the surface exists.
    pub fn start(window: Option<SendWindow>, host: String, port: u16, caps: ParsedCaps) -> Session {
        let handle = ClientHandle::new();
        let shared = Arc::new(Shared::default());
        shared.set_phase(Phase::Connecting, format!("{host}:{port}"));
        let dec = DecoderCtl::spawn(caps.caps.clone(), handle.clone(), shared.clone(), window);
        let thread = {
            let (handle, shared, dec) = (handle.clone(), shared.clone(), dec.clone());
            std::thread::Builder::new()
                .name("ezd-client".into())
                .spawn(move || run(handle, shared, dec, host, port, caps))
                .expect("spawn client thread")
        };
        Session {
            handle,
            shared,
            dec,
            thread: Some(thread),
        }
    }

    /// Stops the session. The decoder is released before returning; the network teardown
    /// (Bye, drain) finishes on a background thread so the caller never waits for the socket.
    pub fn stop(mut self) {
        self.handle.stop();
        self.dec.shutdown(Duration::from_secs(2));
        if let Some(t) = self.thread.take() {
            let _ = std::thread::Builder::new()
                .name("ezd-reaper".into())
                .spawn(move || {
                    let _ = t.join();
                });
        }
    }

    /// Attaches a new output surface.
    pub fn surface_changed(&self, window: SendWindow) {
        self.dec.send(Cmd::Surface(Some(window), None));
    }

    /// Detaches the output surface; returns once the codec no longer uses it.
    pub fn surface_destroyed(&self) {
        if !self
            .dec
            .send_and_wait(|a| Cmd::Surface(None, Some(a)), Duration::from_secs(2))
        {
            tracing::warn!("decoder did not release the surface in time");
        }
    }

    /// Asks the host for a keyframe.
    pub fn request_keyframe(&self) {
        self.handle.request_keyframe();
    }

    /// `pollStats()` JSON.
    pub fn stats_json(&self) -> String {
        self.shared.to_json(&self.handle.snapshot())
    }

    /// Latest cursor state.
    pub fn cursor(&self) -> Option<CursorSnapshot> {
        self.handle.cursor()
    }

    /// Cached cursor shape.
    pub fn cursor_shape(&self, id: u32) -> Option<Arc<CursorShape>> {
        self.handle.cursor_shape(id)
    }
}

fn run(
    handle: ClientHandle,
    shared: Arc<Shared>,
    dec: Arc<DecoderCtl>,
    host: String,
    port: u16,
    caps: ParsedCaps,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            shared.set_phase(Phase::Failed, format!("runtime: {e}"));
            return;
        }
    };
    rt.block_on(async {
        let addr = format!("{host}:{port}");
        let link =
            match tokio::time::timeout(CONNECT_TIMEOUT, Link::connect_tcp(addr.as_str())).await {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => {
                    shared.set_phase(Phase::Failed, format!("connect {addr}: {e}"));
                    return;
                }
                Err(_) => {
                    shared.set_phase(Phase::Failed, format!("connect {addr}: timed out"));
                    return;
                }
            };
        if handle.is_stopped() {
            shared.set_phase(Phase::Ended, "stopped");
            return;
        }
        shared.set_phase(Phase::Handshaking, addr);
        let mut cfg = ClientConfig::new(caps.device_name, caps.caps);
        cfg.app_version = env!("CARGO_PKG_VERSION").into();
        let mut handler = Handler {
            shared: shared.clone(),
            dec: dec.clone(),
        };
        match run_client(link, cfg, &handle, &mut handler).await {
            Ok(o) => {
                tracing::info!("session ended: {:?}", o.end);
                let detail = match o.end {
                    ClientEnd::Stopped => "stopped".to_string(),
                    ClientEnd::HostBye(r) => format!("host: {r}"),
                    ClientEnd::HostEof => "host closed the connection".to_string(),
                    ClientEnd::TimedOut => "timed out".to_string(),
                };
                shared.set_phase(Phase::Ended, detail);
            }
            Err(e) => {
                tracing::warn!("session failed: {e}");
                shared.set_phase(Phase::Failed, e.to_string());
            }
        }
    });
    dec.shutdown(Duration::from_secs(2));
}
