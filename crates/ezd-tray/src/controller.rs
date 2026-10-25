//! The brain of the app, living on the main thread: owns the configuration, the host service, the
//! tray and the event queue. [`Controller::tick`] is called by whichever loop is running (the
//! idle loop, or the settings window while it is open).

use crate::config::{self, TrayConfig};
use crate::icons::IconKind;
use crate::ipc;
use crate::menu_model::{MenuModel, MenuState, menu_model};
use crate::platform::{self, InputPermission};
use crate::state::{Lifecycle, Step};
use crate::tray::{MenuAction, Tray};
use crate::ui::view::{Action, InputAccess, InputFixState, RepairState, Snapshot};
use crate::{autostart, paths};
use ezd_host_service::{HostService, HostStatus};
use i18n_embed::fluent::FluentLanguageLoader;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

/// How long Quit waits for the service to finish (hardware encoders can take a while to tear
/// down) before the process exits anyway.
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Something the main loop has to look at.
pub enum Event {
    Tray(TrayIconEvent),
    Menu(tray_icon::menu::MenuEvent),
    Ipc(ipc::Request),
    /// The old service finished stopping.
    ServiceStopped,
}

/// Lets other threads wake whatever loop is running (the idle loop waits on the event channel;
/// the settings window waits for egui repaints).
#[derive(Clone, Default)]
pub struct Waker {
    ctx: Arc<Mutex<Option<eframe::egui::Context>>>,
}

impl Waker {
    pub fn set_context(&self, ctx: Option<eframe::egui::Context>) {
        *self.ctx.lock().unwrap_or_else(|e| e.into_inner()) = ctx;
    }

    pub fn wake(&self) {
        if let Some(ctx) = &*self.ctx.lock().unwrap_or_else(|e| e.into_inner()) {
            ctx.request_repaint();
        }
    }
}

/// Whether input devices can be created, and the outcome of the last attempt to grant it.
#[derive(Default)]
struct InputState {
    access: InputAccess,
    fix: InputFixState,
}

struct Quitting {
    since: Instant,
    waiters: Vec<Sender<()>>,
    timed_out: bool,
}

pub struct Controller {
    rt: tokio::runtime::Handle,
    loader: FluentLanguageLoader,
    config_path: PathBuf,
    config: TrayConfig,
    lifecycle: Lifecycle,
    service: Option<HostService>,
    status_rx: Option<watch::Receiver<HostStatus>>,
    status: HostStatus,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    waker: Waker,
    tray: Option<Tray>,
    autostart: bool,
    repair: Arc<Mutex<RepairState>>,
    input_probe: Arc<dyn InputPermission>,
    input: Arc<Mutex<InputState>>,
    want_settings: bool,
    focus_window: bool,
    quitting: Option<Quitting>,
    #[cfg_attr(not(feature = "wifi"), allow(dead_code))]
    wifi: wifi::WifiRuntime,
}

impl Controller {
    /// `config_path` is where changes are saved. Nothing is started yet.
    pub fn new(
        rt: tokio::runtime::Handle,
        loader: FluentLanguageLoader,
        config_path: PathBuf,
        config: TrayConfig,
        waker: Waker,
    ) -> Controller {
        let (tx, rx) = channel();
        Controller {
            rt,
            loader,
            config_path,
            config,
            lifecycle: Lifecycle::Stopped,
            service: None,
            status_rx: None,
            status: HostStatus::Stopped,
            tx,
            rx,
            waker,
            tray: None,
            autostart: autostart::is_enabled(),
            repair: Arc::new(Mutex::new(RepairState::Idle)),
            input_probe: Arc::new(platform::SystemInput),
            input: Arc::new(Mutex::new(InputState::default())),
            want_settings: false,
            focus_window: false,
            quitting: None,
            wifi: wifi::WifiRuntime::default(),
        }
    }

    /// Replaces the input permission probe (tests).
    #[cfg(test)]
    pub fn set_input_permission(&mut self, probe: Arc<dyn InputPermission>) {
        self.input_probe = probe;
    }

    /// A sender other threads (IPC, tray callbacks) use to reach this controller, and the waker
    /// they call afterwards.
    pub fn sender(&self) -> (Sender<Event>, Waker) {
        (self.tx.clone(), self.waker.clone())
    }

    pub fn loader(&self) -> &FluentLanguageLoader {
        &self.loader
    }

    pub fn waker(&self) -> &Waker {
        &self.waker
    }

    // ------------------------------------------------------------------ tray

