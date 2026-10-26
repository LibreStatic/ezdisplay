//! One connection attempt to the host: client core on a small tokio runtime plus the decoder.

use crate::audio::{AudioPlayer, opus_available};
use crate::caps::ParsedCaps;
use crate::decoder::{Cmd, DecoderCtl, SendWindow};
use crate::status::{Phase, Shared};
use crate::wifi::error_code;
use ezd_client_core::wifi::connect_paired;
use ezd_client_core::{
    ClientConfig, ClientEnd, ClientHandle, ClientHandler, CursorSnapshot, Flow, HandlerResult,
    run_client,
};
use ezd_link::{BlockingOptions, Link};
use ezd_proto::pair::Fingerprint;
use ezd_proto::{
    AudioCodec, AudioConfig, AudioPacket, ClientAudio, CursorShape, SampleFormat, StreamColor,
    StreamConfig, VideoFrame,
};
use std::fs::File;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The accessory gadget driver moves 16 KiB per transfer; smaller reads can fail or truncate.
const ACCESSORY_BUF: usize = 16 * 1024;
/// Wi-Fi sessions retry while the host is busy or not reachable yet, up to this long.
const WIFI_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long [`Session::stop`] lets the client say Bye over the accessory before revoking it.
const ACCESSORY_BYE_GRACE: Duration = Duration::from_millis(500);

/// Where a session connects to.
#[derive(Debug, Clone)]
pub enum Target {
    /// Raw TCP (the adb-reversed loopback port).
    Tcp {
        /// Host name or IP.
        host: String,
        /// TCP port.
        port: u16,
    },
    /// QUIC to a paired host, pinned to its certificate.
    Wifi {
        /// Host address.
        addr: SocketAddr,
        /// Fingerprint of the paired host certificate.
        fingerprint: Fingerprint,
        /// Directory with this device's identity and the paired hosts.
        state_dir: PathBuf,
    },
    /// The USB accessory file descriptor (Android Open Accessory). Owned: closed when the session ends.
    Accessory {
        /// Descriptor detached from the `ParcelFileDescriptor`.
        fd: AccessoryFd,
    },
}

/// The accessory descriptor, shared by the link's read and write threads without duplicating it.
///
/// The gadget driver lets one opener at a time have `/dev/usb_accessory`, and the system opens it
/// again on every new accessory handshake. A read blocks until the host sends something, even
/// after the cable is pulled, and keeps the device open while it waits: closing the descriptor
/// does not end it. [`AccessoryFd::revoke`] releases the device without waiting for that thread.
#[derive(Debug, Clone)]
pub struct AccessoryFd(Arc<AccessoryInner>);

#[derive(Debug)]
struct AccessoryInner {
    file: File,
    /// The thread inside `read`, if any.
    reader: Mutex<Option<libc::pthread_t>>,
}

impl AccessoryFd {
    pub fn new(fd: OwnedFd) -> Self {
        AccessoryFd(Arc::new(AccessoryInner {
            file: File::from(fd),
            reader: Mutex::new(None),
        }))
    }

    /// Points the descriptor at `/dev/null`, then interrupts a read in progress so it lets go of
    /// the device; restarted, it reads end of file. The number stays allocated (this value holds
    /// it), so a thread still using it never touches an unrelated descriptor.
    fn revoke(&self) {
        let null = match File::open("/dev/null") {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("cannot release the accessory: /dev/null: {e}");
                return;
            }
        };
        // SAFETY: both descriptors are open and owned by live `File`s; dup2 replaces the target
        // atomically, so its number is never free for another open to take.
        if unsafe { libc::dup2(null.as_raw_fd(), self.0.file.as_raw_fd()) } < 0 {
            tracing::warn!(
                "cannot release the accessory: {}",
                std::io::Error::last_os_error()
            );
            return;
        }
        let reader = self.0.reader.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(thread) = *reader {
            install_wake_handler();
            // SAFETY: the reader clears its entry under this lock before it exits, so `thread`
            // is a live thread. The handler does nothing; the signal only ends the wait.
            unsafe { libc::pthread_kill(thread, WAKE_SIGNAL) };
        }
    }
}

/// Interrupts a blocked accessory read. Nothing else in the app uses it, and its default action
/// (ignore) would not interrupt anything, hence the handler.
const WAKE_SIGNAL: libc::c_int = libc::SIGURG;

