//! [`HostService`]: the accept loop, the adb watcher and the shared state.

use crate::adb::{AdbClient, AdbLocator, Watcher, locate_system_adb};
use crate::config::{AdbMode, ServiceConfig, SourceKind};
use crate::error::ServiceError;
use crate::status::{DeviceInfo, HostEvent, HostStatus, LiveSession, Transport, WifiInfo};
#[cfg(feature = "wifi")]
use crate::store::{ForgetError, PairedDevice, StoreError};
use ezd_host_core::{SessionEvent, VideoSource, run_host_session_with};
use ezd_link::Link;
use ezd_proto::Prefs;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

/// How often the adb watcher lists the devices.
pub const ADB_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Default lifetime of a pairing window.
#[cfg(feature = "wifi")]
const DEFAULT_PAIRING_TTL: Duration = ezd_link::pair::DEFAULT_WINDOW;
#[cfg(not(feature = "wifi"))]
const DEFAULT_PAIRING_TTL: Duration = Duration::from_secs(5 * 60);

/// Creates the video source for one client connection.
pub type SourceFactory =
    Arc<dyn Fn(&ServiceConfig) -> Result<Box<dyn VideoSource>, ServiceError> + Send + Sync>;

/// The parts of the service that tests replace.
#[derive(Clone)]
pub struct ServiceHooks {
    /// Delay between adb polls.
    pub adb_poll_interval: Duration,
    /// Finds adb; called again on every poll while it returns `None`.
    pub locate_adb: AdbLocator,
    /// Builds the source of a session.
    pub create_source: SourceFactory,
    /// How long a pairing window stays open.
    pub pairing_ttl: Duration,
}

impl Default for ServiceHooks {
    fn default() -> Self {
        ServiceHooks {
            adb_poll_interval: ADB_POLL_INTERVAL,
            locate_adb: Arc::new(locate_system_adb),
            create_source: Arc::new(create_source),
            pairing_ttl: DEFAULT_PAIRING_TTL,
        }
    }
}

/// The source the configuration asks for, exactly as `ezd-host run` created it.
fn create_source(cfg: &ServiceConfig) -> Result<Box<dyn VideoSource>, ServiceError> {
    match cfg.source {
        SourceKind::Synthetic => synthetic_source(),
        SourceKind::Virtual => crate::platform::create_virtual_source(&cfg.platform_options()),
    }
}

#[cfg(feature = "ffmpeg")]
fn synthetic_source() -> Result<Box<dyn VideoSource>, ServiceError> {
    Ok(Box::new(ezd_ffmpeg::SwVideoSource::new()))
}

#[cfg(not(feature = "ffmpeg"))]
fn synthetic_source() -> Result<Box<dyn VideoSource>, ServiceError> {
    Err(ServiceError::Source(
        "the synthetic source needs a build with the `ffmpeg` feature".into(),
    ))
}

// One live session at a time, so the size of the variant does not matter.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Phase {
    Waiting,
    Streaming(LiveSession),
    Error(ServiceError),
    Stopped,
}

pub(crate) struct State {
    pub(crate) phase: Phase,
    devices: Vec<DeviceInfo>,
    /// adb problem (not found, reverse failing); shown while no session is running.
    adb_error: Option<ServiceError>,
}

impl State {
    fn status(&self) -> HostStatus {
        match &self.phase {
            Phase::Stopped => HostStatus::Stopped,
            Phase::Streaming(s) => HostStatus::Streaming(LiveSession {
                devices: self.devices.clone(),
                ..s.clone()
            }),
            Phase::Error(e) => HostStatus::Error(e.clone()),
            Phase::Waiting => match &self.adb_error {
                Some(e) => HostStatus::Error(e.clone()),
                None => HostStatus::Waiting {
                    devices: self.devices.clone(),
                },
            },
        }
    }
}

