//! The USB accessory acceptor: polls the USB devices, switches a connected tablet to Android
//! accessory mode and runs a session over its bulk endpoints.
//!
//! One loop, once per [`AoaHooks::poll_interval`], on the session slot (see
//! [`Inner::slot`](crate::service::Inner)): when the slot is busy the cycle is skipped, so nothing
//! is switched while a session streams. The permit is held from before a switch until the session
//! ends, so a TCP or Wi-Fi client waits instead of racing the accessory.
//!
//! * A device that exposes an MTP or adb interface is switched (the handshake), at most once: if
//!   the app never talks on the accessory, the device is declined for the rest of the process and
//!   reset, which gives a phone its file transfer back.
//! * A device already in accessory mode (left from an earlier session, or the host restarted) is
//!   opened once and kept open while it is silent; each cycle listens to it for a moment. Silence
//!   never declines it, since the app may simply not be running yet, and keeping the link means
//!   no bytes the app sends between two polls are lost.
//! * A declined device is tried again once polls have seen it off the bus for
//!   [`AoaHooks::declined_rearm`] (the user replugged it, maybe after installing the app).
//! * A device whose session ended normally stays in accessory mode and is picked up again by the
//!   next cycle when the app reconnects.
//!
//! All USB I/O runs on blocking threads behind [`UsbBackend`], which tests replace.

use crate::config::{AoaMode, ServiceConfig};
use crate::service::{Inner, stream_session};
use crate::status::Transport;
use ezd_link::Link;
use ezd_link::aoa::{Aoa, DeviceInfo, Mode};
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::sync::watch;

/// How often the USB devices are polled.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long a device we just switched gets to send its first bytes: Android may first ask the
/// user which app to open.
pub const SWITCHED_TIMEOUT: Duration = Duration::from_secs(30);
/// How long an accessory that was already there is listened to per poll.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long a declined device must be gone from the bus before it is tried again (a replug after
/// installing the app). The re-enumeration after our own reset takes far less.
pub const DECLINED_REARM: Duration = Duration::from_secs(10);
/// How long a device must have been on the bus before it is switched: a tablet that was just
/// plugged in ignores the handshake while it is still setting up its USB functions.
pub const SETTLE: Duration = Duration::from_secs(2);
/// Switch attempts before a device that keeps failing the handshake is declined.
pub const SWITCH_ATTEMPTS: u32 = 3;
/// Delay between two switch attempts on the same device.
pub const SWITCH_RETRY: Duration = Duration::from_secs(3);

/// The USB operations the acceptor needs. Every method blocks.
pub trait UsbBackend: Send + Sync + 'static {
    /// The accessory devices and the devices that may be switched.
    fn list(&self) -> io::Result<Vec<DeviceInfo>>;
    /// Switches `dev` to accessory mode and returns the accessory it turned into.
    fn switch(&self, dev: &DeviceInfo) -> io::Result<DeviceInfo>;
    /// Opens the accessory `dev`. Dropping the link releases it.
    fn open(&self, dev: &DeviceInfo) -> io::Result<Link>;
    /// Resets `dev`.
    fn reset(&self, dev: &DeviceInfo) -> io::Result<()>;
}

impl UsbBackend for Aoa {
    fn list(&self) -> io::Result<Vec<DeviceInfo>> {
        Aoa::list(self)
    }

    fn switch(&self, dev: &DeviceInfo) -> io::Result<DeviceInfo> {
        Aoa::switch(self, dev)
    }

    fn open(&self, dev: &DeviceInfo) -> io::Result<Link> {
        Aoa::open(self, dev)
    }

    fn reset(&self, dev: &DeviceInfo) -> io::Result<()> {
        Aoa::reset(self, dev)
    }
}

/// Creates the backend; `Err` carries why USB accessory mode is unavailable. Called again later
/// while it fails.
pub type UsbFactory = Arc<dyn Fn() -> Result<Arc<dyn UsbBackend>, String> + Send + Sync>;