fn install_wake_handler() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    extern "C" fn wake(_: libc::c_int) {}
    ONCE.call_once(|| {
        // SAFETY: a zeroed sigaction is valid; the handler is async-signal-safe (empty).
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = wake as extern "C" fn(libc::c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(WAKE_SIGNAL, &sa, std::ptr::null_mut()) < 0 {
                tracing::warn!("cannot install the accessory wake handler");
            }
        }
    });
}

impl Read for AccessoryFd {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let lock = || self.0.reader.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: pthread_self has no preconditions.
        *lock() = Some(unsafe { libc::pthread_self() });
        let r = (&self.0.file).read(buf);
        *lock() = None;
        r
    }
}

impl Write for AccessoryFd {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        (&self.0.file).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Target {
    fn label(&self) -> String {
        match self {
            Target::Tcp { host, port } => format!("{host}:{port}"),
            Target::Wifi { addr, .. } => addr.to_string(),
            Target::Accessory { .. } => "usb-accessory".to_string(),
        }
    }
}

/// A running (or finished) session.
pub struct Session {
    handle: ClientHandle,
    shared: Arc<Shared>,
    dec: Arc<DecoderCtl>,
    thread: Option<JoinHandle<()>>,
    accessory: Option<AccessoryFd>,
}

struct Handler {
    shared: Arc<Shared>,
    dec: Arc<DecoderCtl>,
    /// Sent right before the matching `StreamConfig`; SDR BT.709 when the host sends none.
    color: StreamColor,
    /// Output for the PC's audio; present between an `AudioConfig` with a stream and the next one.
    audio: Option<AudioPlayer>,
}

impl ClientHandler for Handler {
    fn on_audio_config(&mut self, cfg: &AudioConfig) {
        // Drop the previous output first: it holds the audio device.
        self.audio = None;
        let Some(stream) = &cfg.stream else {
            tracing::info!("no audio in this session");
            return;
        };
        tracing::info!(
            "audio {:?} {} Hz, {} ch, {:?}, {} samples per packet",
            stream.codec,
            stream.sample_rate,
            stream.channels,
            stream.format,
            stream.frame_samples
        );
        match AudioPlayer::new(stream) {
            Ok(p) => self.audio = Some(p),
            Err(e) => tracing::warn!("audio disabled: {e}"),
        }
    }

    fn on_audio_packet(&mut self, pkt: AudioPacket) {
        if let Some(p) = &mut self.audio {
            p.on_packet(pkt);
        }
    }