pub(crate) struct Inner {
    pub(crate) cfg: watch::Sender<ServiceConfig>,
    status: watch::Sender<HostStatus>,
    events: broadcast::Sender<HostEvent>,
    state: Mutex<State>,
    /// The address being listened on; `None` while unbound (rebind failed).
    addr: watch::Sender<Option<SocketAddr>>,
    /// Ends the running session when it holds `true`.
    session_stop: Mutex<Option<Arc<watch::Sender<bool>>>>,
    /// Set by `stop`.
    pub(crate) stop: watch::Sender<bool>,
    pub(crate) hooks: ServiceHooks,
    /// One permit: held for as long as a session streams, whichever transport it came from.
    pub(crate) slot: Arc<Semaphore>,
    /// The Wi-Fi listener, when on.
    pub(crate) wifi_info: watch::Sender<Option<WifiInfo>>,
    #[cfg(feature = "wifi")]
    pub(crate) wifi: Option<Arc<crate::wifi::Wifi>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    pub(crate) fn update(&self, f: impl FnOnce(&mut State)) {
        let mut st = lock(&self.state);
        f(&mut st);
        let new = st.status();
        self.status.send_if_modified(|cur| {
            let changed = *cur != new;
            if changed {
                *cur = new;
            }
            changed
        });
    }

    pub(crate) fn emit(&self, ev: HostEvent) {
        let _ = self.events.send(ev);
    }

    fn on_session_event(&self, ev: SessionEvent, transport: Transport) {
        self.update(|st| match ev {
            SessionEvent::Configured {
                client_hello,
                stream,
                dynamic_range,
                ..
            } => {
                st.phase = Phase::Streaming(LiveSession {
                    device_name: client_hello.device_name,
                    stream,
                    dynamic_range,
                    devices: Vec::new(),
                    stats: None,
                    input: None,
                    transport,
                });
            }
            SessionEvent::Stats(s) => {
                if let Phase::Streaming(l) = &mut st.phase {
                    l.stats = Some(s);
                }
            }
            SessionEvent::Input(i) => {
                if let Phase::Streaming(l) = &mut st.phase {
                    l.input = Some(i);
                }
            }
        });
    }

    fn set_addr(&self, addr: Option<SocketAddr>) {
        self.addr.send_replace(addr);
        if let Some(a) = addr {
            tracing::info!("listening on {a}");
            self.emit(HostEvent::Listening(a));
        }
    }
}

/// Handle to the running service. Dropping it asks the tasks to stop without waiting; use
/// [`stop`](Self::stop) for a clean shutdown.
pub struct HostService {
    inner: Arc<Inner>,
    serve: Option<JoinHandle<()>>,
    watch: Option<JoinHandle<()>>,
    /// Wi-Fi tasks (accept loop, pairing watcher).
    wifi_tasks: Vec<JoinHandle<()>>,
}

impl HostService {
    /// Binds `cfg.listen` and starts serving on `handle`. A bind error is returned here.
    pub fn start(
        handle: &tokio::runtime::Handle,
        cfg: ServiceConfig,
    ) -> Result<HostService, ServiceError> {
        Self::start_with(handle, cfg, ServiceHooks::default())
    }