    fn menu_state(&self) -> MenuState {
        MenuState {
            lifecycle: self.lifecycle,
            status: self.status.clone(),
            scale_permille: self.config.service.scale_permille,
            telemetry: self.config.service.telemetry,
            autostart: self.autostart,
        }
    }

    fn model(&self) -> MenuModel {
        menu_model(&self.loader, &self.menu_state())
    }

    /// Creates the tray icon. On Windows the calling thread must then run a message loop. If the
    /// tray cannot be created the settings window opens instead, so the app stays reachable.
    pub fn create_tray(&mut self) {
        let kind = IconKind::of(&self.status);
        match Tray::new(kind, &self.model()) {
            Ok(t) => self.tray = Some(t),
            Err(e) => {
                tracing::error!("cannot create the tray icon: {e}");
                self.want_settings = true;
            }
        }
    }

    fn refresh_tray(&mut self) {
        if self.tray.is_none() {
            return;
        }
        let kind = if matches!(self.lifecycle, Lifecycle::Stopping { .. }) {
            IconKind::Stopped
        } else {
            IconKind::of(&self.status)
        };
        let model = self.model();
        if let Some(t) = &mut self.tray {
            t.update(kind, &model);
        }
    }

    // --------------------------------------------------------------- service

    /// Requests a start (queued if the old service is still stopping).
    pub fn start(&mut self) {
        let step = self.lifecycle.request_start();
        self.run(step);
    }

    /// Requests a stop; cancels a queued start.
    pub fn stop(&mut self) {
        let step = self.lifecycle.request_stop();
        self.run(step);
    }

    fn run(&mut self, step: Step) {
        match step {
            Step::None => {}
            Step::StartService => self.start_service(),
            Step::StopService => self.stop_service(),
        }
    }

    fn start_service(&mut self) {
        match HostService::start(&self.rt, self.config.service.clone()) {
            Ok(svc) => {
                tracing::info!("service started on {:?}", svc.local_addr());
                let rx = svc.status();
                self.status = rx.borrow().clone();
                self.status_rx = Some(rx);
                self.service = Some(svc);
                self.wifi_started();
            }
            Err(e) => {
                tracing::warn!("cannot start the service: {e}");
                self.lifecycle.start_failed();
                self.status = HostStatus::Error(e);
            }
        }
    }

    fn stop_service(&mut self) {
        self.status_rx = None;
        self.status = HostStatus::Stopped;
        self.wifi_stopped();
        let Some(svc) = self.service.take() else {
            // Nothing to wait for.
            let _ = self.tx.send(Event::ServiceStopped);
            return;
        };
        tracing::info!("stopping the service");
        let (tx, waker) = self.sender();
        self.rt.spawn(async move {
            svc.stop().await;
            let _ = tx.send(Event::ServiceStopped);
            waker.wake();
        });
    }

    /// Saves `config`, passes it to the service and refreshes the menu. With `restart_session`
    /// the running session ends and the tablet reconnects with the new settings.
    pub fn set_config(&mut self, config: TrayConfig, restart_session: bool) {
        if config == self.config && !restart_session {
            return;
        }
        let service_changed = config.service != self.config.service;
        self.config = config;
        if let Err(e) = config::save(&self.config_path, &self.config) {
            tracing::error!("cannot save {}: {e}", self.config_path.display());
        }
        if let Some(svc) = &self.service
            && (service_changed || restart_session)
        {
            svc.apply(self.config.service.clone(), restart_session);
        }
    }

    // ------------------------------------------------------------------ misc

    pub fn config(&self) -> &TrayConfig {
        &self.config
    }

    #[cfg(test)]
    pub fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    #[cfg(test)]
    pub fn status(&self) -> &HostStatus {
        &self.status
    }

    pub fn set_autostart(&mut self, on: bool) {
        match autostart::set(on, self.config.run_elevated) {
            Ok(()) => tracing::info!("autostart {}", if on { "enabled" } else { "disabled" }),
            Err(e) => tracing::warn!("cannot change autostart: {e}"),
        }
        self.autostart = autostart::is_enabled();
    }

    /// Windows: saves the "run elevated" setting and gives the launcher task the matching run
    /// level (asks for permission when this process is not elevated). It applies on the next start.
    pub fn set_run_elevated(&mut self, on: bool) {
        if self.config.run_elevated == on {
            return;
        }
        match autostart::set_run_elevated(on) {
            Ok(()) => {
                tracing::info!("run elevated {}", if on { "enabled" } else { "disabled" });
                let mut config = self.config.clone();
                config.run_elevated = on;
                self.set_config(config, false);
            }
            Err(e) => tracing::warn!("cannot change the run level: {e}"),
        }
    }

