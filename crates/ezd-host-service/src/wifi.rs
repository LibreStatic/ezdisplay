//! The Wi-Fi side of the service: the QUIC accept loop, pairing and mDNS.
//!
//! Every incoming connection gets its own task, so a slow handshake or a pairing in progress
//! never delays the next client. Only the streaming session is exclusive, across both
//! transports (see `Inner::slot`).

use crate::config::WifiConfig;
use crate::error::ServiceError;
use crate::mdns::Advertiser;
use crate::service::{Inner, finish_session, lock};
use crate::status::{HostEvent, PairingCloseReason, RefusedWhy, Transport, WifiInfo};
use crate::store::{PairedStore, default_config_dir};
use ezd_link::identity::{Identity, fingerprint_hex, fingerprint_short};
use ezd_link::pair::{CloseReason, PairUri, PairingWindow, WindowState, clean_name, host_pair};
use ezd_link::quic::{Congestion, QuicIncoming, QuicServer};
use ezd_proto::pair::{Fingerprint, Refusal};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;

/// Most connections handled at once (handshaking, pairing or streaming); the rest are refused.
const MAX_CONNECTIONS: usize = 8;
/// Time a client has to complete the handshake and say what it wants.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything the Wi-Fi tasks share.
pub(crate) struct Wifi {
    server: QuicServer,
    store: PairedStore,
    window: PairingWindow,
    /// Wakes the pairing watcher after the window changed.
    changed: Notify,
    /// The window state last reported as an event: its PIN and deadline.
    announced: Mutex<Option<(String, Instant)>>,
    fingerprint: Fingerprint,
    host_name: String,
    bound: SocketAddr,
    /// Address written into the pairing URI.
    advertised: SocketAddr,
    ttl: Duration,
}

impl Wifi {
    /// Loads the identity and the paired devices, binds the UDP socket and decides whether the
    /// pairing window starts open.
    pub(crate) fn setup(
        handle: &tokio::runtime::Handle,
        cfg: &WifiConfig,
        ttl: Duration,
    ) -> Result<Arc<Wifi>, ServiceError> {
        let dir = cfg
            .config_dir
            .clone()
            .or_else(default_config_dir)
            .ok_or_else(|| ServiceError::Wifi("no config directory".into()))?;
        let identity = Identity::load_or_create(&dir)
            .map_err(|e| ServiceError::Wifi(format!("{}: {e}", dir.display())))?;
        let store = PairedStore::new(&dir);
        let paired = store
            .list()
            .map_err(|e| ServiceError::Wifi(e.to_string()))?;
        let server = {
            let _enter = handle.enter();
            QuicServer::bind(cfg.listen, &identity, congestion(cfg))
        }
        .map_err(|e| ServiceError::Bind {
            addr: cfg.listen,
            reason: e.to_string(),
        })?;
        let bound = server.local_addr().map_err(|e| ServiceError::Bind {
            addr: cfg.listen,
            reason: e.to_string(),
        })?;
        let window = PairingWindow::default();
        if paired.is_empty() || cfg.pair_on_start {
            window.open(ttl);
        }
        Ok(Arc::new(Wifi {
            server,
            store,
            window,
            changed: Notify::new(),
            announced: Mutex::new(None),
            fingerprint: identity.fingerprint(),
            host_name: host_name(),
            bound,
            advertised: pick_lan_addr(bound, outbound_ip(bound)),
            ttl,
        }))
    }

    pub(crate) fn info(&self) -> WifiInfo {
        WifiInfo {
            addr: self.bound,
            fingerprint: fingerprint_hex(&self.fingerprint),
            fingerprint_short: fingerprint_short(&self.fingerprint),
            host_name: self.host_name.clone(),
            pairing_open: self.window.is_open(),
        }
    }

    pub(crate) fn open_pairing(&self) -> String {
        let pin = self.window.open(self.ttl);
        self.changed.notify_one();
        pin
    }

    pub(crate) fn refresh_pairing(&self) {
        *lock(&self.announced) = None;
        self.changed.notify_one();
    }

    pub(crate) fn close_pairing(&self) {
        self.window.close();
        self.changed.notify_one();
    }