    /// Like [`start`](Self::start) with replaceable parts, for tests.
    pub fn start_with(
        handle: &tokio::runtime::Handle,
        cfg: ServiceConfig,
        hooks: ServiceHooks,
    ) -> Result<HostService, ServiceError> {
        let bind_err = |e: std::io::Error| ServiceError::Bind {
            addr: cfg.listen,
            reason: e.to_string(),
        };
        let std_listener = std::net::TcpListener::bind(cfg.listen).map_err(bind_err)?;
        std_listener.set_nonblocking(true).map_err(bind_err)?;
        let listener = {
            let _enter = handle.enter();
            TcpListener::from_std(std_listener).map_err(bind_err)?
        };
        let local = listener.local_addr().map_err(bind_err)?;

        #[cfg(feature = "wifi")]
        let wifi = match &cfg.wifi {
            Some(w) => Some(crate::wifi::Wifi::setup(handle, w, hooks.pairing_ttl)?),
            None => None,
        };
        let requested = cfg.listen;
        let status = HostStatus::Waiting {
            devices: Vec::new(),
        };
        let inner = Arc::new(Inner {
            cfg: watch::channel(cfg).0,
            status: watch::channel(status).0,
            events: broadcast::channel(64).0,
            state: Mutex::new(State {
                phase: Phase::Waiting,
                devices: Vec::new(),
                adb_error: None,
            }),
            addr: watch::channel(None).0,
            session_stop: Mutex::new(None),
            stop: watch::channel(false).0,
            hooks,
            slot: Arc::new(Semaphore::new(1)),
            wifi_info: watch::channel(None).0,
            #[cfg(feature = "wifi")]
            wifi,
        });
        inner.set_addr(Some(local));
        #[allow(unused_mut)]
        let mut wifi_tasks = Vec::new();
        #[cfg(feature = "wifi")]
        if let Some(w) = &inner.wifi {
            inner.wifi_info.send_replace(Some(w.info()));
            wifi_tasks.push(handle.spawn(crate::wifi::serve(inner.clone(), w.clone())));
            wifi_tasks.push(handle.spawn(crate::wifi::watch_pairing(inner.clone(), w.clone())));
        }
        let serve = handle.spawn(serve(inner.clone(), listener, requested));
        let watch = handle.spawn(watch_adb(inner.clone()));
        Ok(HostService {
            inner,
            serve: Some(serve),
            watch: Some(watch),
            wifi_tasks,
        })
    }

    /// The Wi-Fi listener; `None` while Wi-Fi is off.
    pub fn wifi(&self) -> watch::Receiver<Option<WifiInfo>> {
        self.inner.wifi_info.subscribe()
    }

    /// Opens the pairing window with a new PIN (the PIN and QR URI arrive as
    /// [`HostEvent::PairingOpen`]). Returns the PIN, or `None` when Wi-Fi is off.
    #[cfg(feature = "wifi")]
    pub fn open_pairing(&self) -> Option<String> {
        Some(self.inner.wifi.as_ref()?.open_pairing())
    }

    /// Emits [`HostEvent::PairingOpen`] again if a window is open: for a frontend that
    /// subscribed to [`events`](Self::events) after the service started with the window open.
    #[cfg(feature = "wifi")]
    pub fn refresh_pairing(&self) {
        if let Some(w) = &self.inner.wifi {
            w.refresh_pairing();
        }
    }

    /// Closes the pairing window.
    #[cfg(feature = "wifi")]
    pub fn close_pairing(&self) {
        if let Some(w) = &self.inner.wifi {
            w.close_pairing();
        }
    }

    /// The paired devices; `None` while Wi-Fi is off.
    #[cfg(feature = "wifi")]
    pub fn paired_devices(&self) -> Option<Result<Vec<PairedDevice>, StoreError>> {
        Some(self.inner.wifi.as_ref()?.paired())
    }

    /// Forgets the paired device matching `query` (name or fingerprint, see
    /// [`PairedStore::forget`](crate::store::PairedStore::forget)); `None` while Wi-Fi is off.
    /// If the device is streaming, its session ends at once.
    #[cfg(feature = "wifi")]
    pub fn forget_paired(&self, query: &str) -> Option<Result<PairedDevice, ForgetError>> {
        Some(self.inner.wifi.as_ref()?.forget(query))
    }

    /// Forgets the paired device with exactly this fingerprint (hex), never one that merely has
    /// it as a name; `None` while Wi-Fi is off. If the device is streaming, its session ends at
    /// once. Another process that forgets a device (`ezd-host paired forget`) cannot signal
    /// the service: its session ends at the next periodic check of the store (about 2 s).
    #[cfg(feature = "wifi")]
    pub fn forget_paired_fingerprint(
        &self,
        fingerprint: &str,
    ) -> Option<Result<PairedDevice, ForgetError>> {
        Some(self.inner.wifi.as_ref()?.forget_fingerprint(fingerprint))
    }