/// The parts of the acceptor that tests replace.
#[derive(Clone)]
pub struct AoaHooks {
    /// Creates the USB backend (libusb by default).
    pub backend: UsbFactory,
    /// Delay between polls.
    pub poll_interval: Duration,
    /// How long a device we just switched may stay silent before it is given up on.
    pub switched_timeout: Duration,
    /// How long an accessory that was already present is listened to per poll.
    pub probe_timeout: Duration,
    /// How long a declined device must be absent from the bus before it is switched again.
    pub declined_rearm: Duration,
    /// How long a device must have been on the bus before it is switched.
    pub settle: Duration,
    /// Delay between two switch attempts on the same device.
    pub switch_retry: Duration,
}

impl Default for AoaHooks {
    fn default() -> Self {
        AoaHooks {
            backend: Arc::new(|| {
                Aoa::new()
                    .map(|a| Arc::new(a) as Arc<dyn UsbBackend>)
                    .map_err(|e| e.to_string())
            }),
            poll_interval: POLL_INTERVAL,
            switched_timeout: SWITCHED_TIMEOUT,
            probe_timeout: PROBE_TIMEOUT,
            declined_rearm: DECLINED_REARM,
            settle: SETTLE,
            switch_retry: SWITCH_RETRY,
        }
    }
}

/// What listening to an accessory ended with.
enum Heard {
    /// The app sent its first bytes.
    Data(Vec<u8>),
    /// Nothing arrived in time.
    Silent,
    /// The stream ended or broke: the device went away.
    Gone,
    /// The service is stopping.
    Stopped,
}

/// Accessories that are open and have not said anything yet, by `DeviceInfo::key`. Kept across
/// polls: dropping a link while a blocking read is in flight would discard the bytes it reads.
type Idle = HashMap<String, Link>;

/// Devices given up on, by `DeviceInfo::key`, with when a poll first found them gone (`None`
/// while they are on the bus). Only polls that saw the device missing count, so a long session or
/// AOA being off does not re-arm a phone that stayed plugged in.
type Declined = HashMap<String, Option<Instant>>;

/// Switch candidates on the bus, by `DeviceInfo::key`: when they showed up and how their switch
/// attempts went. Forgotten as soon as a poll misses them.
type Candidates = HashMap<String, Candidate>;

struct Candidate {
    /// When a poll first saw the device.
    seen: Instant,
    /// Failed switch attempts so far.
    failures: u32,
    /// No attempt before this.
    next_try: Instant,
}

/// Sets the reported USB problem; observers are only woken when it changes.
fn set_problem(inner: &Inner, problem: Option<String>) {
    inner.aoa_problem.send_if_modified(|cur| {
        let changed = *cur != problem;
        if changed {
            *cur = problem;
        }
        changed
    });
}

/// Runs the acceptor until the service stops. Idles while `cfg.aoa` is off.
pub(crate) async fn run(inner: Arc<Inner>) {
    let mut stop_rx = inner.stop.subscribe();
    let mut cfg_rx = inner.cfg.subscribe();
    let hooks = inner.hooks.aoa.clone();
    let mut backend: Option<Arc<dyn UsbBackend>> = None;
    // Devices (by `DeviceInfo::key` of the original device) that are never switched again.
    let mut declined = Declined::new();
    let mut idle = Idle::new();
    let mut candidates = Candidates::new();
    let mut reported: Option<String> = None;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let cfg = cfg_rx.borrow_and_update().clone();
        let mut wait = hooks.poll_interval;
        if cfg.aoa == AoaMode::Off {
            backend = None;
            idle.clear();
            reported = None;
            set_problem(&inner, None);
            tokio::select! {
                _ = cfg_rx.changed() => {}
                _ = stop_rx.changed() => {}
            }
            continue;
        }
        if backend.is_none() {
            let make = hooks.backend.clone();
            match tokio::task::spawn_blocking(move || make()).await {
                Ok(Ok(b)) => {
                    backend = Some(b);
                    reported = None;
                    set_problem(&inner, None);
                }
                Ok(Err(reason)) => {
                    if reported.as_ref() != Some(&reason) {
                        tracing::info!("USB accessory mode unavailable: {reason}");
                        reported = Some(reason.clone());
                    }
                    set_problem(&inner, Some(reason));
                    // Initializing again is not free: look less often.
                    wait = hooks.poll_interval * 10;
                }
                Err(e) => tracing::error!("USB backend setup failed: {e}"),
            }
        }
        if let Some(b) = &backend {
            // Without the permit a session is running (or about to): leave the devices alone.
            if let Ok(permit) = inner.slot.clone().try_acquire_owned() {
                let state = PollState {
                    declined: &mut declined,
                    idle: &mut idle,
                    candidates: &mut candidates,
                };
                cycle(&inner, &cfg, b, state, &mut stop_rx).await;
                drop(permit);
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = cfg_rx.changed() => {}
            _ = stop_rx.changed() => {}
        }
    }
}

