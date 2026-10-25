//! The settings window as a plain egui view: it reads a [`Snapshot`] of the app, edits a copy of
//! the configuration and records what the user asked for as [`Action`]s. It knows nothing about
//! the tray or the service, so tests render it with fake state.

use super::theme::{self, Palette};
use crate::config::TrayConfig;
use crate::menu_model;
use crate::state::Lifecycle;
use crate::text;
use eframe::egui::{self, ComboBox, RichText, Ui};
use ezd_host_service::{AdbMode, DeviceInfo, DeviceState, HostStatus, LiveSession, PenChoice};
use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed_fl::fl;
use std::path::PathBuf;

pub const REPOSITORY_URL: &str = "https://github.com/LibreStatic/ezdisplay";
pub const SAMSUNG_DRIVER_URL: &str = "https://developer.samsung.com/android-usb-driver";

/// Refresh rates offered, in hertz.
const REFRESH_HZ: [u32; 7] = [30, 60, 90, 120, 144, 165, 240];
/// Encoders that can be forced by name; the current one is kept even if it is not listed.
const ENCODERS: [&str; 6] = [
    "h264_nvenc",
    "h264_amf",
    "h264_qsv",
    "h264_vaapi",
    "h264_mf",
    "libx264",
];

/// Where adb comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdbInfo {
    pub path: Option<PathBuf>,
    /// The copy shipped next to the executable (otherwise the Android SDK or `PATH`).
    pub bundled: bool,
}

/// What the display driver page shows (Windows).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VddInfo {
    /// The driver answers.
    pub present: bool,
    /// ezdisplay installed it (so it may repair or remove it).
    pub owned: bool,
    pub driver_dir: Option<String>,
}

/// Outcome of the elevated repair.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RepairState {
    #[default]
    Idle,
    Running,
    /// Exit code of the installer.
    Finished(u32),
    /// The user declined the elevation prompt: nothing happened.
    Cancelled,
    Failed(String),
}

/// Exit code of `ezd-host vdd install` when Windows must restart to finish.
pub const EXIT_REBOOT_REQUIRED: u32 = 3010;

/// Whether the user may create input devices (Linux `/dev/uinput`); always `Ok` elsewhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputAccess {
    #[default]
    Ok,
    /// The device exists but this user may not open it.
    Denied,
    /// The kernel module is not loaded.
    Missing,
}

/// Outcome of "Enable input" (the privileged install of the udev rule).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputFixState {
    #[default]
    Idle,
    Running,
    Done,
    /// The user dismissed the authorization dialog: nothing happened.
    Cancelled,
    /// No authentication agent (or no pkexec) to ask with.
    NoAgent,
    /// The rule is installed but the device is still not accessible.
    StillDenied,
    /// The install script failed with this exit code.
    Failed(i32),
    Error(String),
}

/// Everything the window shows, copied from the app each frame.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub lifecycle: Lifecycle,
    pub status: HostStatus,
    pub config: TrayConfig,
    pub autostart: bool,
    pub adb: AdbInfo,
    /// `Some` only on Windows with the driver page.
    pub vdd: Option<VddInfo>,
    pub repair: RepairState,
    pub input_access: InputAccess,
    pub input_fix: InputFixState,
    pub has_licenses_dir: bool,
    /// GPU choices and the USB driver hint are Windows-only.
    pub windows: bool,
    pub version: String,
}

/// What the user asked for; the app carries these out after the frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The configuration changed (saved and passed to the service for the next session).
    SetConfig(TrayConfig),
    /// Apply the configuration now: the running session ends and the tablet reconnects.
    ApplyNow,
    Start,
    Stop,
    SetAutostart(bool),
    OpenLogs,
    OpenLicenses,
    OpenUrl(String),
    Repair,
    /// Install the udev rule that lets the app create input devices (asks for authorization).
    EnableInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Status,
    Display,
    Input,
    Connection,
    General,
    Driver,
}

