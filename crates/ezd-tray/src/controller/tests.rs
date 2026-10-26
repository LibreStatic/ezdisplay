use super::*;
use crate::i18n::loader_for;
use ezd_host_service::{AdbMode, SourceKind};

struct Fixture {
    // Declared first so the controller (and its service) go before the runtime.
    ctl: Controller,
    _rt: tokio::runtime::Runtime,
    dir: tempfile::TempDir,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn fixture() -> Fixture {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut config = TrayConfig::default();
    config.service.source = SourceKind::Synthetic;
    config.service.adb = AdbMode::Off;
    config.service.listen = std::net::SocketAddr::from(([127, 0, 0, 1], free_port()));
    let ctl = Controller::new(
        rt.handle().clone(),
        loader_for(&["en".parse().unwrap()]),
        dir.path().join("config.toml"),
        config,
        Waker::default(),
    );
    Fixture { ctl, _rt: rt, dir }
}

fn wait_until(ctl: &mut Controller, what: &str, cond: impl Fn(&Controller) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond(ctl) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        ctl.wait_and_tick(Duration::from_millis(20));
    }
}

fn waiting(c: &Controller) -> bool {
    matches!(c.status(), HostStatus::Waiting { .. })
}

#[test]
fn start_and_stop() {
    let mut f = fixture();
    f.ctl.start();
    assert_eq!(f.ctl.lifecycle(), Lifecycle::Running);
    wait_until(&mut f.ctl, "waiting", waiting);
    f.ctl.stop();
    assert_eq!(f.ctl.lifecycle(), Lifecycle::Stopping { restart: false });
    wait_until(&mut f.ctl, "stopped", |c| {
        c.lifecycle() == Lifecycle::Stopped
    });
    assert_eq!(*f.ctl.status(), HostStatus::Stopped);
}

#[test]
fn start_while_stopping_runs_after_the_old_service_released_the_port() {
    let mut f = fixture();
    f.ctl.start();
    wait_until(&mut f.ctl, "waiting", waiting);
    f.ctl.stop();
    // Same fixed port: this only works if the new service starts after the old one is gone.
    f.ctl.start();
    assert_eq!(f.ctl.lifecycle(), Lifecycle::Stopping { restart: true });
    wait_until(&mut f.ctl, "running again", |c| {
        c.lifecycle() == Lifecycle::Running && waiting(c)
    });
    assert!(!matches!(f.ctl.status(), HostStatus::Error(_)));
    f.ctl.stop();
    wait_until(&mut f.ctl, "stopped", |c| {
        c.lifecycle() == Lifecycle::Stopped
    });
}

#[test]
fn bind_failure_is_shown_as_an_error() {
    let mut f = fixture();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut c = f.ctl.config().clone();
    c.service.listen = taken.local_addr().unwrap();
    f.ctl.set_config(c, false);
    f.ctl.start();
    assert_eq!(f.ctl.lifecycle(), Lifecycle::Stopped);
    assert!(matches!(
        f.ctl.status(),
        HostStatus::Error(ezd_host_service::ServiceError::Bind { .. })
    ));
}

#[test]
fn quit_waits_for_the_service_and_notifies_waiters() {
    let mut f = fixture();
    f.ctl.start();
    wait_until(&mut f.ctl, "waiting", waiting);
    let (tx, rx) = channel();
    f.ctl.quit(Some(tx));
    assert!(f.ctl.is_quitting() && !f.ctl.should_exit());
    wait_until(&mut f.ctl, "exit", |c| c.should_exit());
    // The waiter hears back only when the app finishes.
    assert!(rx.try_recv().is_err());
    f.ctl.finish();
    assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok());
}

#[test]
fn quit_without_a_service_exits_at_once() {
    let mut f = fixture();
    f.ctl.quit(None);
    assert!(f.ctl.should_exit());
}

#[test]
fn config_changes_are_saved() {
    let mut f = fixture();
    let mut c = f.ctl.config().clone();
    c.service.scale_permille = 750;
    f.ctl.set_config(c.clone(), false);
    let saved = config::load(&f.dir.path().join("config.toml"));
    assert!(saved.error.is_none());
    assert_eq!(saved.config.service.scale_permille, 750);
    assert_eq!(*f.ctl.config(), c);
}

#[test]
fn window_actions_reach_the_controller() {
    let mut f = fixture();
    f.ctl.dispatch(vec![Action::Start]);
    assert_eq!(f.ctl.lifecycle(), Lifecycle::Running);
    f.ctl.dispatch(vec![Action::Stop]);
    wait_until(&mut f.ctl, "stopped", |c| {
        c.lifecycle() == Lifecycle::Stopped
    });
}

#[test]
fn settings_requests_are_consumed_once() {
    let mut f = fixture();
    assert!(!f.ctl.take_open_request());
    f.ctl.request_settings();
    assert!(f.ctl.take_open_request());
    assert!(!f.ctl.take_open_request());
    assert!(f.ctl.take_focus_request());
}

/// Pretends to be the system: starts denied, and `enable` grants access.
struct FakeInput {
    granted: std::sync::atomic::AtomicBool,
    outcome: InputFixState,
    enables: std::sync::atomic::AtomicUsize,
}