    /// Reports the window's changes since the last call as events.
    fn sync(&self, inner: &Inner) {
        let state = self.window.state();
        let mut last = lock(&self.announced);
        match state {
            WindowState::Open { pin, expires, .. } => {
                if last.as_ref() == Some(&(pin.clone(), expires)) {
                    return;
                }
                *last = Some((pin.clone(), expires));
                let uri = PairUri {
                    addr: self.advertised,
                    fingerprint: self.fingerprint,
                    pin: pin.clone(),
                    name: self.host_name.clone(),
                }
                .to_uri();
                inner.wifi_info.send_modify(|i| {
                    if let Some(i) = i {
                        i.pairing_open = true;
                    }
                });
                inner.emit(HostEvent::PairingOpen {
                    pin,
                    uri,
                    fingerprint: fingerprint_short(&self.fingerprint),
                    expires,
                });
            }
            WindowState::Closed(reason) => {
                if last.take().is_none() {
                    return;
                }
                inner.wifi_info.send_modify(|i| {
                    if let Some(i) = i {
                        i.pairing_open = false;
                    }
                });
                inner.emit(HostEvent::PairingClosed {
                    reason: match reason {
                        CloseReason::NotOpen => PairingCloseReason::Closed,
                        CloseReason::Paired => PairingCloseReason::Paired,
                        CloseReason::TooManyAttempts => PairingCloseReason::TooManyAttempts,
                        CloseReason::Expired => PairingCloseReason::Expired,
                    },
                });
            }
        }
    }
}

fn congestion(cfg: &WifiConfig) -> Congestion {
    cfg.congestion.into()
}

/// The machine's name as shown to tablets.
fn host_name() -> String {
    let name = clean_name(&gethostname::gethostname().to_string_lossy());
    if name.is_empty() {
        "ezdisplay".into()
    } else {
        name
    }
}

/// The address the machine uses to reach the LAN, found by "connecting" a UDP socket: the
/// kernel picks the route and the source address, and nothing is sent.
fn outbound_ip(bound: SocketAddr) -> Option<IpAddr> {
    if !bound.ip().is_unspecified() {
        return None;
    }
    let (local, probe): (SocketAddr, SocketAddr) = if bound.is_ipv4() {
        (
            (std::net::Ipv4Addr::UNSPECIFIED, 0).into(),
            ([192, 0, 2, 1], 9).into(),
        )
    } else {
        (
            (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
            "[2001:db8::1]:9".parse().ok()?,
        )
    };
    let sock = UdpSocket::bind(local).ok()?;
    sock.connect(probe).ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// The address to put in the pairing URI: a bound specific address as is; for an unspecified
/// one the LAN address (`outbound`), or the bound address when there is none.
fn pick_lan_addr(bound: SocketAddr, outbound: Option<IpAddr>) -> SocketAddr {
    match outbound {
        Some(ip) if bound.ip().is_unspecified() && ip.is_ipv4() == bound.is_ipv4() => {
            SocketAddr::new(ip, bound.port())
        }
        _ => bound,
    }
}

async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

/// Reports pairing window changes, and its expiry on time.
pub(crate) async fn watch_pairing(inner: Arc<Inner>, wifi: Arc<Wifi>) {
    let mut stop = inner.stop.subscribe();
    loop {
        if *stop.borrow() {
            break;
        }
        wifi.sync(&inner);
        let wait = match wifi.window.state() {
            WindowState::Open { expires, .. } => {
                expires.saturating_duration_since(Instant::now()) + Duration::from_millis(5)
            }
            WindowState::Closed(_) => Duration::from_secs(3600),
        };
        tokio::select! {
            () = wifi.changed.notified() => {}
            () = tokio::time::sleep(wait) => {}
            _ = stop.changed() => {}
        }
    }
}

/// Accepts Wi-Fi clients until the service stops; waits for the sessions it started.
pub(crate) async fn serve(inner: Arc<Inner>, wifi: Arc<Wifi>) {
    let mut stop = inner.stop.subscribe();
    let advertiser = match Advertiser::start(
        wifi.bound.port(),
        &fingerprint_short(&wifi.fingerprint),
        &wifi.host_name,
    ) {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::warn!("mDNS advertisement not started: {e}");
            None
        }
    };
    tracing::info!(addr = %wifi.bound, "Wi-Fi listening");
    let mut tasks = JoinSet::new();
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            incoming = wifi.server.accept_incoming() => {
                let Some(incoming) = incoming else { break };
                if tasks.len() >= MAX_CONNECTIONS {
                    tracing::warn!(peer = %incoming.remote_address(), "too many connections, refusing");
                    incoming.refuse();
                    continue;
                }
                tasks.spawn(handle_incoming(inner.clone(), wifi.clone(), incoming, inner.stop.subscribe()));
            }
            Some(done) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(e) = done {
                    tracing::error!("Wi-Fi connection task failed: {e}");
                }
            }
            _ = stop.changed() => break,
        }
    }
    // Sessions end on their own (they got the stop signal and say Bye); the server stays up
    // until they are done so that the Bye is delivered.
    while let Some(done) = tasks.join_next().await {
        if let Err(e) = done {
            tracing::error!("Wi-Fi connection task failed: {e}");
        }
    }
    // Closing the endpoint discards what is still in flight, such as the last Bye.
    wifi.server.wait_idle(Duration::from_secs(2)).await;
    wifi.server.close();
    drop(advertiser);
}