    pub fn refresh_autostart(&mut self) {
        self.autostart = autostart::is_enabled();
    }

    /// Data for the settings window. Probes the input permission (once per call, so once per
    /// window opening).
    pub fn snapshot(&self) -> Snapshot {
        {
            let mut input = self.input.lock().unwrap_or_else(|e| e.into_inner());
            if input.fix != InputFixState::Running {
                input.access = self.input_probe.access();
                // A finished attempt is only reported while the window that started it is open.
                input.fix = InputFixState::Idle;
            }
        }
        let (input_access, input_fix) = self.input_state();
        Snapshot {
            lifecycle: self.lifecycle,
            status: self.status.clone(),
            config: self.config.clone(),
            autostart: self.autostart,
            adb: platform::adb_info(),
            vdd: platform::vdd_info(),
            repair: self
                .repair
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            input_access,
            input_fix,
            has_licenses_dir: paths::licenses_dir().is_some(),
            wifi: self.wifi_state(),
            usb_problem: self.usb_problem(),
            windows: cfg!(windows),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Cheap per-frame part of [`Controller::snapshot`]: leaves the file-system lookups as they
    /// were.
    pub fn refresh_snapshot(&self, snap: &mut Snapshot) {
        snap.lifecycle = self.lifecycle;
        snap.status = self.status.clone();
        snap.config = self.config.clone();
        snap.autostart = self.autostart;
        snap.wifi = self.wifi_state();
        snap.usb_problem = self.usb_problem();
        snap.repair = self
            .repair
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        (snap.input_access, snap.input_fix) = self.input_state();
    }

    /// Why USB accessory mode is unavailable, while the service runs.
    fn usb_problem(&self) -> Option<String> {
        self.service.as_ref()?.aoa_problem().borrow().clone()
    }

    fn input_state(&self) -> (InputAccess, InputFixState) {
        let input = self.input.lock().unwrap_or_else(|e| e.into_inner());
        (input.access, input.fix.clone())
    }

    /// Carries out what the settings window asked for.
    pub fn dispatch(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::SetConfig(mut c) => {
                    // Only `SetRunElevated` changes this, together with the launcher task.
                    c.run_elevated = self.config.run_elevated;
                    self.set_config(c, false);
                }
                Action::ApplyNow => {
                    let c = self.config.clone();
                    self.set_config(c, true);
                }
                Action::Start => self.start(),
                Action::Stop => self.stop(),
                Action::SetAutostart(on) => self.set_autostart(on),
                Action::SetRunElevated(on) => self.set_run_elevated(on),
                Action::OpenLogs => platform::open_path(&paths::log_dir()),
                Action::OpenLicenses => {
                    if let Some(d) = paths::licenses_dir() {
                        platform::open_path(&d);
                    }
                }
                Action::OpenUrl(u) => platform::open(&u),
                Action::Repair => self.repair_driver(),
                Action::EnableInput => self.enable_input(),
                Action::SetWifi(on) => self.set_wifi(on),
                Action::PairDevice => self.pair_device(),
                Action::ClosePairing => self.close_pairing(),
                Action::ForgetDevice(fp) => self.forget_device(&fp),
            }
        }
    }