impl Section {
    fn title(self, l: &FluentLanguageLoader) -> String {
        match self {
            Section::Status => fl!(l, "section-status"),
            Section::Display => fl!(l, "section-display"),
            Section::Input => fl!(l, "section-input"),
            Section::Connection => fl!(l, "section-connection"),
            Section::General => fl!(l, "section-general"),
            Section::Driver => fl!(l, "section-driver"),
        }
    }
}

/// The window's state.
pub struct SettingsView {
    pub snap: Snapshot,
    pub section: Section,
    actions: Vec<Action>,
    adapter_text: String,
    adapter_focused: bool,
    gpu_named: bool,
}

impl SettingsView {
    pub fn new(snap: Snapshot) -> Self {
        let mut v = SettingsView {
            adapter_text: String::new(),
            adapter_focused: false,
            gpu_named: false,
            section: Section::default(),
            actions: Vec::new(),
            snap: snap.clone(),
        };
        v.set_snapshot(snap);
        v
    }

    /// Replaces the shown data (the section and text being edited stay).
    pub fn set_snapshot(&mut self, snap: Snapshot) {
        if !self.adapter_focused {
            self.adapter_text = snap.config.service.adapter.clone().unwrap_or_default();
        }
        self.gpu_named |= snap.config.service.adapter.is_some();
        self.snap = snap;
        if self.section == Section::Driver && self.snap.vdd.is_none() {
            self.section = Section::Status;
        }
    }