    fn on_stream_color(&mut self, color: &StreamColor) {
        tracing::info!(
            "stream colour {}-bit primaries {} transfer {} matrix {} full_range {}",
            color.bit_depth,
            color.primaries,
            color.transfer,
            color.matrix,
            color.full_range
        );
        self.color = *color;
    }

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
        // Consume the colour so a later stream from a host that sends none is not tagged stale.
        let color = std::mem::take(&mut self.color);
        if !self.dec.send(Cmd::Stream(cfg.clone(), color)) {
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
    /// Starts connecting to `target`. `window` may be `None` until the surface exists.
    pub fn start(window: Option<SendWindow>, target: Target, caps: ParsedCaps) -> Session {
        let handle = ClientHandle::new();
        let shared = Arc::new(Shared::default());
        shared.set_phase(Phase::Connecting, target.label());
        let accessory = match &target {
            Target::Accessory { fd } => Some(fd.clone()),
            _ => None,
        };
        let dec = DecoderCtl::spawn(caps.caps.clone(), handle.clone(), shared.clone(), window);
        let thread = {
            let (handle, shared, dec) = (handle.clone(), shared.clone(), dec.clone());
            std::thread::Builder::new()
                .name("ezd-client".into())
                .spawn(move || run(handle, shared, dec, target, caps))
                .expect("spawn client thread")
        };
        Session {
            handle,
            shared,
            dec,
            thread: Some(thread),
            accessory,
        }
    }

    /// Stops the session. The decoder is released before returning; the network teardown
    /// (Bye, drain) finishes on a background thread so the caller never waits for the socket.
    /// An accessory is released before returning too: the app may be frozen right after.
    pub fn stop(mut self) {
        self.handle.stop();
        self.dec.shutdown(Duration::from_secs(2));
        if let Some(fd) = self.accessory.take() {
            let start = std::time::Instant::now();
            while self.thread.as_ref().is_some_and(|t| !t.is_finished())
                && start.elapsed() < ACCESSORY_BYE_GRACE
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            fd.revoke();
        }
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

    /// Hands one input message to the link; false if it was dropped.
    pub fn send_input(&self, msg: &ezd_proto::Message) -> bool {
        self.handle.send_input(msg)
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

/// Why a connection failed: English text for the status detail and logs, plus a stable code
/// Kotlin localizes (Wi-Fi only).
struct ConnectError {
    detail: String,
    code: Option<&'static str>,
}

impl From<String> for ConnectError {
    fn from(detail: String) -> Self {
        ConnectError { detail, code: None }
    }
}

/// Opens the link to `target`.
async fn connect(target: &Target) -> Result<Link, ConnectError> {
    match target {
        Target::Tcp { host, port } => {
            let addr = format!("{host}:{port}");
            match tokio::time::timeout(CONNECT_TIMEOUT, Link::connect_tcp(addr.as_str())).await {
                Ok(Ok(l)) => Ok(l),
                Ok(Err(e)) => Err(format!("connect {addr}: {e}").into()),
                Err(_) => Err(format!("connect {addr}: timed out").into()),
            }
        }
        Target::Wifi {
            addr,
            fingerprint,
            state_dir,
        } => connect_paired(state_dir, *addr, *fingerprint, WIFI_CONNECT_TIMEOUT)
            .await
            .map_err(|e| ConnectError {
                detail: format!("connect {addr}: {e}"),
                code: Some(error_code(&e)),
            }),
        Target::Accessory { fd } => {
            let io = |what: &str, e: std::io::Error| ConnectError::from(format!("{what}: {e}"));
            Link::from_blocking(
                fd.clone(),
                fd.clone(),
                "usb-accessory",
                BlockingOptions {
                    read_buf: ACCESSORY_BUF,
                    write_buf: ACCESSORY_BUF,
                    thread_name: "ezd-acc".into(),
                    ..Default::default()
                },
            )
            .map_err(|e| io("accessory link", e))
        }
    }
}

/// What this device plays: 48 kHz stereo PCM, and Opus when the device has a decoder.
fn client_audio(enabled: bool) -> ClientAudio {
    if !enabled {
        return ClientAudio::none();
    }
    let mut codecs = vec![AudioCodec::Pcm];
    if opus_available() {
        codecs.push(AudioCodec::Opus);
    }
    ClientAudio {
        enabled: true,
        sample_rates: vec![crate::jitter::SAMPLE_RATE],
        max_channels: 2,
        formats: vec![SampleFormat::F32, SampleFormat::S16],
        codecs,
    }
}

fn run(
    handle: ClientHandle,
    shared: Arc<Shared>,
    dec: Arc<DecoderCtl>,
    target: Target,
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
        let addr = target.label();
        let link = match connect(&target).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("{}", e.detail);
                match e.code {
                    Some(code) => shared.set_failed_code(e.detail, code),
                    None => shared.set_phase(Phase::Failed, e.detail),
                }
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
        cfg.hdr = caps.hdr;
        cfg.audio = Some(client_audio(caps.audio));
        let mut handler = Handler {
            shared: shared.clone(),
            dec: dec.clone(),
            color: StreamColor::default(),
            audio: None,
        };
        match run_client(link, cfg, &handle, &mut handler).await {
            Ok(o) => {
                tracing::info!("session ended: {:?}", o.end);
                let detail = match o.end {
                    ClientEnd::Stopped => "stopped".to_string(),
                    ClientEnd::HostBye(r) => format!("host: {r}"),
                    ClientEnd::HostEof => "host closed the connection".to_string(),
                    ClientEnd::TimedOut => "timed out".to_string(),
                    ClientEnd::HostSilent => "host stopped responding".to_string(),
                };
                shared.set_phase(Phase::Ended, detail);
            }
            Err(e) => {
                tracing::warn!("session failed: {e}");
                shared.set_phase(Phase::Failed, e.to_string());
            }
        }
    });
    // A session that ended on its own leaves the read thread waiting on the device.
    if let Target::Accessory { fd } = &target {
        fd.revoke();
    }
    dec.shutdown(Duration::from_secs(2));
}