    fn repair_driver(&mut self) {
        {
            let mut r = self.repair.lock().unwrap_or_else(|e| e.into_inner());
            if *r == RepairState::Running {
                return;
            }
            *r = RepairState::Running;
        }
        let state = self.repair.clone();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let result = platform::repair_driver();
            tracing::info!("driver repair finished: {result:?}");
            *state.lock().unwrap_or_else(|e| e.into_inner()) = result;
            waker.wake();
        });
    }

    fn enable_input(&mut self) {
        {
            let mut input = self.input.lock().unwrap_or_else(|e| e.into_inner());
            if input.fix == InputFixState::Running {
                return;
            }
            input.fix = InputFixState::Running;
        }
        let state = self.input.clone();
        let probe = self.input_probe.clone();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let result = probe.enable();
            tracing::info!("enabling input finished: {result:?}");
            let access = probe.access();
            let mut input = state.lock().unwrap_or_else(|e| e.into_inner());
            input.access = access;
            input.fix = result;
            drop(input);
            waker.wake();
        });
    }

    // ---------------------------------------------------------------- window

    /// Whether the settings window should open now (clears the request).
    pub fn take_open_request(&mut self) -> bool {
        std::mem::take(&mut self.want_settings)
    }

    /// Asks for the settings window (focuses it if it is open).
    pub fn request_settings(&mut self) {
        self.want_settings = true;
        self.focus_window = true;
    }

    /// Whether an open window should come to the front (clears the request).
    pub fn take_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.focus_window)
    }

    // ------------------------------------------------------------------ quit

    /// Starts shutting down: stops the service; [`Controller::should_exit`] turns true once it
    /// has finished.
    pub fn quit(&mut self, waiter: Option<Sender<()>>) {
        match &mut self.quitting {
            Some(q) => q.waiters.extend(waiter),
            None => {
                tracing::info!("quit requested");
                self.quitting = Some(Quitting {
                    since: Instant::now(),
                    waiters: waiter.into_iter().collect(),
                    timed_out: false,
                });
                self.stop();
            }
        }
    }

    pub fn is_quitting(&self) -> bool {
        self.quitting.is_some()
    }

    /// Quit was requested and the service is gone (or took too long).
    pub fn should_exit(&self) -> bool {
        self.quitting
            .as_ref()
            .is_some_and(|q| self.lifecycle == Lifecycle::Stopped || q.timed_out)
    }

    /// Removes the tray icon and tells the instances waiting for `--quit` that we are done. Call
    /// right before exiting.
    pub fn finish(&mut self) {
        self.tray = None;
        if let Some(q) = self.quitting.take() {
            for w in q.waiters {
                let _ = w.send(());
            }
        }
    }

    // ------------------------------------------------------------------ loop

    /// Processes everything pending.
    pub fn tick(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.handle(ev);
        }
        self.poll_status();
        self.poll_wifi();
        if let Some(q) = &mut self.quitting
            && !q.timed_out
            && q.since.elapsed() > QUIT_TIMEOUT
        {
            tracing::error!("the service did not stop within {QUIT_TIMEOUT:?}; exiting anyway");
            q.timed_out = true;
        }
        self.refresh_tray();
    }

    /// Waits up to `timeout` for the first event, then [`tick`](Self::tick)s.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn wait_and_tick(&mut self, timeout: Duration) {
        if let Ok(ev) = self.rx.recv_timeout(timeout) {
            self.handle(ev);
        }
        self.tick();
    }

    fn poll_status(&mut self) {
        if let Some(rx) = &mut self.status_rx
            && rx.has_changed().unwrap_or(false)
        {
            self.status = rx.borrow_and_update().clone();
        }
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::ServiceStopped => {
                let step = self.lifecycle.stopped();
                self.run(step);
            }
            Event::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => self.request_settings(),
            Event::Tray(_) => {}
            Event::Ipc(req) => match req.command {
                ipc::Command::Show => {
                    self.request_settings();
                    let _ = req.done.send(());
                }
                ipc::Command::Quit => self.quit(Some(req.done)),
            },
            Event::Menu(ev) => {
                let action = self.tray.as_ref().and_then(|t| t.action(&ev.id));
                if let Some(a) = action {
                    self.menu_action(a);
                }
                // The native check items flip themselves on click; resynchronize them.
                if let Some(t) = &mut self.tray {
                    t.invalidate();
                }
            }
        }
    }

    fn menu_action(&mut self, a: MenuAction) {
        match a {
            MenuAction::Toggle => {
                if self.lifecycle.wants_running() {
                    self.stop();
                } else {
                    self.start();
                }
            }
            MenuAction::Scale(p) => {
                let mut c = self.config.clone();
                c.service.scale_permille = p;
                // The scale is negotiated at the handshake: reconnect for it to take effect.
                self.set_config(c, true);
            }
            MenuAction::Telemetry => {
                let mut c = self.config.clone();
                c.service.telemetry = !c.service.telemetry;
                // Sent to the tablet at the handshake as well.
                self.set_config(c, true);
            }
            MenuAction::Settings => self.request_settings(),
            MenuAction::Autostart => {
                let on = !self.autostart;
                self.set_autostart(on);
            }
            MenuAction::OpenLogs => platform::open_path(&paths::log_dir()),
            MenuAction::Quit => self.quit(None),
        }
    }
}

/// Routes the tray and menu callbacks (run on the backend's thread) into the controller.
pub fn forward_tray_events(tx: Sender<Event>, waker: Waker) {
    {
        let (tx, waker) = (tx.clone(), waker.clone());
        TrayIconEvent::set_event_handler(Some(move |e| {
            let _ = tx.send(Event::Tray(e));
            waker.wake();
        }));
    }
    crate::tray::set_menu_handler(move |e| {
        let _ = tx.send(Event::Menu(e));
        waker.wake();
    });
}

mod wifi;

#[cfg(test)]
mod tests;