/// One poll: listens to the accessories that are there, then switches one new device.
async fn cycle(
    inner: &Inner,
    cfg: &ServiceConfig,
    backend: &Arc<dyn UsbBackend>,
    state: PollState<'_>,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let PollState {
        declined,
        idle,
        candidates,
    } = state;
    let list = match blocking(backend, |b| b.list()).await {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("cannot list the USB devices: {e}");
            return;
        }
    };
    let hooks = &inner.hooks.aoa;

    // A declined device that has been gone long enough is a candidate again.
    let now = Instant::now();
    declined.retain(|key, gone_since| {
        if list.iter().any(|d| d.key() == *key) {
            // Back on the bus: a short absence (our own reset) is forgotten.
            let rearm = gone_since.is_some_and(|t| now.duration_since(t) >= hooks.declined_rearm);
            *gone_since = None;
            return !rearm;
        }
        let since = *gone_since.get_or_insert(now);
        now.duration_since(since) < hooks.declined_rearm
    });

    // Accessories that went away take their links with them.
    idle.retain(|key, _| {
        list.iter()
            .any(|d| d.mode == Mode::Accessory && d.key() == *key)
    });
    for acc in list.iter().filter(|d| d.mode == Mode::Accessory) {
        let key = acc.key();
        if !idle.contains_key(&key) {
            let target = acc.clone();
            match blocking(backend, move |b| b.open(&target)).await {
                Ok(link) => {
                    idle.insert(key.clone(), link);
                }
                Err(e) => {
                    tracing::debug!("cannot open the accessory {}: {e}", acc.describe());
                    continue;
                }
            }
        }
        let Some(link) = idle.get_mut(&key) else {
            continue;
        };
        match listen(link, hooks.probe_timeout, stop_rx).await {
            Heard::Data(first) => {
                if let Some(link) = idle.remove(&key) {
                    start_session(inner, cfg, backend, acc, link, first, stop_rx).await;
                }
                return;
            }
            Heard::Silent => {}
            Heard::Gone => {
                idle.remove(&key);
            }
            Heard::Stopped => return,
        }
    }
    candidates.retain(|key, _| list.iter().any(|d| d.key() == *key));
    for d in list.iter().filter(|d| d.mode != Mode::Accessory) {
        candidates.entry(d.key()).or_insert(Candidate {
            seen: now,
            failures: 0,
            next_try: now,
        });
    }
    let Some(dev) = list.iter().find(|d| {
        let key = d.key();
        d.mode != Mode::Accessory
            && !declined.contains_key(&key)
            && candidates
                .get(&key)
                .is_some_and(|c| now.duration_since(c.seen) >= hooks.settle && now >= c.next_try)
    }) else {
        return;
    };
    tracing::info!(
        "switching {} ({}) to accessory mode",
        dev.describe(),
        dev.mode
    );
    let target = dev.clone();
    let acc = match blocking(backend, move |b| b.switch(&target)).await {
        Ok(acc) => acc,
        Err(e) => {
            let c = candidates
                .get_mut(&dev.key())
                .expect("the device was just picked from the candidates");
            c.failures += 1;
            c.next_try = Instant::now() + hooks.switch_retry;
            // A device without accessory support never gets it; others may just not be ready.
            if e.kind() == io::ErrorKind::Unsupported || c.failures >= SWITCH_ATTEMPTS {
                tracing::warn!("cannot switch {} to accessory mode: {e}", dev.describe());
                declined.insert(dev.key(), None);
            } else {
                tracing::info!(
                    "cannot switch {} to accessory mode yet ({e}), trying again",
                    dev.describe()
                );
            }
            return;
        }
    };
    match open_and_listen(backend, &acc, hooks.switched_timeout, stop_rx).await {
        Ok((link, first)) => {
            start_session(inner, cfg, backend, &acc, link, first, stop_rx).await;
            return;
        }
        Err(Heard::Stopped) => return,
        Err(_) => {}
    }
    tracing::info!(
        "{} did not start the app in accessory mode: giving up on it and resetting it",
        dev.describe()
    );
    declined.insert(dev.key(), None);
    if let Err(e) = blocking(backend, move |b| b.reset(&acc)).await {
        tracing::warn!("cannot reset {}: {e}", dev.describe());
    }
}