/// One client connection: handshake, then pairing or a session.
async fn handle_incoming(
    inner: Arc<Inner>,
    wifi: Arc<Wifi>,
    incoming: QuicIncoming,
    mut stop: watch::Receiver<bool>,
) {
    let peer = incoming.remote_address();
    let open = async {
        let conn = incoming.handshake().await?;
        conn.accept_open().await
    };
    let opened = tokio::select! {
        r = tokio::time::timeout(OPEN_TIMEOUT, open) => match r {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                tracing::debug!(%peer, "connection dropped before its request: {e}");
                return;
            }
            Err(_) => {
                tracing::debug!(%peer, "connection timed out before its request");
                return;
            }
        },
        () = stopped(&mut stop) => return,
    };
    match opened.open {
        ezd_proto::pair::LinkOpen::Session => session(&inner, &wifi, opened, peer, stop).await,
        ezd_proto::pair::LinkOpen::Pair { .. } => pairing(&inner, &wifi, opened, peer, stop).await,
    }
}

async fn pairing(
    inner: &Inner,
    wifi: &Wifi,
    opened: ezd_link::quic::Opened,
    peer: SocketAddr,
    mut stop: watch::Receiver<bool>,
) {
    let result = tokio::select! {
        r = host_pair(opened, &wifi.window, &wifi.host_name, |c| {
            wifi.store.add(c).map_err(std::io::Error::other)
        }) => r,
        () = stopped(&mut stop) => return,
    };
    wifi.changed.notify_one();
    match result {
        Ok(client) => {
            tracing::info!(%peer, device = %client.name, "device paired");
            inner.emit(HostEvent::Paired {
                device: client.name,
            });
        }
        Err(e) => {
            tracing::info!(%peer, "pairing failed: {e}");
            inner.emit(HostEvent::PairingFailed {
                peer,
                reason: e.to_string(),
            });
        }
    }
}