impl FakeInput {
    fn new(outcome: InputFixState) -> Arc<FakeInput> {
        Arc::new(FakeInput {
            granted: false.into(),
            outcome,
            enables: 0.into(),
        })
    }
}

impl InputPermission for FakeInput {
    fn access(&self) -> InputAccess {
        if self.granted.load(std::sync::atomic::Ordering::SeqCst) {
            InputAccess::Ok
        } else {
            InputAccess::Denied
        }
    }

    fn enable(&self) -> InputFixState {
        self.enables
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.outcome == InputFixState::Done {
            self.granted
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.outcome.clone()
    }
}

fn wait_for_input_fix(ctl: &Controller, snap: &mut Snapshot, want: InputFixState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        ctl.refresh_snapshot(snap);
        if snap.input_fix == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the install"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn enabling_input_runs_off_thread_and_reprobes() {
    let mut f = fixture();
    let fake = FakeInput::new(InputFixState::Done);
    f.ctl.set_input_permission(fake.clone());
    let mut snap = f.ctl.snapshot();
    assert_eq!(snap.input_access, InputAccess::Denied);

    f.ctl.dispatch(vec![Action::EnableInput]);
    wait_for_input_fix(&f.ctl, &mut snap, InputFixState::Done);
    assert_eq!(snap.input_access, InputAccess::Ok);
    assert_eq!(fake.enables.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Reopening the window probes again and forgets the finished attempt.
    let snap = f.ctl.snapshot();
    assert_eq!(snap.input_access, InputAccess::Ok);
    assert_eq!(snap.input_fix, InputFixState::Idle);
}

#[test]
fn a_cancelled_enable_leaves_access_denied() {
    let mut f = fixture();
    f.ctl
        .set_input_permission(FakeInput::new(InputFixState::Cancelled));
    let mut snap = f.ctl.snapshot();
    f.ctl.dispatch(vec![Action::EnableInput]);
    wait_for_input_fix(&f.ctl, &mut snap, InputFixState::Cancelled);
    assert_eq!(snap.input_access, InputAccess::Denied);
}

#[cfg(feature = "wifi")]
mod wifi {
    use super::*;
    use ezd_host_service::WifiConfig;

    fn wifi_fixture() -> Fixture {
        let mut f = fixture();
        let mut c = f.ctl.config().clone();
        c.service.wifi = Some(WifiConfig {
            listen: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            config_dir: Some(f.dir.path().join("wifi")),
            ..WifiConfig::default()
        });
        f.ctl.set_config(c, false);
        f
    }

    #[test]
    fn the_card_follows_the_service_and_pairing_opens_and_closes() {
        let mut f = wifi_fixture();
        assert!(f.ctl.snapshot().wifi.info.is_none());
        f.ctl.start();
        wait_until(&mut f.ctl, "listener", |c| c.snapshot().wifi.info.is_some());
        let snap = f.ctl.snapshot();
        assert!(snap.wifi.supported && snap.wifi.enabled);
        // Nobody is paired yet, so the window opens by itself.
        wait_until(&mut f.ctl, "the first window", |c| {
            c.snapshot().wifi.pairing.is_some()
        });
        let offer = f.ctl.snapshot().wifi.pairing.unwrap();
        assert_eq!(offer.pin.len(), 6);
        assert!(offer.uri.starts_with("ezd://pair"));
        f.ctl.dispatch(vec![Action::ClosePairing]);
        wait_until(&mut f.ctl, "closed", |c| {
            c.snapshot().wifi.pairing.is_none()
        });
        f.ctl.dispatch(vec![Action::PairDevice]);
        wait_until(&mut f.ctl, "reopened", |c| {
            c.snapshot().wifi.pairing.is_some()
        });
        assert!(f.ctl.snapshot().wifi.paired.is_empty());
        // Forgetting an unknown device changes nothing.
        f.ctl.dispatch(vec![Action::ForgetDevice("abcd".into())]);
        assert!(f.ctl.snapshot().wifi.paired.is_empty());
    }

    #[test]
    fn switching_wifi_restarts_the_running_service_and_saves() {
        let mut f = fixture();
        f.ctl.start();
        wait_until(&mut f.ctl, "waiting", waiting);
        assert!(f.ctl.snapshot().wifi.info.is_none());
        // The default listener port may be taken on a developer machine; the switch itself is
        // what is tested, so use a free one.
        let mut c = f.ctl.config().clone();
        c.service.wifi = Some(WifiConfig {
            listen: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            config_dir: Some(f.dir.path().join("wifi")),
            ..WifiConfig::default()
        });
        f.ctl.set_config(c, false);
        f.ctl.dispatch(vec![Action::SetWifi(false)]);
        wait_until(&mut f.ctl, "restarted", waiting);
        assert!(f.ctl.config().service.wifi.is_none());
        assert!(!f.ctl.snapshot().wifi.enabled);
        let saved = config::load(&f.dir.path().join("config.toml")).config;
        assert!(saved.service.wifi.is_none());
    }
}
