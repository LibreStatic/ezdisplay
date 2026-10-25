//! The settings window as an eframe app, wired to the [`Controller`].

use super::view::SettingsView;
use super::{fonts, system_accent, theme};
use crate::controller::Controller;
use crate::icons;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Application id: Wayland app_id and X11 class, matching the desktop entry.
pub const APP_ID: &str = "com.librestatic.ezdisplay";

/// How often the live statistics are redrawn.
const REPAINT: Duration = Duration::from_millis(500);

/// How long the window waits for the system accent before its first frame, so it does not open in
/// the brand colour and switch a moment later. A slower read still applies when it finishes.
const FIRST_ACCENT_WAIT: Duration = Duration::from_millis(200);

struct SettingsApp<'a> {
    controller: &'a mut Controller,
    view: SettingsView,
    /// Last known window focus, to spot the moment it comes back.
    focused: bool,
    /// The system accent currently applied (`None`: the brand palettes).
    accent: Arc<Mutex<Option<egui::Color32>>>,
    /// Set while a background read of the system accent runs.
    reading: Arc<AtomicBool>,
    /// Whether the system accent may be applied; checked by the reader under the `accent` lock.
    enabled: Arc<AtomicBool>,
}

impl SettingsApp<'_> {
    /// Applies the "use the system accent" setting: reads the system colour, or restores the brand one.
    fn apply_accent_setting(&self, ctx: &egui::Context, on: bool) {
        if on {
            self.enabled.store(true, Ordering::Release);
            refresh_accent(ctx, &self.accent, &self.reading, &self.enabled);
        } else {
            let mut current = self.accent.lock().unwrap_or_else(|e| e.into_inner());
            self.enabled.store(false, Ordering::Release);
            theme::install_accent(ctx, None);
            *current = None;
            ctx.request_repaint();
        }
    }
}

/// Reads the system accent on a background thread (never on the UI thread) and applies it when it
/// differs from the one in use. Does nothing while another read is running.
fn refresh_accent(
    ctx: &egui::Context,
    accent: &Arc<Mutex<Option<egui::Color32>>>,
    reading: &Arc<AtomicBool>,
    enabled: &Arc<AtomicBool>,
) {
    if reading.swap(true, Ordering::AcqRel) {
        return;
    }
    let (ctx, accent, reading, enabled) = (
        ctx.clone(),
        accent.clone(),
        reading.clone(),
        enabled.clone(),
    );
    let spawned = std::thread::Builder::new().name("accent".into()).spawn({
        let reading = reading.clone();
        move || {
            let found = system_accent::read();
            match found {
                Some(c) => tracing::debug!("system accent colour: {}", c.to_hex()),
                None => tracing::debug!("no system accent colour available"),
            }
            {
                let mut current = accent.lock().unwrap_or_else(|e| e.into_inner());
                if enabled.load(Ordering::Acquire) && *current != found {
                    theme::install_accent(&ctx, found);
                    *current = found;
                    ctx.request_repaint();
                }
            }
            reading.store(false, Ordering::Release);
        }
    });
    if let Err(e) = spawned {
        tracing::debug!("cannot start the accent reader: {e}");
        reading.store(false, Ordering::Release);
    }
}

impl eframe::App for SettingsApp<'_> {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The tray keeps working while the window is open.
        self.controller.tick();
        if self.controller.is_quitting() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.controller.take_focus_request() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        let use_accent = self.view.snap.config.use_system_accent;
        if use_accent != self.enabled.load(Ordering::Acquire) {
            self.apply_accent_setting(ctx, use_accent);
        } else if use_accent && focused && !self.focused {
            refresh_accent(ctx, &self.accent, &self.reading, &self.enabled);
        }
        self.focused = focused;
        self.controller.refresh_snapshot(&mut self.view.snap);
        let snap = self.view.snap.clone();
        self.view.set_snapshot(snap);
        ctx.request_repaint_after(REPAINT);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let loader = self.controller.loader();
        self.view.show(ui, loader);
        let actions = self.view.take_actions();
        if !actions.is_empty() {
            self.controller.dispatch(actions);
        }
    }
}

/// Opens the settings window on the calling (main) thread and returns when it is closed.
pub fn run_settings(controller: &mut Controller) -> Result<(), eframe::Error> {
    controller.refresh_autostart();
    let snapshot = controller.snapshot();
    let waker = controller.waker().clone();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("ezdisplay")
        .with_app_id(APP_ID)
        .with_inner_size([820.0, 560.0])
        .with_min_inner_size([640.0, 420.0]);
    if let Some(icon) = icons::window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = eframe::run_native(
        "ezdisplay",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            fonts::install_fonts(&cc.egui_ctx);
            waker.set_context(Some(cc.egui_ctx.clone()));
            let accent = Arc::new(Mutex::new(None));
            let reading = Arc::new(AtomicBool::new(false));
            let enabled = Arc::new(AtomicBool::new(snapshot.config.use_system_accent));
            if enabled.load(Ordering::Acquire) {
                refresh_accent(&cc.egui_ctx, &accent, &reading, &enabled);
                let deadline = std::time::Instant::now() + FIRST_ACCENT_WAIT;
                while reading.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Ok(Box::new(SettingsApp {
                controller,
                view: SettingsView::new(snapshot),
                focused: false,
                accent,
                reading,
                enabled,
            }))
        }),
    );
    waker.set_context(None);
    result
}