async fn session(
    inner: &Inner,
    wifi: &Wifi,
    opened: ezd_link::quic::Opened,
    peer: SocketAddr,
    mut stop: watch::Receiver<bool>,
) {
    let refuse = |why: RefusedWhy, opened: ezd_link::quic::Opened| async move {
        tracing::info!(%peer, ?why, "Wi-Fi session refused");
        inner.emit(HostEvent::SessionRefused { peer, why });
        opened
            .refuse(match why {
                RefusedWhy::NotPaired => Refusal::NotPaired,
                RefusedWhy::Busy => Refusal::Busy,
            })
            .await;
    };
    let paired = match opened.peer_fingerprint() {
        Some(fp) => match wifi.store.is_paired(&fp) {
            Ok(p) => p.then_some(fp),
            Err(e) => {
                tracing::warn!("cannot read the paired devices: {e}");
                None
            }
        },
        None => None,
    };
    let Some(fp) = paired else {
        return refuse(RefusedWhy::NotPaired, opened).await;
    };
    let Ok(permit) = inner.slot.clone().try_acquire_owned() else {
        return refuse(RefusedWhy::Busy, opened).await;
    };
    if let Err(e) = wifi.store.touch(&fp) {
        tracing::warn!("cannot record the last use: {e}");
    }
    tracing::info!(%peer, "Wi-Fi client connected");
    inner.emit(HostEvent::ClientConnected(peer));
    let cfg = inner.cfg.borrow().clone();
    // The source first: the client only gets its answer once the host can stream.
    let source = match (inner.hooks.create_source)(&cfg) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("cannot create virtual source: {e}");
            inner.emit(HostEvent::SourceFailed(e.clone()));
            inner.update(|st| st.phase = crate::service::Phase::Error(e));
            // The client retries; "busy" is the closest answer the protocol has.
            opened.refuse(Refusal::Busy).await;
            return;
        }
    };
    let link = match opened.accept_session().await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(%peer, "cannot start the Wi-Fi session: {e}");
            return;
        }
    };
    finish_session(inner, &cfg, link, Transport::Wifi, source, &mut stop).await;
    drop(permit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AdbMode, ServiceConfig};
    use crate::service::tests::{cfg, handshake, hooks, wait_session_end, wait_status};
    use crate::service::{HostService, ServiceHooks};
    use crate::status::HostStatus;
    use crate::store::tests::TempDir;
    use ezd_link::identity::parse_fingerprint;
    use ezd_link::pair::{PairedClient, pair};
    use ezd_link::quic::{QuicError, connect};
    use ezd_proto::{Bye, Message};
    use tokio::sync::broadcast;
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(10);

    fn wifi_cfg(dir: &TempDir, pair_on_start: bool) -> ServiceConfig {
        ServiceConfig {
            adb: AdbMode::Off,
            wifi: Some(WifiConfig {
                listen: "127.0.0.1:0".parse().unwrap(),
                pair_on_start,
                config_dir: Some(dir.0.clone()),
                ..WifiConfig::default()
            }),
            ..cfg()
        }
    }

    fn start(cfg: ServiceConfig, hooks: ServiceHooks) -> HostService {
        HostService::start_with(&tokio::runtime::Handle::current(), cfg, hooks).unwrap()
    }

    fn host_fp(svc: &HostService) -> (SocketAddr, Fingerprint) {
        let info = svc.wifi().borrow().clone().unwrap();
        (info.addr, parse_fingerprint(&info.fingerprint).unwrap())
    }

    fn seed(dir: &TempDir, id: &Identity, name: &str) {
        PairedStore::new(&dir.0)
            .add(&PairedClient {
                fingerprint: id.fingerprint(),
                name: name.into(),
            })
            .unwrap();
    }

    async fn next_event<T>(
        ev: &mut broadcast::Receiver<HostEvent>,
        mut pick: impl FnMut(HostEvent) -> Option<T>,
    ) -> T {
        timeout(WAIT, async {
            loop {
                if let Some(v) = pick(ev.recv().await.unwrap()) {
                    return v;
                }
            }
        })
        .await
        .unwrap()
    }

    async fn session_link(
        addr: SocketAddr,
        id: &Identity,
        fp: Fingerprint,
    ) -> Result<ezd_link::Link, QuicError> {
        connect(addr, id, Some(fp), Congestion::Cubic)
            .await?
            .open_session()
            .await
    }

    #[tokio::test]
    async fn pairs_through_the_service_then_streams_without_a_pin() {
        let dir = TempDir::new("wifi-pair");
        let svc = start(wifi_cfg(&dir, false), hooks());
        let mut events = svc.events();
        let mut status = svc.status();
        let (addr, fp) = host_fp(&svc);
        // The store is empty: the window is open from the start.
        let (pin, uri) = next_event(&mut events, |e| match e {
            HostEvent::PairingOpen { pin, uri, .. } => Some((pin, uri)),
            _ => None,
        })
        .await;
        let parsed = PairUri::parse(&uri).unwrap();
        assert_eq!(
            (parsed.pin.as_str(), parsed.fingerprint),
            (pin.as_str(), fp)
        );

        let id = Identity::generate().unwrap();
        let conn = connect(addr, &id, None, Congestion::Cubic).await.unwrap();
        let host = pair(conn, &pin, "Tab S9").await.unwrap();
        assert_eq!(host.fingerprint, fp);
        let device = next_event(&mut events, |e| match e {
            HostEvent::Paired { device } => Some(device),
            _ => None,
        })
        .await;
        assert_eq!(device, "Tab S9");
        let reason = next_event(&mut events, |e| match e {
            HostEvent::PairingClosed { reason } => Some(reason),
            _ => None,
        })
        .await;
        assert_eq!(reason, PairingCloseReason::Paired);
        assert!(!svc.wifi().borrow().as_ref().unwrap().pairing_open);
        assert_eq!(PairedStore::new(&dir.0).list().unwrap()[0].name, "Tab S9");

        // A later connection needs no PIN.
        let link = session_link(addr, &id, fp).await.unwrap();
        let (tx, _rx) = handshake(link).await;
        let HostStatus::Streaming(live) =
            wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await
        else {
            unreachable!()
        };
        assert_eq!(
            (live.transport, live.device_name.as_str()),
            (Transport::Wifi, "tablet")
        );
        assert_eq!(live.stream.width, 1280);
        assert!(
            PairedStore::new(&dir.0).list().unwrap()[0]
                .last_seen
                .is_some()
        );
        tx.send_control(Message::Bye(Bye {
            reason: "done".into(),
        }))
        .unwrap();
        assert_eq!(
            wait_session_end(&mut events).await,
            ezd_host_core::EndReason::ClientBye("done".into())
        );
        wait_status(&mut status, |s| matches!(s, HostStatus::Waiting { .. })).await;
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn an_unpaired_client_is_refused_when_no_window_is_open() {
        let dir = TempDir::new("wifi-unpaired");
        seed(&dir, &Identity::generate().unwrap(), "someone else");
        let svc = start(wifi_cfg(&dir, false), hooks());
        let mut events = svc.events();
        let (addr, fp) = host_fp(&svc);
        assert!(!svc.wifi().borrow().as_ref().unwrap().pairing_open);
        let id = Identity::generate().unwrap();
        let err = session_link(addr, &id, fp).await.unwrap_err();
        assert!(
            matches!(err, QuicError::Refused(Refusal::NotPaired)),
            "{err:?}"
        );
        let why = next_event(&mut events, |e| match e {
            HostEvent::SessionRefused { why, .. } => Some(why),
            _ => None,
        })
        .await;
        assert_eq!(why, RefusedWhy::NotPaired);
        // Pairing is refused too, with the window closed.
        let conn = connect(addr, &id, None, Congestion::Cubic).await.unwrap();
        let err = pair(conn, "123456", "x").await.unwrap_err();
        assert!(
            matches!(err, QuicError::Refused(Refusal::PairingClosed)),
            "{err:?}"
        );
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn a_second_client_is_busy_while_one_streams() {
        let dir = TempDir::new("wifi-busy");
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        seed(&dir, &a, "A");
        seed(&dir, &b, "B");
        let svc = start(wifi_cfg(&dir, false), hooks());
        let mut events = svc.events();
        let mut status = svc.status();
        let (addr, fp) = host_fp(&svc);
        let (tx, _rx) = handshake(session_link(addr, &a, fp).await.unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;

        let err = session_link(addr, &b, fp).await.unwrap_err();
        assert!(matches!(err, QuicError::Refused(Refusal::Busy)), "{err:?}");
        let why = next_event(&mut events, |e| match e {
            HostEvent::SessionRefused { why, .. } => Some(why),
            _ => None,
        })
        .await;
        assert_eq!(why, RefusedWhy::Busy);
        // The first session is untouched.
        assert!(matches!(*status.borrow(), HostStatus::Streaming(_)));

        // A TCP client waits for the Wi-Fi session instead of starting a second one.
        let tcp = tokio::spawn(crate::service::tests::connect(svc.local_addr().unwrap()));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!tcp.is_finished());
        tx.send_control(Message::Bye(Bye {
            reason: "done".into(),
        }))
        .unwrap();
        let (_tcp_tx, _tcp_rx) = timeout(WAIT, tcp).await.unwrap().unwrap();
        let HostStatus::Streaming(live) = wait_status(
            &mut status,
            |s| matches!(s, HostStatus::Streaming(l) if l.transport == Transport::Tcp),
        )
        .await
        else {
            unreachable!()
        };
        assert_eq!(live.transport, Transport::Tcp);
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn pairing_can_be_opened_and_closed_and_expires() {
        let dir = TempDir::new("wifi-window");
        seed(&dir, &Identity::generate().unwrap(), "someone else");
        let svc = start(
            wifi_cfg(&dir, false),
            ServiceHooks {
                pairing_ttl: Duration::from_millis(300),
                ..hooks()
            },
        );
        let mut events = svc.events();
        let pin = svc.open_pairing().unwrap();
        let opened = next_event(&mut events, |e| match e {
            HostEvent::PairingOpen { pin, .. } => Some(pin),
            _ => None,
        })
        .await;
        assert_eq!(opened, pin);
        assert!(svc.wifi().borrow().as_ref().unwrap().pairing_open);
        // It closes by itself when the time runs out.
        let reason = next_event(&mut events, |e| match e {
            HostEvent::PairingClosed { reason } => Some(reason),
            _ => None,
        })
        .await;
        assert_eq!(reason, PairingCloseReason::Expired);

        svc.open_pairing().unwrap();
        next_event(&mut events, |e| {
            matches!(e, HostEvent::PairingOpen { .. }).then_some(())
        })
        .await;
        svc.close_pairing();
        let reason = next_event(&mut events, |e| match e {
            HostEvent::PairingClosed { reason } => Some(reason),
            _ => None,
        })
        .await;
        assert_eq!(reason, PairingCloseReason::Closed);
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn pair_on_start_opens_the_window_even_with_a_paired_device() {
        let dir = TempDir::new("wifi-pair-on-start");
        seed(&dir, &Identity::generate().unwrap(), "someone else");
        let svc = start(wifi_cfg(&dir, true), hooks());
        let wifi = svc.wifi();
        assert!(wifi.borrow().as_ref().unwrap().pairing_open);
        timeout(WAIT, svc.stop()).await.unwrap();
        assert!(wifi.borrow().is_none());
    }

    #[tokio::test]
    async fn a_corrupt_paired_file_stops_the_start() {
        let dir = TempDir::new("wifi-corrupt");
        std::fs::write(dir.0.join(crate::store::PAIRED_FILE), "garbage").unwrap();
        let r = HostService::start_with(
            &tokio::runtime::Handle::current(),
            wifi_cfg(&dir, false),
            hooks(),
        );
        assert!(matches!(r, Err(ServiceError::Wifi(m)) if m.contains("corrupt")));
        assert_eq!(
            std::fs::read_to_string(dir.0.join(crate::store::PAIRED_FILE)).unwrap(),
            "garbage"
        );
    }

    #[tokio::test]
    async fn stop_ends_a_wifi_session_with_bye() {
        let dir = TempDir::new("wifi-stop");
        let id = Identity::generate().unwrap();
        seed(&dir, &id, "A");
        let svc = start(wifi_cfg(&dir, false), hooks());
        let mut status = svc.status();
        let mut events = svc.events();
        let (addr, fp) = host_fp(&svc);
        let (_tx, mut rx) = handshake(session_link(addr, &id, fp).await.unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        timeout(WAIT, svc.stop()).await.unwrap();
        assert_eq!(
            wait_session_end(&mut events).await,
            ezd_host_core::EndReason::HostShutdown
        );
        // The connection outlives the session long enough to deliver the Bye.
        let bye = timeout(WAIT, async {
            while let Ok(Some(m)) = rx.recv().await {
                if matches!(m, Message::Bye(_)) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap();
        assert!(bye, "the client never got the Bye");
    }

    #[test]
    fn lan_address_replaces_an_unspecified_bind() {
        let any: SocketAddr = "0.0.0.0:27183".parse().unwrap();
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        assert_eq!(
            pick_lan_addr(any, Some(lan)),
            "192.168.1.20:27183".parse().unwrap()
        );
        // No route: keep what was bound.
        assert_eq!(pick_lan_addr(any, None), any);
        // A wrong family is not used.
        assert_eq!(pick_lan_addr(any, Some("fe80::1".parse().unwrap())), any);
        // A specific bind is never replaced.
        let loopback: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        assert_eq!(pick_lan_addr(loopback, Some(lan)), loopback);
        assert_eq!(outbound_ip(loopback), None);
    }

    #[test]
    fn the_outbound_probe_does_not_return_an_unspecified_address() {
        // Without a network the probe finds nothing, which is fine.
        if let Some(ip) = outbound_ip("0.0.0.0:1".parse().unwrap()) {
            assert!(!ip.is_unspecified() && ip.is_ipv4());
        }
    }
}