/// What the acceptor remembers between polls.
struct PollState<'a> {
    declined: &'a mut Declined,
    idle: &'a mut Idle,
    candidates: &'a mut Candidates,
}

/// Opens the freshly switched accessory and waits up to `wait` for the app's first bytes. On
/// silence the link is dropped (the caller resets the device).
async fn open_and_listen(
    backend: &Arc<dyn UsbBackend>,
    acc: &DeviceInfo,
    wait: Duration,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<(Link, Vec<u8>), Heard> {
    let target = acc.clone();
    let mut link = match blocking(backend, move |b| b.open(&target)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("cannot open the accessory {}: {e}", acc.describe());
            return Err(Heard::Gone);
        }
    };
    match listen(&mut link, wait, stop_rx).await {
        Heard::Data(first) => Ok((link, first)),
        other => Err(other),
    }
}

/// Waits up to `wait` for the first bytes on `link`. Cancel-safe: on a timeout nothing was read,
/// so the link can be listened to again.
async fn listen(link: &mut Link, wait: Duration, stop_rx: &mut watch::Receiver<bool>) -> Heard {
    let mut first = vec![0u8; 4096];
    tokio::select! {
        r = tokio::time::timeout(wait, link.reader.read(&mut first)) => match r {
            Err(_elapsed) => Heard::Silent,
            Ok(Ok(n)) if n > 0 => {
                first.truncate(n);
                Heard::Data(first)
            }
            Ok(_) => Heard::Gone,
        },
        _ = stop_rx.changed() => Heard::Stopped,
    }
}

/// Runs the session with the bytes already read put back in front of the stream.
async fn start_session(
    inner: &Inner,
    cfg: &ServiceConfig,
    backend: &Arc<dyn UsbBackend>,
    acc: &DeviceInfo,
    mut link: Link,
    first: Vec<u8>,
    stop_rx: &mut watch::Receiver<bool>,
) {
    tracing::info!(peer = %link.peer, "USB accessory is talking");
    let inner_reader = std::mem::replace(&mut link.reader, Box::new(tokio::io::empty()));
    link.reader = Box::new(PrefixRead {
        prefix: first,
        pos: 0,
        inner: inner_reader,
    });
    if stream_session(inner, cfg, link, Transport::Aoa, stop_rx).await {
        // The app is still in a session with an earlier host process and cannot tell that it
        // ended (an accessory never reports the host closing it). A reset makes it see the cable
        // go away; the device then comes back in its normal mode and is switched again.
        tracing::info!(
            "{} is still in an old session: resetting it",
            acc.describe()
        );
        let target = acc.clone();
        if let Err(e) = blocking(backend, move |b| b.reset(&target)).await {
            tracing::warn!("cannot reset {}: {e}", acc.describe());
        }
    }
}