    /// What was requested since the last call.
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// Draws the whole window into `ui` (the root ui of the frame).
    pub fn show(&mut self, ui: &mut Ui, l: &FluentLanguageLoader) {
        let before = self.snap.config.clone();
        let pal = theme::palette_of(ui);
        egui::Panel::left("sections")
            .exact_size(200.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.surface_container)
                    .inner_margin(12.0),
            )
            .show(ui, |ui| self.nav(ui, l, pal));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.surface).inner_margin(20.0))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(560.0);
                        self.content(ui, l, pal);
                    });
            });
        if self.snap.config != before {
            self.actions
                .push(Action::SetConfig(self.snap.config.clone()));
        }
    }

    fn nav(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(NAV_INSET);
            ui.label(RichText::new("ezdisplay").heading().color(pal.primary));
        });
        ui.add_space(12.0);
        let mut sections = vec![
            Section::Status,
            Section::Display,
            Section::Input,
            Section::Connection,
            Section::General,
        ];
        if self.snap.vdd.is_some() {
            sections.push(Section::Driver);
        }
        for s in sections {
            let selected = self.section == s;
            let text = RichText::new(s.title(l)).size(15.0);
            let size = egui::vec2(ui.available_width(), 34.0);
            // egui places a button's label by the layout it is in: start of the row, centred
            // vertically.
            let r = ui
                .allocate_ui_with_layout(
                    size,
                    egui::Layout::left_to_right(egui::Align::Center)
                        .with_main_align(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().button_padding.x = NAV_INSET;
                        ui.add(
                            egui::Button::selectable(selected, text)
                                .fill(if selected {
                                    pal.primary_container
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .min_size(size),
                        )
                    },
                )
                .inner;
            if r.clicked() {
                self.section = s;
            }
        }
    }

    fn content(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        ui.heading(self.section.title(l));
        ui.add_space(8.0);
        match self.section {
            Section::Status => self.status(ui, l, pal),
            Section::Display => self.display(ui, l, pal),
            Section::Input => self.input(ui, l, pal),
            Section::Connection => self.connection(ui, l, pal),
            Section::General => self.general(ui, l, pal),
            Section::Driver => self.driver(ui, l, pal),
        }
    }

    // ---------------------------------------------------------------- status

    fn status(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let line = text::status_line(l, self.snap.lifecycle, &self.snap.status);
        let is_error = matches!(self.snap.status, HostStatus::Error(_));
        let rich = RichText::new(line).size(17.0).strong();
        ui.label(if is_error {
            rich.color(pal.error)
        } else {
            rich
        });
        ui.add_space(8.0);

        let starts = !self.snap.lifecycle.wants_running();
        let label = if starts {
            fl!(l, "menu-start")
        } else {
            fl!(l, "menu-stop")
        };
        let clicked = if starts {
            primary_button(ui, pal, &label)
        } else {
            tonal_button(ui, pal, &label)
        };
        if clicked {
            self.actions
                .push(if starts { Action::Start } else { Action::Stop });
        }
        ui.add_space(12.0);
        if self.input_blocked() {
            ui.label(RichText::new(fl!(l, "input-notice")).color(pal.error));
            self.enable_input_button(ui, l, pal);
            ui.add_space(12.0);
        }

        match self.snap.status.clone() {
            HostStatus::Streaming(live) => self.live(ui, l, pal, &live),
            HostStatus::Waiting { devices } if !devices.is_empty() => {
                subheading(ui, &fl!(l, "devices-heading"));
                for d in &devices {
                    ui.label(device_line(l, d));
                }
            }
            _ => {}
        }
    }

    fn live(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette, live: &LiveSession) {
        let s = &live.stream;
        egui::Grid::new("live")
            .num_columns(2)
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                row(ui, pal, &fl!(l, "stat-device"), &live.device_name);
                row(
                    ui,
                    pal,
                    &fl!(l, "stat-mode"),
                    &fl!(
                        l,
                        "mode-value",
                        width = s.width,
                        height = s.height,
                        hz = text::decimal(l, text::hz(s.refresh_mhz)),
                        codec = format!("{:?}", s.codec)
                    ),
                );
                row(
                    ui,
                    pal,
                    &fl!(l, "stat-encoder"),
                    &fl!(
                        l,
                        "encoder-value",
                        encoder = s.encoder_name.as_str(),
                        mbps = mbps(l, u64::from(s.bitrate_kbps) * 1000)
                    ),
                );

                // The statistics heading is a row of the same grid.
                ui.end_row();
                subheading(ui, &fl!(l, "stats-heading"));
                ui.end_row();
                let Some(stats) = &live.stats else {
                    ui.label(RichText::new(fl!(l, "stats-waiting")).color(pal.on_surface_variant));
                    ui.end_row();
                    return;
                };
                let secs = stats.window.as_secs_f64();
                if secs > 0.0 {
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-fps"),
                        &fl!(
                            l,
                            "value-fps",
                            fps = format!("{:.0}", stats.frames as f64 / secs)
                        ),
                    );
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-bitrate"),
                        &fl!(
                            l,
                            "value-mbps",
                            mbps = mbps(l, (stats.bytes as f64 * 8.0 / secs) as u64)
                        ),
                    );
                }
                if let Some(c) = &stats.client {
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-e2e"),
                        &pair(l, Some(c.e2e_us_p50), Some(c.e2e_us_p95)),
                    );
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-decode"),
                        &pair(l, Some(c.decode_us_p50), Some(c.decode_us_p95)),
                    );
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-drops"),
                        &fl!(
                            l,
                            "value-drops",
                            dropped = c.frames_dropped,
                            late = stats.ack_timeouts
                        ),
                    );
                }
                if stats.transfer_us_p50.is_some() {
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-transfer"),
                        &pair(l, stats.transfer_us_p50, stats.transfer_us_p95),
                    );
                }
                if stats.input_latency_p50_us.is_some() {
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-input"),
                        &pair(l, stats.input_latency_p50_us, stats.input_latency_p95_us),
                    );
                }
                if let Some(rtt) = stats.rtt_us {
                    row(
                        ui,
                        pal,
                        &fl!(l, "stat-rtt"),
                        &fl!(l, "value-ms", ms = ms(l, rtt.max(0) as u64)),
                    );
                }
            });
    }

    // --------------------------------------------------------------- display

    fn display(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        hint(ui, pal, &fl!(l, "display-hint"));
        let windows = self.snap.windows;
        let cfg = &mut self.snap.config.service;

        // Scale.
        let current = cfg.scale_permille;
        labelled(ui, &fl!(l, "setting-scale"), |ui| {
            ComboBox::from_id_salt("scale")
                .selected_text(fl!(
                    l,
                    "menu-scale-option",
                    percent = menu_model::percent(current)
                ))
                .show_ui(ui, |ui| {
                    let mut options = menu_model::SCALES.to_vec();
                    if !options.contains(&current) {
                        options.push(current);
                        options.sort_unstable_by(|a, b| b.cmp(a));
                    }
                    for p in options {
                        let text = fl!(l, "menu-scale-option", percent = menu_model::percent(p));
                        ui.selectable_value(&mut cfg.scale_permille, p, text);
                    }
                })
                .response
        });

        // Refresh rate.
        let hz_now = cfg.max_refresh_mhz;
        labelled(ui, &fl!(l, "setting-refresh"), |ui| {
            ComboBox::from_id_salt("refresh")
                .selected_text(fl!(l, "value-hz", hz = text::decimal(l, text::hz(hz_now))))
                .show_ui(ui, |ui| {
                    let mut options: Vec<u32> = REFRESH_HZ.iter().map(|h| h * 1000).collect();
                    if !options.contains(&hz_now) {
                        options.push(hz_now);
                        options.sort_unstable();
                    }
                    for mhz in options {
                        let text = fl!(l, "value-hz", hz = text::decimal(l, text::hz(mhz)));
                        ui.selectable_value(&mut cfg.max_refresh_mhz, mhz, text);
                    }
                })
                .response
        });

        // Bitrate.
        let mut auto = cfg.bitrate_kbps.is_none();
        labelled(ui, &fl!(l, "setting-bitrate"), |ui| {
            ui.horizontal(|ui| {
                let r = ui.checkbox(&mut auto, fl!(l, "bitrate-auto"));
                if let Some(kbps) = &mut cfg.bitrate_kbps {
                    let mut m = (*kbps / 1000).max(1);
                    if ui
                        .add(egui::DragValue::new(&mut m).range(1..=200).suffix(" Mbps"))
                        .changed()
                    {
                        *kbps = m * 1000;
                    }
                }
                r
            })
            .inner
        });
        if auto && cfg.bitrate_kbps.is_some() {
            cfg.bitrate_kbps = None;
        } else if !auto && cfg.bitrate_kbps.is_none() {
            cfg.bitrate_kbps = Some(40_000);
        }

        // Encoder.
        let auto_text = fl!(l, "encoder-auto");
        labelled(ui, &fl!(l, "setting-encoder"), |ui| {
            ComboBox::from_id_salt("encoder")
                .selected_text(cfg.encoder.clone().unwrap_or_else(|| auto_text.clone()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.encoder, None, auto_text.clone());
                    let mut names: Vec<String> =
                        ENCODERS.iter().map(|s| (*s).to_string()).collect();
                    if let Some(c) = &cfg.encoder
                        && !names.contains(c)
                    {
                        names.push(c.clone());
                    }
                    for n in names {
                        ui.selectable_value(&mut cfg.encoder, Some(n.clone()), n);
                    }
                })
                .response
        });

        // GPU (Windows).
        if windows {
            ui.add_space(6.0);
            subheading(ui, &fl!(l, "setting-gpu"));
            let named = self.gpu_named;
            let prefer = cfg.prefer_dgpu;
            if ui.radio(!named && !prefer, fl!(l, "gpu-auto")).clicked() {
                self.gpu_named = false;
                cfg.prefer_dgpu = false;
                cfg.adapter = None;
            }
            if ui
                .radio(!named && prefer, fl!(l, "gpu-prefer-dgpu"))
                .clicked()
            {
                self.gpu_named = false;
                cfg.prefer_dgpu = true;
                cfg.adapter = None;
            }
            if ui.radio(named, fl!(l, "gpu-named")).clicked() {
                self.gpu_named = true;
                cfg.prefer_dgpu = false;
            }
            if self.gpu_named {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.adapter_text)
                        .hint_text(fl!(l, "gpu-name-hint"))
                        .desired_width(280.0),
                );
                self.adapter_focused = r.has_focus();
                if r.changed() {
                    let t = self.adapter_text.trim();
                    cfg.adapter = (!t.is_empty()).then(|| t.to_string());
                }
            }
        }

        ui.add_space(14.0);
        let streaming = matches!(self.snap.status, HostStatus::Streaming(_));
        let apply = ui.add_enabled(streaming, egui::Button::new(fl!(l, "apply-now")));
        if apply.clicked() {
            self.actions.push(Action::ApplyNow);
        }
    }

    // ----------------------------------------------------------------- input

    /// Input is on in the settings but the system refuses to create input devices.
    fn input_blocked(&self) -> bool {
        self.snap.config.service.input && self.snap.input_access != InputAccess::Ok
    }

    /// The "Enable input" button and the outcome of the last attempt.
    fn enable_input_button(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let running = self.snap.input_fix == InputFixState::Running;
        if primary_button_enabled(ui, pal, &fl!(l, "input-enable"), !running) {
            self.actions.push(Action::EnableInput);
        }
        match &self.snap.input_fix {
            InputFixState::Idle | InputFixState::Done => {}
            InputFixState::Running => {
                ui.label(fl!(l, "input-enable-running"));
            }
            InputFixState::Cancelled => {
                ui.label(fl!(l, "input-enable-cancelled"));
            }
            InputFixState::NoAgent => {
                ui.colored_label(pal.error, fl!(l, "input-enable-no-agent"));
            }
            InputFixState::StillDenied => {
                ui.colored_label(pal.error, fl!(l, "input-enable-still-denied"));
            }
            InputFixState::Failed(code) => {
                let code = i64::from(*code);
                ui.colored_label(pal.error, fl!(l, "input-enable-failed", code = code));
            }
            InputFixState::Error(reason) => {
                ui.colored_label(
                    pal.error,
                    fl!(l, "input-enable-error", reason = reason.as_str()),
                );
            }
        }
    }

    fn input(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        if self.input_blocked() {
            let text = if self.snap.input_access == InputAccess::Missing {
                fl!(l, "input-permission-missing")
            } else {
                fl!(l, "input-permission-denied")
            };
            egui::Frame::new()
                .fill(pal.surface_container)
                .corner_radius(12.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(text).color(pal.error));
                    ui.add_space(6.0);
                    self.enable_input_button(ui, l, pal);
                });
            ui.add_space(12.0);
        } else if self.snap.input_fix == InputFixState::Done {
            ui.label(fl!(l, "input-enable-done"));
            ui.add_space(8.0);
        }
        let cfg = &mut self.snap.config.service;
        ui.checkbox(&mut cfg.input, fl!(l, "input-enabled"));
        let selected = match cfg.pen_mode {
            PenChoice::Tablet => fl!(l, "pen-tablet"),
            PenChoice::Mouse => fl!(l, "pen-mouse"),
        };
        ui.add_enabled_ui(cfg.input, |ui| {
            labelled(ui, &fl!(l, "setting-pen"), |ui| {
                ComboBox::from_id_salt("pen-mode")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut cfg.pen_mode,
                            PenChoice::Tablet,
                            fl!(l, "pen-tablet"),
                        );
                        ui.selectable_value(
                            &mut cfg.pen_mode,
                            PenChoice::Mouse,
                            fl!(l, "pen-mouse"),
                        );
                    })
                    .response
            });
        });
        ui.checkbox(&mut cfg.telemetry, fl!(l, "telemetry"));
    }

    // ------------------------------------------------------------ connection

    fn connection(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        // adb.
        egui::Grid::new("adb-info")
            .num_columns(2)
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                let value = match &self.snap.adb.path {
                    Some(p) if self.snap.adb.bundled => {
                        fl!(l, "adb-bundled", path = p.display().to_string())
                    }
                    Some(p) => fl!(l, "adb-sdk", path = p.display().to_string()),
                    None => fl!(l, "error-adb-not-found"),
                };
                row(ui, pal, "adb", &value);
            });
        ui.add_space(10.0);

        // Devices.
        let devices: Vec<DeviceInfo> = match &self.snap.status {
            HostStatus::Waiting { devices } => devices.clone(),
            HostStatus::Streaming(s) => s.devices.clone(),
            _ => Vec::new(),
        };
        subheading(ui, &fl!(l, "devices-count", count = devices.len()));
        if devices.is_empty() {
            ui.label(RichText::new(fl!(l, "devices-none")).color(pal.on_surface_variant));
            if self.snap.windows && ui.link(fl!(l, "samsung-driver")).clicked() {
                self.actions
                    .push(Action::OpenUrl(SAMSUNG_DRIVER_URL.into()));
            }
        }
        for d in &devices {
            ui.label(device_line(l, d));
        }
        ui.add_space(10.0);

        let cfg = &mut self.snap.config.service;
        ui.checkbox(&mut cfg.open_app_on_connect, fl!(l, "open-app"));
        let adb_off = cfg.adb == AdbMode::Off;
        let mut use_adb = !adb_off;
        if ui.checkbox(&mut use_adb, fl!(l, "use-adb")).changed() {
            cfg.adb = if use_adb { AdbMode::Auto } else { AdbMode::Off };
        }

        ui.add_space(8.0);
        egui::CollapsingHeader::new(fl!(l, "advanced")).show(ui, |ui| {
            let mut port = cfg.listen.port();
            labelled(ui, &fl!(l, "setting-port"), |ui| {
                ui.add(egui::DragValue::new(&mut port).range(1024..=65535))
            });
            if port != cfg.listen.port() {
                cfg.listen.set_port(port);
            }
            hint(ui, pal, &fl!(l, "port-hint"));
        });
    }

    // --------------------------------------------------------------- general

    fn general(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let mut autostart = self.snap.autostart;
        if ui
            .checkbox(&mut autostart, fl!(l, "menu-autostart"))
            .changed()
        {
            self.snap.autostart = autostart;
            self.actions.push(Action::SetAutostart(autostart));
        }
        ui.checkbox(
            &mut self.snap.config.start_on_launch,
            fl!(l, "start-on-launch"),
        );
        ui.add_space(10.0);
        if ui.button(fl!(l, "menu-logs")).clicked() {
            self.actions.push(Action::OpenLogs);
        }
        ui.add_space(14.0);
        ui.label(
            RichText::new(fl!(
                l,
                "about-version",
                version = self.snap.version.as_str()
            ))
            .strong(),
        );
        ui.horizontal(|ui| {
            if ui.button(fl!(l, "licenses")).clicked() {
                self.actions.push(if self.snap.has_licenses_dir {
                    Action::OpenLicenses
                } else {
                    Action::OpenUrl(REPOSITORY_URL.into())
                });
            }
            if ui.link(REPOSITORY_URL).clicked() {
                self.actions.push(Action::OpenUrl(REPOSITORY_URL.into()));
            }
        });
        let _ = pal;
    }

    // ---------------------------------------------------------------- driver

    fn driver(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let Some(info) = self.snap.vdd.clone() else {
            return;
        };
        egui::Grid::new("driver-info")
            .num_columns(2)
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                let state = if info.present {
                    fl!(l, "driver-installed")
                } else {
                    fl!(l, "driver-missing")
                };
                row(ui, pal, &fl!(l, "driver-state"), &state);
                let owner = if !info.present {
                    fl!(l, "driver-owner-none")
                } else if info.owned {
                    fl!(l, "driver-owner-ours")
                } else {
                    fl!(l, "driver-owner-other")
                };
                row(ui, pal, &fl!(l, "driver-owner"), &owner);
                if let Some(dir) = &info.driver_dir {
                    row(ui, pal, &fl!(l, "driver-folder"), dir);
                }
            });
        ui.add_space(12.0);
        hint(ui, pal, &fl!(l, "driver-hint"));
        let running = self.snap.repair == RepairState::Running;
        if ui
            .add_enabled(!running, egui::Button::new(fl!(l, "driver-repair")))
            .clicked()
        {
            self.actions.push(Action::Repair);
        }
        ui.add_space(6.0);
        match &self.snap.repair {
            RepairState::Idle => {}
            RepairState::Running => {
                ui.label(fl!(l, "repair-running"));
            }
            RepairState::Cancelled => {
                ui.label(fl!(l, "repair-cancelled"));
            }
            RepairState::Finished(0) => {
                ui.label(fl!(l, "repair-done"));
            }
            RepairState::Finished(EXIT_REBOOT_REQUIRED) => {
                ui.label(fl!(l, "repair-restart"));
            }
            RepairState::Finished(code) => {
                let code = i64::from(*code);
                ui.colored_label(pal.error, fl!(l, "repair-failed", code = code));
                ui.label(fl!(
                    l,
                    "repair-log",
                    path = r"%ProgramData%\ezdisplay\logs\vdd-install.log"
                ));
            }
            RepairState::Failed(reason) => {
                ui.colored_label(pal.error, fl!(l, "repair-error", reason = reason.as_str()));
            }
        }
    }
}