    /// Current state; changes are pushed to the receiver.
    pub fn status(&self) -> watch::Receiver<HostStatus> {
        self.inner.status.subscribe()
    }

    /// One-off notifications from now on.
    pub fn events(&self) -> broadcast::Receiver<HostEvent> {
        self.inner.events.subscribe()
    }

    /// The address the listener is bound to (resolves port 0); `None` while unbound.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        *self.inner.addr.borrow()
    }

    /// Replaces the configuration. It is used from the next session on; with `restart_session`
    /// the running session ends now (the tablet reconnects by itself). A changed `listen`
    /// rebinds as soon as no session is running.
    pub fn apply(&self, cfg: ServiceConfig, restart_session: bool) {
        self.inner.cfg.send_replace(cfg);
        if restart_session && let Some(tx) = &*lock(&self.inner.session_stop) {
            tx.send_replace(true);
        }
    }

    /// Ends the running session (the client is sent `Bye`), waits until its source has stopped,
    /// removes the adb reverses and stops serving. Do not exit the process before this returns:
    /// tearing a hardware encoder down while the process exits can deadlock in the driver.
    pub async fn stop(mut self) {
        self.inner.stop.send_replace(true);
        let tasks = [self.serve.take(), self.watch.take()]
            .into_iter()
            .flatten()
            .chain(std::mem::take(&mut self.wifi_tasks));
        for task in tasks {
            if let Err(e) = task.await {
                tracing::error!("service task failed: {e}");
            }
        }
        crate::platform::release_virtual_display();
        self.inner.addr.send_replace(None);
        self.inner.wifi_info.send_replace(None);
        self.inner.update(|st| st.phase = Phase::Stopped);
    }
}

impl Drop for HostService {
    fn drop(&mut self) {
        self.inner.stop.send_replace(true);
    }
}

/// Accepts clients one at a time and runs their sessions.
async fn serve(inner: Arc<Inner>, first: TcpListener, requested: SocketAddr) {
    let mut stop_rx = inner.stop.subscribe();
    let mut cfg_rx = inner.cfg.subscribe();
    let mut listener = Some(first);
    let mut bound_for = requested;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let cfg = cfg_rx.borrow_and_update().clone();
        if cfg.listen != bound_for {
            // Free the old address first: the new one may overlap it.
            listener = None;
            inner.addr.send_replace(None);
            bound_for = cfg.listen;
            match TcpListener::bind(cfg.listen).await {
                Ok(l) => {
                    inner.set_addr(l.local_addr().ok());
                    listener = Some(l);
                    inner.update(|st| st.phase = Phase::Waiting);
                }
                Err(e) => {
                    let err = ServiceError::Bind {
                        addr: cfg.listen,
                        reason: e.to_string(),
                    };
                    tracing::warn!("{err}");
                    inner.update(|st| st.phase = Phase::Error(err));
                }
            }
        }
        let Some(l) = &listener else {
            // Not listening: wait for another address.
            tokio::select! {
                _ = cfg_rx.changed() => {}
                _ = stop_rx.changed() => {}
            }
            continue;
        };
        let accepted = tokio::select! {
            r = l.accept() => r,
            _ = cfg_rx.changed() => continue,
            _ = stop_rx.changed() => break,
        };
        let (sock, peer) = match accepted {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        tracing::info!(%peer, "client connected");
        inner.emit(HostEvent::ClientConnected(peer));
        let link = match Link::from_tcp(sock) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(%peer, "cannot use the connection: {e}");
                continue;
            }
        };
        // One session at a time across transports: while a Wi-Fi session streams, this client
        // waits for it (like a second client in the listen backlog did before).
        let permit = tokio::select! {
            p = inner.slot.clone().acquire_owned() => p.expect("the slot is never closed"),
            _ = stop_rx.changed() => break,
        };
        stream_session(&inner, &cfg, link, Transport::Tcp, &mut stop_rx).await;
        drop(permit);
    }
    // Close the listener before reporting that serving stopped.
    drop(listener);
}

