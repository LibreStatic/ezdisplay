//! The settings window as an eframe app, wired to the [`Controller`].

use super::view::SettingsView;
use super::{fonts, theme};
use crate::controller::Controller;
use crate::icons;
use eframe::egui;
use std::time::Duration;

/// Application id: Wayland app_id and X11 class, matching the desktop entry.
pub const APP_ID: &str = "com.librestatic.ezdisplay";

/// How often the live statistics are redrawn.
const REPAINT: Duration = Duration::from_millis(500);

struct SettingsApp<'a> {
    controller: &'a mut Controller,
    view: SettingsView,
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
            Ok(Box::new(SettingsApp {
                controller,
                view: SettingsView::new(snapshot),
            }))
        }),
    );
    waker.set_context(None);
    result
}