// ------------------------------------------------------------------- helpers

/// Left inset of the sidebar title and item labels.
const NAV_INSET: f32 = 12.0;

/// Column gap and row gap of the two-column grids.
const GRID_SPACING: [f32; 2] = [28.0, 6.0];

/// A filled button for the main action of a page.
fn primary_button(ui: &mut Ui, pal: Palette, text: &str) -> bool {
    primary_button_enabled(ui, pal, text, true)
}

fn primary_button_enabled(ui: &mut Ui, pal: Palette, text: &str, enabled: bool) -> bool {
    action_button(
        ui,
        RichText::new(text).color(pal.on_primary),
        pal.primary,
        enabled,
    )
}

/// A tonal button: the main action of a page when it is not a call to act, such as Stop.
fn tonal_button(ui: &mut Ui, pal: Palette, text: &str) -> bool {
    action_button(
        ui,
        RichText::new(text).color(pal.on_surface),
        pal.primary_container,
        true,
    )
}

fn action_button(ui: &mut Ui, text: RichText, fill: egui::Color32, enabled: bool) -> bool {
    // In a row, so the label is centred in the button's minimum width.
    ui.horizontal(|ui| {
        ui.add_enabled(
            enabled,
            egui::Button::new(text.strong())
                .fill(fill)
                .min_size(egui::vec2(96.0, 32.0)),
        )
    })
    .inner
    .clicked()
}

