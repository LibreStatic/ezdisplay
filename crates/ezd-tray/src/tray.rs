//! The tray icon and its menu: applies a [`MenuModel`] to the native items.

use crate::icons::{self, IconKind};
use crate::menu_model::{MenuModel, SCALES};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// What a click on a menu item means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Toggle,
    Scale(u16),
    Telemetry,
    Settings,
    Autostart,
    OpenLogs,
    Quit,
}

/// The tray icon with its menu items.
pub struct Tray {
    icon: TrayIcon,
    kind: IconKind,
    tooltip: String,
    last: Option<MenuModel>,
    status: MenuItem,
    toggle: MenuItem,
    scale: Submenu,
    scales: Vec<(u16, CheckMenuItem)>,
    telemetry: CheckMenuItem,
    settings: MenuItem,
    autostart: CheckMenuItem,
    logs: MenuItem,
    quit: MenuItem,
    // Keeps the menu alive: the StatusNotifier backend reads snapshots of it.
    _menu: Menu,
}

impl Tray {
    pub fn new(kind: IconKind, model: &MenuModel) -> Result<Tray, String> {
        let menu = Menu::new();
        let status = MenuItem::new(&model.status, false, None);
        let toggle = MenuItem::new(&model.toggle.label, true, None);
        let scale = Submenu::new(&model.scale_title, true);
        let scales: Vec<(u16, CheckMenuItem)> = SCALES
            .iter()
            .zip(&model.scales)
            .map(|(&p, s)| (p, CheckMenuItem::new(&s.label, true, s.checked, None)))
            .collect();
        for (_, item) in &scales {
            scale.append(item).map_err(|e| e.to_string())?;
        }
        let telemetry = CheckMenuItem::new(&model.telemetry.0, true, model.telemetry.1, None);
        let settings = MenuItem::new(&model.settings, true, None);
        let autostart = CheckMenuItem::new(&model.autostart.0, true, model.autostart.1, None);
        let logs = MenuItem::new(&model.logs, true, None);
        let quit = MenuItem::new(&model.quit, true, None);
        let sep = PredefinedMenuItem::separator;
        menu.append_items(&[
            &status,
            &sep(),
            &toggle,
            &scale,
            &telemetry,
            &sep(),
            &settings,
            &autostart,
            &logs,
            &sep(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let icon = icons::tray_icon(kind)?;
        let tray = TrayIconBuilder::new()
            .with_id("ezdisplay")
            .with_title("ezdisplay")
            .with_tooltip(&model.status)
            .with_icon(icon)
            .with_menu(Box::new(menu.clone()))
            // Left click opens the settings window; the menu is for the right button.
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Tray {
            icon: tray,
            kind,
            tooltip: model.status.clone(),
            last: Some(model.clone()),
            status,
            toggle,
            scale,
            scales,
            telemetry,
            settings,
            autostart,
            logs,
            quit,
            _menu: menu,
        })
    }

    /// What a menu click on `id` means.
    pub fn action(&self, id: &MenuId) -> Option<MenuAction> {
        let id = id.clone();
        if id == *self.toggle.id() {
            Some(MenuAction::Toggle)
        } else if id == *self.telemetry.id() {
            Some(MenuAction::Telemetry)
        } else if id == *self.settings.id() {
            Some(MenuAction::Settings)
        } else if id == *self.autostart.id() {
            Some(MenuAction::Autostart)
        } else if id == *self.logs.id() {
            Some(MenuAction::OpenLogs)
        } else if id == *self.quit.id() {
            Some(MenuAction::Quit)
        } else {
            self.scales
                .iter()
                .find(|(_, item)| *item.id() == id)
                .map(|(p, _)| MenuAction::Scale(*p))
        }
    }

    /// Forces the next [`Tray::update`] to rewrite every item. Needed after a click: the native
    /// check items flip themselves, which may differ from the model (a click on the checked
    /// scale would otherwise leave it unchecked).
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    /// Brings icon, tooltip and menu in line with the model. Setters run only for what changed:
    /// each one makes the StatusNotifier backend re-publish the menu over D-Bus.
    pub fn update(&mut self, kind: IconKind, model: &MenuModel) {
        if kind != self.kind {
            match icons::tray_icon(kind) {
                Ok(icon) => {
                    let _ = self.icon.set_icon(Some(icon));
                    self.kind = kind;
                }
                Err(e) => tracing::warn!("cannot load the {kind:?} icon: {e}"),
            }
        }
        if self.tooltip != model.status {
            let _ = self.icon.set_tooltip(Some(model.status.as_str()));
            self.tooltip = model.status.clone();
        }
        if self.last.as_ref() == Some(model) {
            return;
        }
        let old = self.last.take();
        let old = old.as_ref();
        if old.is_none_or(|o| o.status != model.status) {
            self.status.set_text(&model.status);
        }
        if old.is_none_or(|o| o.toggle != model.toggle) {
            self.toggle.set_text(&model.toggle.label);
        }
        if old.is_none_or(|o| o.scale_title != model.scale_title) {
            self.scale.set_text(&model.scale_title);
        }
        for (i, ((_, item), s)) in self.scales.iter().zip(&model.scales).enumerate() {
            let prev = old.and_then(|o| o.scales.get(i));
            if prev.is_none_or(|p| p.label != s.label) {
                item.set_text(&s.label);
            }
            if prev.is_none_or(|p| p.checked != s.checked) {
                item.set_checked(s.checked);
            }
        }
        if old.is_none_or(|o| o.telemetry.0 != model.telemetry.0) {
            self.telemetry.set_text(&model.telemetry.0);
        }
        if old.is_none_or(|o| o.telemetry.1 != model.telemetry.1) {
            self.telemetry.set_checked(model.telemetry.1);
        }
        if old.is_none_or(|o| o.settings != model.settings) {
            self.settings.set_text(&model.settings);
        }
        if old.is_none_or(|o| o.autostart.0 != model.autostart.0) {
            self.autostart.set_text(&model.autostart.0);
        }
        if old.is_none_or(|o| o.autostart.1 != model.autostart.1) {
            self.autostart.set_checked(model.autostart.1);
        }
        if old.is_none_or(|o| o.logs != model.logs) {
            self.logs.set_text(&model.logs);
        }
        if old.is_none_or(|o| o.quit != model.quit) {
            self.quit.set_text(&model.quit);
        }
        self.last = Some(model.clone());
    }
}

/// Routes menu clicks to `f` (called from the backend's thread).
pub fn set_menu_handler(f: impl Fn(MenuEvent) + Send + Sync + 'static) {
    MenuEvent::set_event_handler(Some(f));
}