/// Runs `f` on a blocking thread.
async fn blocking<T: Send + 'static>(
    backend: &Arc<dyn UsbBackend>,
    f: impl FnOnce(&dyn UsbBackend) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let b = backend.clone();
    tokio::task::spawn_blocking(move || f(&*b))
        .await
        .map_err(io::Error::other)?
}

/// Yields `prefix` first, then the bytes of `inner`.
struct PrefixRead {
    prefix: Vec<u8>,
    pos: usize,
    inner: ezd_link::BoxRead,
}

impl AsyncRead for PrefixRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            buf.put_slice(&self.prefix[self.pos..self.pos + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AdbMode;
    use crate::service::tests::{
        WAIT, cfg, connect, handshake, hooks, start, wait_session_end, wait_status,
    };
    use crate::status::HostStatus;
    use ezd_proto::{Bye, Message};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;
    use tokio::time::timeout;

    fn mtp(serial: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            vid: 0x04e8,
            pid: 0x6860,
            bus: 1,
            address: 4,
            ports: vec![2],
            product: Some("Tab".into()),
            serial: serial.map(String::from),
            mode: Mode::Mtp,
        }
    }

    fn accessory_of(d: &DeviceInfo) -> DeviceInfo {
        DeviceInfo {
            vid: 0x18d1,
            pid: 0x2d00,
            address: d.address + 1,
            mode: Mode::Accessory,
            ..d.clone()
        }
    }

    /// A tablet on the bus: MTP until switched, then an accessory until reset.
    #[derive(Default)]
    struct FakeUsb {
        devices: Mutex<Vec<DeviceInfo>>,
        /// The tablet side of every opened accessory, oldest first.
        peers: Mutex<VecDeque<Link>>,
        calls: Mutex<Vec<String>>,
        fail_switch: Mutex<bool>,
    }

    impl FakeUsb {
        fn with(devices: Vec<DeviceInfo>) -> Arc<FakeUsb> {
            Arc::new(FakeUsb {
                devices: Mutex::new(devices),
                ..FakeUsb::default()
            })
        }

        fn calls(&self, prefix: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.starts_with(prefix))
                .count()
        }

        async fn peer(&self) -> Link {
            timeout(WAIT, async {
                loop {
                    if let Some(l) = self.peers.lock().unwrap().pop_front() {
                        return l;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the acceptor never opened an accessory")
        }

        async fn until(&self, what: &str, f: impl Fn(&FakeUsb) -> bool) {
            let t = Instant::now();
            while !f(self) {
                assert!(t.elapsed() < WAIT, "timed out waiting for {what}");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    impl UsbBackend for FakeUsb {
        fn list(&self) -> io::Result<Vec<DeviceInfo>> {
            self.calls.lock().unwrap().push("list".into());
            Ok(self.devices.lock().unwrap().clone())
        }

        fn switch(&self, dev: &DeviceInfo) -> io::Result<DeviceInfo> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("switch {}", dev.key()));
            if *self.fail_switch.lock().unwrap() {
                return Err(io::Error::other("not an accessory-capable device"));
            }
            let acc = accessory_of(dev);
            let mut devices = self.devices.lock().unwrap();
            devices.retain(|d| d.key() != dev.key());
            devices.push(acc.clone());
            Ok(acc)
        }

        fn open(&self, dev: &DeviceInfo) -> io::Result<Link> {
            self.calls.lock().unwrap().push(format!("open {}", dev.pid));
            let (host, tablet) = Link::duplex_pair(64 * 1024);
            // Kept alive so the host does not see the end of the stream.
            self.peers.lock().unwrap().push_back(tablet);
            Ok(host)
        }

        fn reset(&self, dev: &DeviceInfo) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("reset {}", dev.pid));
            // Back to MTP at the original address.
            let mut devices = self.devices.lock().unwrap();
            devices.retain(|d| d.mode != Mode::Accessory);
            devices.push(DeviceInfo {
                address: dev.address - 1,
                ..mtp(dev.serial.as_deref())
            });
            Ok(())
        }
    }

    fn aoa_hooks(fake: &Arc<FakeUsb>, switched: Duration, probe: Duration) -> crate::ServiceHooks {
        let mut h = hooks();
        let f = fake.clone();
        h.aoa = AoaHooks {
            backend: Arc::new(move || Ok(f.clone() as Arc<dyn UsbBackend>)),
            poll_interval: Duration::from_millis(20),
            switched_timeout: switched,
            probe_timeout: probe,
            declined_rearm: Duration::from_secs(10),
            settle: Duration::ZERO,
            switch_retry: Duration::from_millis(30),
        };
        h
    }

    fn aoa_cfg() -> ServiceConfig {
        ServiceConfig {
            aoa: AoaMode::Auto,
            adb: AdbMode::Off,
            ..cfg()
        }
    }

    #[tokio::test]
    async fn a_candidate_is_switched_and_its_first_bytes_start_the_session() {
        let fake = FakeUsb::with(vec![mtp(Some("R1"))]);
        let svc = start(
            aoa_cfg(),
            aoa_hooks(&fake, Duration::from_secs(10), Duration::from_millis(50)),
        );
        let mut status = svc.status();
        let mut events = svc.events();
        let tablet = fake.peer().await;
        // The hello is sent right away, before the host reads anything: it must not be lost.
        let (tx, _rx) = handshake(tablet).await;
        let HostStatus::Streaming(live) =
            wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await
        else {
            unreachable!()
        };
        assert_eq!(live.transport, Transport::Aoa);
        assert_eq!(live.device_name, "tablet");
        assert_eq!(fake.calls("switch"), 1);
        assert_eq!(fake.calls("reset"), 0);

        tx.send_control(Message::Bye(Bye {
            reason: "done".into(),
        }))
        .unwrap();
        wait_session_end(&mut events).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Waiting { .. })).await;
        // A device that ended normally stays in accessory mode and is not declined.
        assert_eq!(fake.calls("reset"), 0);
        assert_eq!(fake.calls("switch"), 1);
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn a_switched_device_that_stays_silent_is_declined_and_reset() {
        let fake = FakeUsb::with(vec![mtp(Some("R1"))]);
        let svc = start(
            aoa_cfg(),
            aoa_hooks(&fake, Duration::from_millis(100), Duration::from_millis(50)),
        );
        fake.until("the reset", |f| f.calls("reset") == 1).await;
        // The device is back as MTP and a candidate again, but it is never switched twice.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(fake.calls("switch"), 1);
        assert_eq!(fake.calls("reset"), 1);
        assert!(
            fake.devices
                .lock()
                .unwrap()
                .iter()
                .all(|d| d.mode == Mode::Mtp)
        );
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn a_failing_switch_is_retried_a_few_times_then_given_up() {
        let fake = FakeUsb::with(vec![mtp(None)]);
        *fake.fail_switch.lock().unwrap() = true;
        let svc = start(
            aoa_cfg(),
            aoa_hooks(&fake, Duration::from_millis(100), Duration::from_millis(50)),
        );
        let attempts = SWITCH_ATTEMPTS as usize;
        fake.until("the last attempt", |f| f.calls("switch") == attempts)
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(fake.calls("switch"), attempts);
        assert_eq!(fake.calls("reset"), 0);
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn a_busy_slot_means_no_switch() {
        let fake = FakeUsb::with(vec![]);
        let svc = start(
            aoa_cfg(),
            aoa_hooks(&fake, Duration::from_secs(10), Duration::from_millis(50)),
        );
        let mut status = svc.status();
        let mut events = svc.events();
        let (tx, _rx) = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;

        fake.devices.lock().unwrap().push(mtp(Some("R1")));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(fake.calls("switch"), 0);
        assert!(matches!(*status.borrow(), HostStatus::Streaming(_)));

        // Once the session is over the device is switched.
        tx.send_control(Message::Bye(Bye {
            reason: "done".into(),
        }))
        .unwrap();
        wait_session_end(&mut events).await;
        fake.until("the switch", |f| f.calls("switch") == 1).await;
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn an_accessory_that_was_already_there_is_never_declined() {
        let accessory = accessory_of(&mtp(Some("R1")));
        let fake = FakeUsb::with(vec![accessory]);
        let svc = start(
            aoa_cfg(),
            aoa_hooks(&fake, Duration::from_millis(30), Duration::from_millis(30)),
        );
        let mut status = svc.status();
        // The app stays silent for several polls: no switch, no reset, nothing declined, and the
        // accessory is opened once and kept.
        fake.until("a few probes", |f| f.calls("list") >= 5).await;
        assert_eq!(fake.calls("switch"), 0);
        assert_eq!(fake.calls("reset"), 0);
        assert_eq!(fake.calls("open"), 1);

        // The app starts later: its first bytes (sent after earlier probes timed out) arrive
        // intact and the session handshake succeeds on the same link.
        let tablet = fake.peer().await;
        let (_tx, _rx) = handshake(tablet).await;
        let HostStatus::Streaming(live) =
            wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await
        else {
            unreachable!()
        };
        assert_eq!(live.transport, Transport::Aoa);
        assert_eq!(fake.calls("reset"), 0);
        assert_eq!(fake.calls("open"), 1);
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn an_accessory_still_in_an_old_session_is_reset() {
        let fake = FakeUsb::with(vec![accessory_of(&mtp(Some("R1")))]);
        let svc = start(
            aoa_cfg(),
            aoa_hooks(&fake, Duration::from_millis(100), Duration::from_millis(50)),
        );
        // The app talks as if a session were running: no Hello, just its statistics.
        let tablet = fake.peer().await;
        let (tx, _rx, _t) = tablet.into_mux(ezd_proto::DEFAULT_CHUNK_LEN);
        tx.send_control(Message::ClientStats(ezd_proto::ClientStats {
            window_ms: 1000,
            frames_received: 0,
            frames_decoded: 0,
            frames_dropped: 0,
            decode_us_p50: 0,
            decode_us_p95: 0,
            e2e_us_p50: 0,
            e2e_us_p95: 0,
        }))
        .unwrap();
        fake.until("the reset", |f| f.calls("reset") == 1).await;
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn a_device_that_just_showed_up_is_left_to_settle() {
        let fake = FakeUsb::with(vec![mtp(Some("R1"))]);
        let mut h = aoa_hooks(&fake, Duration::from_millis(100), Duration::from_millis(50));
        h.aoa.settle = Duration::from_millis(400);
        let svc = start(aoa_cfg(), h);
        fake.until("a few polls", |f| f.calls("list") >= 3).await;
        assert_eq!(fake.calls("switch"), 0);
        fake.until("the switch", |f| f.calls("switch") == 1).await;
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    fn rearm_hooks(fake: &Arc<FakeUsb>, rearm: Duration) -> crate::ServiceHooks {
        let mut h = aoa_hooks(fake, Duration::from_millis(100), Duration::from_millis(50));
        h.aoa.declined_rearm = rearm;
        h
    }

    #[tokio::test]
    async fn a_declined_device_is_tried_again_after_being_gone_long_enough() {
        let fake = FakeUsb::with(vec![mtp(Some("R1"))]);
        let svc = start(aoa_cfg(), rearm_hooks(&fake, Duration::from_millis(300)));
        fake.until("the reset", |f| f.calls("reset") == 1).await;
        // Unplugged for longer than the re-arm time, then plugged in again.
        fake.devices.lock().unwrap().clear();
        tokio::time::sleep(Duration::from_millis(600)).await;
        fake.devices.lock().unwrap().push(mtp(Some("R1")));
        fake.until("the second switch", |f| f.calls("switch") == 2)
            .await;
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn the_re_enumeration_after_our_reset_does_not_re_arm_a_declined_device() {
        let fake = FakeUsb::with(vec![mtp(Some("R1"))]);
        let svc = start(aoa_cfg(), rearm_hooks(&fake, Duration::from_millis(400)));
        fake.until("the reset", |f| f.calls("reset") == 1).await;
        // Gone for a moment while it re-enumerates, shorter than the re-arm time.
        fake.devices.lock().unwrap().clear();
        tokio::time::sleep(Duration::from_millis(150)).await;
        fake.devices.lock().unwrap().push(mtp(Some("R1")));
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(fake.calls("switch"), 1);
        assert_eq!(fake.calls("reset"), 1);
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn aoa_off_leaves_the_usb_devices_alone() {
        let fake = FakeUsb::with(vec![mtp(Some("R1"))]);
        let svc = start(
            ServiceConfig {
                aoa: AoaMode::Off,
                ..aoa_cfg()
            },
            aoa_hooks(&fake, Duration::from_secs(10), Duration::from_millis(50)),
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(fake.calls.lock().unwrap().is_empty());

        // Turning it on at run time starts the acceptor.
        svc.apply(aoa_cfg(), false);
        fake.until("the switch", |f| f.calls("switch") == 1).await;
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn an_unavailable_backend_is_reported_and_retried() {
        let fake = FakeUsb::with(vec![]);
        let present = Arc::new(Mutex::new(false));
        let mut h = aoa_hooks(&fake, Duration::from_secs(10), Duration::from_millis(50));
        let (f, p) = (fake.clone(), present.clone());
        h.aoa.backend = Arc::new(move || {
            if *p.lock().unwrap() {
                Ok(f.clone() as Arc<dyn UsbBackend>)
            } else {
                Err("UsbDk is not installed".into())
            }
        });
        let svc = start(aoa_cfg(), h);
        let mut status = svc.status();
        let mut problem = svc.aoa_problem();
        timeout(
            WAIT,
            problem.wait_for(|p| p.as_deref() == Some("UsbDk is not installed")),
        )
        .await
        .unwrap()
        .unwrap();
        // Informational only: the host is not in an error state because of it.
        assert!(matches!(*status.borrow(), HostStatus::Waiting { .. }));
        // A TCP client is still served.
        let mut events = svc.events();
        let (tx, _rx) = connect(svc.local_addr().unwrap()).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Streaming(_))).await;
        tx.send_control(Message::Bye(Bye {
            reason: "done".into(),
        }))
        .unwrap();
        wait_session_end(&mut events).await;
        // USB shows up later (UsbDk gets installed): the acceptor notices and the error goes.
        *present.lock().unwrap() = true;
        fake.until("a poll", |f| f.calls("list") > 0).await;
        wait_status(&mut status, |s| matches!(s, HostStatus::Waiting { .. })).await;
        timeout(WAIT, problem.wait_for(|p| p.is_none()))
            .await
            .unwrap()
            .unwrap();
        timeout(WAIT, svc.stop()).await.unwrap();
    }

    #[tokio::test]
    async fn bytes_read_for_the_probe_come_back_first() {
        use tokio::io::AsyncReadExt;
        let (host, mut tablet) = Link::duplex_pair(1024);
        let mut reader = PrefixRead {
            prefix: b"abc".to_vec(),
            pos: 0,
            inner: host.reader,
        };
        tokio::io::AsyncWriteExt::write_all(&mut tablet.writer, b"def")
            .await
            .unwrap();
        let mut got = [0u8; 6];
        reader.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"abcdef");
    }
}
