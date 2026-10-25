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