/// Creates the source and runs one session on `link`; the caller holds the session slot.
async fn stream_session(
    inner: &Inner,
    cfg: &ServiceConfig,
    link: Link,
    transport: Transport,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let source = match (inner.hooks.create_source)(cfg) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("cannot create virtual source: {e}");
            inner.emit(HostEvent::SourceFailed(e.clone()));
            inner.update(|st| st.phase = Phase::Error(e));
            return;
        }
    };
    finish_session(inner, cfg, link, transport, source, stop_rx).await;
}

/// Runs a session with an existing source and reports its end.
pub(crate) async fn finish_session(
    inner: &Inner,
    cfg: &ServiceConfig,
    link: Link,
    transport: Transport,
    source: Box<dyn VideoSource>,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let result = run_session(inner, cfg, link, transport, source, stop_rx).await;
    inner.update(|st| st.phase = Phase::Waiting);
    match &result {
        Ok(s) => tracing::debug!(end = ?s.end, "session returned"),
        Err(e) => tracing::debug!("session failed: {e}"),
    }
    inner.emit(HostEvent::SessionEnded(result.map_err(|e| e.to_string())));
}

async fn run_session(
    inner: &Inner,
    cfg: &ServiceConfig,
    link: Link,
    transport: Transport,
    source: Box<dyn VideoSource>,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<ezd_host_core::SessionSummary, ezd_host_core::SessionError> {
    let (end_tx, end_rx) = watch::channel(false);
    let end_tx = Arc::new(end_tx);
    *lock(&inner.session_stop) = Some(end_tx.clone());
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    // A Wi-Fi session is always a network link, whatever `cfg.link` says.
    let prefs: Prefs = match transport {
        Transport::Tcp => cfg.prefs(),
        Transport::Wifi => cfg.wifi_prefs(),
    };
    let session = run_host_session_with(
        link,
        source,
        prefs,
        cfg.session_options(end_rx, Some(ev_tx)),
    );
    tokio::pin!(session);
    let mut stop_forwarded = false;
    let result = loop {
        tokio::select! {
            r = &mut session => break r,
            Some(ev) = ev_rx.recv() => inner.on_session_event(ev, transport),
            _ = stop_rx.changed(), if !stop_forwarded => {
                // The session stops the source before it returns.
                tracing::info!("stopping, ending the session");
                stop_forwarded = true;
                end_tx.send_replace(true);
            }
        }
    };
    *lock(&inner.session_stop) = None;
    result
}

/// Keeps `adb reverse` in place for the handled devices and publishes their state.
async fn watch_adb(inner: Arc<Inner>) {
    let mut stop_rx = inner.stop.subscribe();
    let mut cfg_rx = inner.cfg.subscribe();
    let mut addr_rx = inner.addr.subscribe();
    let watcher = Arc::new(Mutex::new(Watcher::default()));
    let mut adb: Option<Arc<dyn AdbClient>> = None;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let cfg = cfg_rx.borrow_and_update().clone();
        let port = addr_rx.borrow_and_update().map(|a| a.port());
        if cfg.adb == AdbMode::Off {
            if let Some(a) = &adb {
                release_all(&watcher, a).await;
            }
            inner.update(|st| {
                st.devices.clear();
                st.adb_error = None;
            });
        } else {
            if adb.is_none() {
                let locate = inner.hooks.locate_adb.clone();
                adb = tokio::task::spawn_blocking(move || locate())
                    .await
                    .ok()
                    .flatten();
            }
            match &adb {
                None => inner.update(|st| {
                    st.devices.clear();
                    st.adb_error = Some(ServiceError::AdbNotFound);
                }),
                Some(a) => {
                    let (a, w) = (a.clone(), watcher.clone());
                    let polled = tokio::task::spawn_blocking(move || {
                        lock(&w).poll(&*a, &cfg.adb, port, cfg.open_app_on_connect)
                    })
                    .await;
                    match polled {
                        Ok(p) => inner.update(|st| {
                            st.devices = p.devices;
                            st.adb_error = p.error;
                        }),
                        Err(e) => tracing::error!("adb poll failed: {e}"),
                    }
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(inner.hooks.adb_poll_interval) => {}
            _ = cfg_rx.changed() => {}
            _ = addr_rx.changed() => {}
            _ = stop_rx.changed() => {}
        }
    }
    if let Some(a) = &adb {
        release_all(&watcher, a).await;
    }
}

async fn release_all(watcher: &Arc<Mutex<Watcher>>, adb: &Arc<dyn AdbClient>) {
    let (w, a) = (watcher.clone(), adb.clone());
    let _ = tokio::task::spawn_blocking(move || {
        let mut w = lock(&w);
        if w.has_reverses() {
            w.release_all(&*a);
        }
    })
    .await;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::adb::fake::{FakeAdb, device};
    use ezd_host_core::testing::ScriptedSource;
    use ezd_link::adb::DeviceState as AdbState;
    use ezd_link::{MuxReceiver, MuxSender};
    use ezd_proto::{
        Bye, ClientCaps, ClientHdr, Codec, DEFAULT_CHUNK_LEN, DecoderCaps, DisplayInfo, Hello,
        Message, Role,
    };
    use tokio::time::timeout;

    pub(crate) const WAIT: Duration = Duration::from_secs(10);

    pub(crate) fn cfg() -> ServiceConfig {
        ServiceConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            adb: AdbMode::Off,
            input: false,
            ..ServiceConfig::default()
        }
    }

    pub(crate) fn hooks() -> ServiceHooks {
        ServiceHooks {
            adb_poll_interval: Duration::from_millis(20),
            locate_adb: Arc::new(|| None),
            create_source: Arc::new(|_| {
                Ok(Box::new(ScriptedSource::new(Duration::from_millis(5), 200)))
            }),
            pairing_ttl: Duration::from_secs(60),
        }
    }

    pub(crate) fn start(cfg: ServiceConfig, hooks: ServiceHooks) -> HostService {
        HostService::start_with(&tokio::runtime::Handle::current(), cfg, hooks).unwrap()
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    pub(crate) fn caps() -> ClientCaps {
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

    /// Connects like the tablet and completes the handshake.
    pub(crate) async fn connect(addr: SocketAddr) -> (MuxSender, MuxReceiver) {
        let sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        handshake(Link::from_tcp(sock).unwrap()).await
    }

    /// Completes the handshake on `link` like the tablet.
    pub(crate) async fn handshake(link: Link) -> (MuxSender, MuxReceiver) {
        let (tx, mut rx, _t) = link.into_mux(DEFAULT_CHUNK_LEN);
        tx.send_control(Message::Hello(Hello::new(Role::Client, "t", "tablet")))
            .unwrap();
        tx.send_control(Message::ClientCaps(caps())).unwrap();
        tx.send_control(Message::ClientHdr(ClientHdr::none()))
            .unwrap();
        let mut seen = 0;
        while seen < 4 {
            match timeout(WAIT, rx.recv()).await.unwrap().unwrap().unwrap() {
                Message::Hello(_)
                | Message::HostCaps(_)
                | Message::StreamColor(_)
                | Message::StreamConfig(_) => seen += 1,
                m => panic!("{m:?}"),
            }
        }
        tx.send_control(Message::ConfigAck).unwrap();
        (tx, rx)
    }

    pub(crate) async fn wait_status(
        rx: &mut watch::Receiver<HostStatus>,
        pred: impl FnMut(&HostStatus) -> bool,
    ) -> HostStatus {
        timeout(WAIT, rx.wait_for(pred))
            .await
            .unwrap()
            .unwrap()
            .clone()
    }

    pub(crate) async fn wait_session_end(
        ev: &mut broadcast::Receiver<HostEvent>,
    ) -> ezd_host_core::EndReason {
        loop {
            if let HostEvent::SessionEnded(r) = timeout(WAIT, ev.recv()).await.unwrap().unwrap() {
                return r.unwrap().end;
            }
        }
    }

    #[tokio::test]
    async fn bind_error_is_returned_by_start() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = taken.local_addr().unwrap();
        let r = HostService::start_with(
            &tokio::runtime::Handle::current(),
            ServiceConfig {
                listen: addr,
                ..cfg()
            },
            hooks(),
        );
        assert!(matches!(r, Err(ServiceError::Bind { addr: a, .. }) if a == addr));
    }

    #[tokio::test]
    async fn streams_one_client_and_reports_the_session() {
        let svc = start(cfg(), hooks());
        let mut status = svc.status();
        let mut events = svc.events();
        assert_eq!(*status.borrow(), HostStatus::Waiting { devices: vec![] });
        let (tx, _rx) = connect(svc.local_addr().unwrap()).await;
        let HostStatus::Streaming(live) =
            wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await
        else {
            unreachable!()
        };
        assert_eq!(live.device_name, "tablet");
        assert_eq!(live.stream.width, 1280);
        assert_eq!(live.dynamic_range, ezd_proto::DynamicRange::Sdr);
        tx.send_control(Message::Bye(Bye {
            reason: "done".into(),
        }))
        .unwrap();
        assert_eq!(
            wait_session_end(&mut events).await,
            ezd_host_core::EndReason::ClientBye("done".into())
        );
        wait_status(&mut status, |s| matches!(s, HostStatus::Waiting { .. })).await;
        svc.stop().await;
    }

    #[tokio::test]
    async fn stop_says_bye_and_reports_stopped() {
        let svc = start(cfg(), hooks());
        let mut status = svc.status();
        let mut events = svc.events();
        let (_tx, mut rx) = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        timeout(WAIT, svc.stop()).await.unwrap();
        assert_eq!(*status.borrow(), HostStatus::Stopped);
        assert_eq!(
            wait_session_end(&mut events).await,
            ezd_host_core::EndReason::HostShutdown
        );
        let mut said_bye = false;
        while let Ok(Ok(Some(m))) = timeout(WAIT, rx.recv()).await {
            said_bye |= matches!(m, Message::Bye(_));
        }
        assert!(said_bye);
    }

    #[tokio::test]
    async fn apply_with_restart_ends_the_session_and_keeps_serving() {
        let svc = start(cfg(), hooks());
        let mut status = svc.status();
        let mut events = svc.events();
        let (_tx, _rx) = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        svc.apply(
            ServiceConfig {
                telemetry: true,
                ..cfg()
            },
            true,
        );
        assert_eq!(
            wait_session_end(&mut events).await,
            ezd_host_core::EndReason::HostShutdown
        );
        wait_status(&mut status, |s| matches!(s, HostStatus::Waiting { .. })).await;
        // The tablet reconnects.
        let _again = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        svc.stop().await;
    }

    #[tokio::test]
    async fn apply_without_restart_leaves_the_session_alone() {
        let svc = start(cfg(), hooks());
        let mut status = svc.status();
        let (_tx, _rx) = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        svc.apply(
            ServiceConfig {
                telemetry: true,
                ..cfg()
            },
            false,
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(matches!(*status.borrow(), HostStatus::Streaming(_)));
        svc.stop().await;
    }

    #[tokio::test]
    async fn a_changed_listen_address_rebinds() {
        let (a, b) = (free_port(), free_port());
        let at = |p: u16| ServiceConfig {
            listen: SocketAddr::from(([127, 0, 0, 1], p)),
            ..cfg()
        };
        let svc = start(at(a), hooks());
        svc.apply(at(b), false);
        timeout(WAIT, async {
            while svc.local_addr().map(|x| x.port()) != Some(b) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", a))
                .await
                .is_err()
        );
        let mut status = svc.status();
        let (_tx, _rx) = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        svc.stop().await;
    }

    #[tokio::test]
    async fn a_failed_rebind_is_reported() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let svc = start(cfg(), hooks());
        let mut status = svc.status();
        svc.apply(
            ServiceConfig {
                listen: taken.local_addr().unwrap(),
                ..cfg()
            },
            false,
        );
        let s = wait_status(&mut status, |s| matches!(s, HostStatus::Error(_))).await;
        assert!(matches!(s, HostStatus::Error(ServiceError::Bind { .. })));
        assert_eq!(svc.local_addr(), None);
        svc.stop().await;
    }

    #[tokio::test]
    async fn a_failing_source_is_reported_and_the_service_recovers() {
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut h = hooks();
        let f = fail.clone();
        h.create_source = Arc::new(move |_| {
            if f.load(std::sync::atomic::Ordering::SeqCst) {
                Err(ServiceError::Source("no gpu".into()))
            } else {
                Ok(Box::new(ScriptedSource::new(Duration::from_millis(5), 200)))
            }
        });
        let svc = start(cfg(), h);
        let mut status = svc.status();
        let mut events = svc.events();
        let _first = tokio::net::TcpStream::connect(svc.local_addr().unwrap())
            .await
            .unwrap();
        wait_status(&mut status, |s| {
            *s == HostStatus::Error(ServiceError::Source("no gpu".into()))
        })
        .await;
        loop {
            if let HostEvent::SourceFailed(e) = timeout(WAIT, events.recv()).await.unwrap().unwrap()
            {
                assert_eq!(e, ServiceError::Source("no gpu".into()));
                break;
            }
        }
        fail.store(false, std::sync::atomic::Ordering::SeqCst);
        let _second = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        svc.stop().await;
    }

    #[tokio::test]
    async fn adb_devices_get_a_reverse_to_the_bound_port_and_it_is_removed_on_stop() {
        let fake = Arc::new(FakeAdb::with_app(vec![
            device("R1", AdbState::Device),
            device("U1", AdbState::Unauthorized),
            device("10.0.0.9:5555", AdbState::Device),
        ]));
        let mut h = hooks();
        let f = fake.clone();
        h.locate_adb = Arc::new(move || Some(f.clone() as Arc<dyn AdbClient>));
        let svc = start(
            ServiceConfig {
                adb: AdbMode::Auto,
                ..cfg()
            },
            h,
        );
        let port = svc.local_addr().unwrap().port();
        let mut status = svc.status();
        let s = wait_status(
            &mut status,
            |s| matches!(s, HostStatus::Waiting { devices } if devices.len() == 2),
        )
        .await;
        let HostStatus::Waiting { devices } = s else {
            unreachable!()
        };
        assert_eq!(devices[0].serial, "R1");
        assert_eq!(devices[0].state, crate::status::DeviceState::Ready);
        assert_eq!(devices[1].state, crate::status::DeviceState::Unauthorized);
        let calls = fake.calls();
        assert!(
            calls.contains(&format!("reverse R1 27183 {port}")),
            "{calls:?}"
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.contains("10.0.0.9") || c.contains("U1"))
        );
        fake.take_calls();
        svc.stop().await;
        assert_eq!(fake.calls(), ["remove_reverse R1 27183"]);
    }

    #[tokio::test]
    async fn missing_adb_is_an_error_status_but_clients_are_still_served() {
        let svc = start(
            ServiceConfig {
                adb: AdbMode::Auto,
                ..cfg()
            },
            hooks(),
        );
        let mut status = svc.status();
        wait_status(&mut status, |s| {
            *s == HostStatus::Error(ServiceError::AdbNotFound)
        })
        .await;
        let (_tx, _rx) = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        svc.stop().await;
    }
}