/// Heading of a group inside a page.
fn subheading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(16.0).strong());
}

/// Secondary explanation text.
fn hint(ui: &mut Ui, pal: Palette, text: &str) {
    ui.label(RichText::new(text).color(pal.on_surface_variant));
    ui.add_space(8.0);
}

/// `label  [widget]` on one line, with the label tied to the widget for screen readers.
fn labelled(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> egui::Response) {
    ui.horizontal(|ui| {
        let name = ui.label(label);
        let r = add(ui);
        r.labelled_by(name.id);
    });
    ui.add_space(4.0);
}

/// One line of a two-column grid.
fn row(ui: &mut Ui, pal: Palette, name: &str, value: &str) {
    ui.label(RichText::new(name).color(pal.on_surface_variant));
    ui.label(value);
    ui.end_row();
}

fn device_line(l: &FluentLanguageLoader, d: &DeviceInfo) -> String {
    let state = match &d.state {
        DeviceState::Ready => fl!(l, "device-ready"),
        DeviceState::Unauthorized => fl!(l, "device-unauthorized"),
        DeviceState::Offline => fl!(l, "device-offline"),
        DeviceState::Other(s) => s.clone(),
    };
    fl!(
        l,
        "device-line",
        name = d.model.as_deref().unwrap_or(d.serial.as_str()),
        serial = d.serial.as_str(),
        state = state
    )
}

/// Microseconds as milliseconds with one decimal.
fn ms(l: &FluentLanguageLoader, us: u64) -> String {
    text::decimal(l, format!("{:.1}", us as f64 / 1000.0))
}

/// Bits per second as Mbit/s with one decimal.
fn mbps(l: &FluentLanguageLoader, bits_per_s: u64) -> String {
    text::decimal(l, format!("{:.1}", bits_per_s as f64 / 1_000_000.0))
}

/// "12.3 ms (95th percentile: 20.1 ms)".
fn pair<T: Into<u64> + Copy>(l: &FluentLanguageLoader, p50: Option<T>, p95: Option<T>) -> String {
    match (p50, p95) {
        (Some(a), Some(b)) => fl!(
            l,
            "value-ms-pair",
            p50 = ms(l, a.into()),
            p95 = ms(l, b.into())
        ),
        (Some(a), None) => fl!(l, "value-ms", ms = ms(l, a.into())),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests;
