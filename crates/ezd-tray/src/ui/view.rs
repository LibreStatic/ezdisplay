//! The settings window as a plain egui view: it reads a [`Snapshot`] of the app, edits a copy of
//! the configuration and records what the user asked for as [`Action`]s. It knows nothing about
//! the tray or the service, so tests render it with fake state.

use super::glyphs;
use super::theme::{self, Palette};
use super::widgets::{self, Tone};
use crate::config::TrayConfig;
use crate::menu_model;
use crate::state::Lifecycle;
use crate::text;
use eframe::egui::{self, ComboBox, Margin, RichText, Ui};
use ezd_host_service::{
    AdbMode, DeviceInfo, DeviceState, HdrChoice, HostStatus, LiveSession, PenChoice,
};
use ezd_proto::{DynamicRange, SdrReason};
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
    /// Windows: run elevated (administrator) or not; applied after restarting the app.
    SetRunElevated(bool),
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

    fn icon(self) -> &'static str {
        match self {
            Section::Status => glyphs::PULSE,
            Section::Display => glyphs::DESKTOP,
            Section::Input => glyphs::PEN,
            Section::Connection => glyphs::PLUG_CONNECTED,
            Section::General => glyphs::SETTINGS,
            Section::Driver => glyphs::DEVELOPER_BOARD,
        }
    }
}

/// Width of the nav column.
const NAV_WIDTH: f32 = 200.0;
/// Widest the content of a page grows.
const CONTENT_WIDTH: f32 = 640.0;
/// Space around the content of a page.
const PAGE_PADDING: i8 = 24;
/// Space between cards.
const CARD_GAP: f32 = 12.0;
/// Width of the drop-down controls of the rows.
const COMBO_WIDTH: f32 = 130.0;
/// Narrowest a stat tile gets before the grid drops a column.
const TILE_MIN_WIDTH: f32 = 140.0;

/// The window's state.
pub struct SettingsView {
    pub snap: Snapshot,
    pub section: Section,
    actions: Vec<Action>,
    adapter_text: String,
    adapter_focused: bool,
    gpu_named: bool,
    advanced_open: bool,
}

/// One measurement of the stat tile grid.
struct Tile {
    icon: &'static str,
    label: String,
    value: String,
    unit: Option<String>,
}

/// The tone and icon of the status banner.
fn status_look(life: Lifecycle, status: &HostStatus) -> (Tone, &'static str) {
    if matches!(life, Lifecycle::Stopping { .. }) {
        return (Tone::Neutral, glyphs::ARROW_SYNC);
    }
    match status {
        HostStatus::Stopped => (Tone::Neutral, glyphs::STOP),
        HostStatus::Waiting { devices } => {
            let any_ready = devices.iter().any(|d| d.state == DeviceState::Ready);
            let any_unauthorized = devices.iter().any(|d| d.state == DeviceState::Unauthorized);
            if !any_ready && any_unauthorized {
                (Tone::Warning, glyphs::WARNING)
            } else {
                (Tone::Warning, glyphs::PLUG_DISCONNECTED)
            }
        }
        HostStatus::Streaming(_) => (Tone::Success, glyphs::CHECKMARK_CIRCLE),
        HostStatus::Error(_) => (Tone::Error, glyphs::ERROR_CIRCLE),
    }
}

impl SettingsView {
    pub fn new(snap: Snapshot) -> Self {
        let mut v = SettingsView {
            adapter_text: String::new(),
            adapter_focused: false,
            gpu_named: false,
            advanced_open: false,
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
            .exact_size(NAV_WIDTH)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.nav_bg)
                    .inner_margin(Margin::same(12)),
            )
            .show(ui, |ui| self.nav(ui, l, pal));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.window_bg))
            .show(ui, |ui| {
                if self.section == Section::Display {
                    // Declared before the scroll area so that it keeps its place at the bottom.
                    egui::Panel::bottom("apply-footer")
                        .resizable(false)
                        .show_separator_line(false)
                        .frame(egui::Frame::new().fill(pal.window_bg))
                        .show(ui, |ui| self.footer(ui, l, pal));
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(Margin::same(PAGE_PADDING))
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width().min(CONTENT_WIDTH));
                                self.content(ui, l, pal);
                            });
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
            ui.add_space(6.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 7.0, pal.accent);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                glyphs::DESKTOP,
                egui::FontId::proportional(18.0),
                pal.on_accent,
            );
            ui.label(
                RichText::new("ezdisplay")
                    .size(18.0)
                    .strong()
                    .color(pal.text),
            );
        });
        ui.add_space(16.0);
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
            if widgets::nav_item(ui, pal, s.icon(), &s.title(l), selected).clicked() {
                self.section = s;
            }
            ui.add_space(2.0);
        }
    }

    fn content(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        ui.label(
            RichText::new(self.section.title(l))
                .size(26.0)
                .strong()
                .color(pal.text),
        );
        ui.add_space(16.0);
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
        let (tone, icon) = status_look(self.snap.lifecycle, &self.snap.status);
        let starts = !self.snap.lifecycle.wants_running();
        let label = if starts {
            fl!(l, "menu-start")
        } else {
            fl!(l, "menu-stop")
        };
        // "Streaming to <device> · <mode>": the device is the title, the mode the second line.
        let (title, subtitle) = match (&self.snap.status, line.split_once(" \u{b7} ")) {
            (HostStatus::Streaming(_), Some((title, mode))) => (title, Some(mode)),
            _ => (line.as_str(), None),
        };
        let clicked = widgets::banner(ui, pal, tone, icon, title, subtitle, |ui| {
            if starts {
                widgets::primary_button(ui, pal, &label).clicked()
            } else {
                widgets::secondary_button(ui, pal, &label).clicked()
            }
        })
        .inner;
        if clicked {
            self.actions
                .push(if starts { Action::Start } else { Action::Stop });
        }
        if self.input_blocked() {
            ui.add_space(CARD_GAP);
            self.enable_input_banner(ui, l, pal, &fl!(l, "input-notice"));
        }
        ui.add_space(CARD_GAP);

        let (devices, live) = match self.snap.status.clone() {
            HostStatus::Waiting { devices } => (devices, None),
            HostStatus::Streaming(live) => (live.devices.clone(), Some(live)),
            _ => (Vec::new(), None),
        };
        widgets::card(
            ui,
            pal,
            Some((glyphs::TABLET, &fl!(l, "devices-heading"))),
            |ui| {
                if devices.is_empty() {
                    widgets::empty_state(
                        ui,
                        pal,
                        glyphs::PLUG_DISCONNECTED,
                        &fl!(l, "devices-empty"),
                        Some(&fl!(l, "devices-none")),
                    );
                }
                for (i, d) in devices.iter().enumerate() {
                    if i > 0 {
                        widgets::row_divider(ui, pal);
                    }
                    device_row(ui, pal, l, d);
                }
            },
        );
        ui.add_space(CARD_GAP);
        match live {
            Some(live) => self.live(ui, l, pal, &live),
            None => {
                section_heading(ui, pal, &fl!(l, "stats-heading"));
                widgets::card(ui, pal, None, |ui| {
                    widgets::empty_state(
                        ui,
                        pal,
                        glyphs::DATA_BAR_VERTICAL,
                        &fl!(l, "stats-empty"),
                        None,
                    );
                });
            }
        }
    }

    fn live(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette, live: &LiveSession) {
        let s = &live.stream;
        let hz = text::decimal(l, text::hz(s.refresh_mhz));
        let codec = format!("{:?}", s.codec);
        let mode = if matches!(live.dynamic_range, DynamicRange::Hdr10 { .. }) {
            fl!(
                l,
                "mode-value-hdr",
                width = s.width,
                height = s.height,
                hz = hz,
                codec = codec
            )
        } else {
            fl!(
                l,
                "mode-value",
                width = s.width,
                height = s.height,
                hz = hz,
                codec = codec
            )
        };
        let mut session = vec![
            (fl!(l, "stat-device"), live.device_name.clone()),
            (fl!(l, "stat-mode"), mode),
        ];
        if let DynamicRange::SdrToneMapped(reason) = live.dynamic_range {
            session.push((fl!(l, "stat-hdr"), sdr_reason_text(l, reason)));
        }
        session.push((
            fl!(l, "stat-encoder"),
            fl!(
                l,
                "encoder-value",
                encoder = s.encoder_name.as_str(),
                mbps = mbps(l, u64::from(s.bitrate_kbps) * 1000)
            ),
        ));
        widgets::card(
            ui,
            pal,
            Some((glyphs::VIDEO, &fl!(l, "card-stream"))),
            |ui| info_rows(ui, pal, &session),
        );
        ui.add_space(CARD_GAP);

        section_heading(ui, pal, &fl!(l, "stats-heading"));
        let Some(stats) = &live.stats else {
            widgets::card(ui, pal, None, |ui| {
                widgets::empty_state(
                    ui,
                    pal,
                    glyphs::CLOCK,
                    &fl!(l, "stats-waiting"),
                    Some(&fl!(l, "stats-empty")),
                );
            });
            return;
        };

        let mut tiles = Vec::new();
        let mut details = Vec::new();
        let secs = stats.window.as_secs_f64();
        if secs > 0.0 {
            tiles.push(Tile {
                icon: glyphs::GAUGE,
                label: fl!(l, "stat-fps"),
                value: format!("{:.0}", stats.frames as f64 / secs),
                unit: Some(fl!(l, "unit-fps")),
            });
            tiles.push(Tile {
                icon: glyphs::ARROW_SWAP,
                label: fl!(l, "stat-bitrate"),
                value: mbps(l, (stats.bytes as f64 * 8.0 / secs) as u64),
                unit: Some(fl!(l, "unit-mbps")),
            });
        }
        if let Some(rtt) = stats.rtt_us {
            tiles.push(Tile {
                icon: glyphs::TIMER,
                label: fl!(l, "stat-rtt"),
                value: ms(l, rtt.max(0) as u64),
                unit: Some("ms".into()),
            });
        }
        if let Some(c) = &stats.client {
            details.push((
                fl!(l, "stat-e2e"),
                pair(l, Some(c.e2e_us_p50), Some(c.e2e_us_p95)),
            ));
            details.push((
                fl!(l, "stat-decode"),
                pair(l, Some(c.decode_us_p50), Some(c.decode_us_p95)),
            ));
            details.push((
                fl!(l, "stat-drops"),
                fl!(
                    l,
                    "value-drops",
                    dropped = c.frames_dropped,
                    late = stats.ack_timeouts
                ),
            ));
        }
        if stats.transfer_us_p50.is_some() {
            details.push((
                fl!(l, "stat-transfer"),
                pair(l, stats.transfer_us_p50, stats.transfer_us_p95),
            ));
        }
        if stats.input_latency_p50_us.is_some() {
            details.push((
                fl!(l, "stat-input"),
                pair(l, stats.input_latency_p50_us, stats.input_latency_p95_us),
            ));
        }
        tile_grid(ui, pal, &tiles);
        if !details.is_empty() {
            widgets::card(
                ui,
                pal,
                Some((glyphs::DATA_BAR_VERTICAL, &fl!(l, "card-measurements"))),
                |ui| info_rows(ui, pal, &details),
            );
        }
    }

    // --------------------------------------------------------------- display

    fn display(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let windows = self.snap.windows;
        let cfg = &mut self.snap.config.service;

        // Picture quality: scale, refresh rate, bitrate.
        let current = cfg.scale_permille;
        let hz_now = cfg.max_refresh_mhz;
        let mut auto = cfg.bitrate_kbps.is_none();
        widgets::card(
            ui,
            pal,
            Some((glyphs::DESKTOP, &fl!(l, "card-picture"))),
            |ui| {
                widgets::labelled_row(
                    ui,
                    pal,
                    &fl!(l, "setting-scale"),
                    Some(&fl!(l, "scale-description")),
                    None,
                    |ui| {
                        ComboBox::from_id_salt("scale")
                            .width(COMBO_WIDTH)
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
                                    let text = fl!(
                                        l,
                                        "menu-scale-option",
                                        percent = menu_model::percent(p)
                                    );
                                    ui.selectable_value(&mut cfg.scale_permille, p, text);
                                }
                            })
                            .response
                    },
                );
                widgets::row_divider(ui, pal);
                widgets::labelled_row(
                    ui,
                    pal,
                    &fl!(l, "setting-refresh"),
                    Some(&fl!(l, "refresh-description")),
                    None,
                    |ui| {
                        ComboBox::from_id_salt("refresh")
                            .width(COMBO_WIDTH)
                            .selected_text(fl!(
                                l,
                                "value-hz",
                                hz = text::decimal(l, text::hz(hz_now))
                            ))
                            .show_ui(ui, |ui| {
                                let mut options: Vec<u32> =
                                    REFRESH_HZ.iter().map(|h| h * 1000).collect();
                                if !options.contains(&hz_now) {
                                    options.push(hz_now);
                                    options.sort_unstable();
                                }
                                for mhz in options {
                                    let text =
                                        fl!(l, "value-hz", hz = text::decimal(l, text::hz(mhz)));
                                    ui.selectable_value(&mut cfg.max_refresh_mhz, mhz, text);
                                }
                            })
                            .response
                    },
                );
                widgets::row_divider(ui, pal);
                widgets::setting_row_wide(
                    ui,
                    pal,
                    &fl!(l, "setting-bitrate"),
                    Some(&fl!(l, "bitrate-description")),
                    210.0,
                    |ui| {
                        widgets::toggle_switch(ui, pal, &mut auto, &fl!(l, "bitrate-auto"));
                        widgets::static_text(ui, &fl!(l, "bitrate-auto"), 14.0, pal.text);
                        if let Some(kbps) = &mut cfg.bitrate_kbps {
                            let mut m = (*kbps / 1000).max(1);
                            if ui
                                .add(egui::DragValue::new(&mut m).range(1..=200).suffix(" Mbps"))
                                .changed()
                            {
                                *kbps = m * 1000;
                            }
                        }
                    },
                );
            },
        );
        if auto && cfg.bitrate_kbps.is_some() {
            cfg.bitrate_kbps = None;
        } else if !auto && cfg.bitrate_kbps.is_none() {
            cfg.bitrate_kbps = Some(40_000);
        }
        ui.add_space(CARD_GAP);

        // Encoding: encoder, HDR, graphics card.
        let auto_text = fl!(l, "encoder-auto");
        let hdr_text = |c: HdrChoice| match c {
            HdrChoice::Auto => fl!(l, "hdr-auto"),
            HdrChoice::Off => fl!(l, "hdr-off"),
        };
        let hdr_now = cfg.hdr;
        let hdr_description = if cfg!(target_os = "linux") {
            fl!(l, "hdr-hint-linux")
        } else {
            fl!(l, "hdr-description")
        };
        let named = self.gpu_named;
        let prefer = cfg.prefer_dgpu;
        let gpu_texts = [
            fl!(l, "gpu-auto"),
            fl!(l, "gpu-prefer-dgpu"),
            fl!(l, "gpu-named"),
        ];
        let gpu_mode = if named {
            2
        } else if prefer {
            1
        } else {
            0
        };
        widgets::card(
            ui,
            pal,
            Some((glyphs::VIDEO, &fl!(l, "card-encoding"))),
            |ui| {
                widgets::labelled_row(
                    ui,
                    pal,
                    &fl!(l, "setting-encoder"),
                    Some(&fl!(l, "encoder-description")),
                    None,
                    |ui| {
                        ComboBox::from_id_salt("encoder")
                            .width(COMBO_WIDTH)
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
                    },
                );
                widgets::row_divider(ui, pal);
                widgets::labelled_row(
                    ui,
                    pal,
                    &fl!(l, "setting-hdr"),
                    Some(&hdr_description),
                    None,
                    |ui| {
                        ComboBox::from_id_salt("hdr")
                            .width(COMBO_WIDTH)
                            .selected_text(hdr_text(hdr_now))
                            .show_ui(ui, |ui| {
                                for c in [HdrChoice::Auto, HdrChoice::Off] {
                                    ui.selectable_value(&mut cfg.hdr, c, hdr_text(c));
                                }
                            })
                            .response
                    },
                );
                if !windows {
                    return;
                }
                widgets::row_divider(ui, pal);
                widgets::labelled_row(
                    ui,
                    pal,
                    &fl!(l, "setting-gpu"),
                    Some(&fl!(l, "gpu-description")),
                    None,
                    |ui| {
                        ComboBox::from_id_salt("gpu")
                            .width(COMBO_WIDTH)
                            .selected_text(gpu_texts[gpu_mode].clone())
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(gpu_mode == 0, &gpu_texts[0]).clicked() {
                                    self.gpu_named = false;
                                    cfg.prefer_dgpu = false;
                                    cfg.adapter = None;
                                }
                                if ui.selectable_label(gpu_mode == 1, &gpu_texts[1]).clicked() {
                                    self.gpu_named = false;
                                    cfg.prefer_dgpu = true;
                                    cfg.adapter = None;
                                }
                                if ui.selectable_label(gpu_mode == 2, &gpu_texts[2]).clicked() {
                                    self.gpu_named = true;
                                    cfg.prefer_dgpu = false;
                                }
                            })
                            .response
                    },
                );
                if self.gpu_named {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.adapter_text)
                            .hint_text(fl!(l, "gpu-name-hint"))
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(12.0);
                    self.adapter_focused = r.has_focus();
                    if r.changed() {
                        let t = self.adapter_text.trim();
                        cfg.adapter = (!t.is_empty()).then(|| t.to_string());
                    }
                }
            },
        );
    }

    /// The bar below the Display page: what applying means and the button.
    fn footer(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let streaming = matches!(self.snap.status, HostStatus::Streaming(_));
        let mut apply = false;
        widgets::footer_bar(ui, pal, |ui| {
            apply = ui
                .add_enabled_ui(streaming, |ui| {
                    widgets::primary_button(ui, pal, &fl!(l, "apply-now")).clicked()
                })
                .inner;
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // Keep clear of the button.
                ui.set_max_width((ui.available_width() - 12.0).max(80.0));
                // A row does not wrap its labels unless told to.
                ui.add(
                    egui::Label::new(
                        RichText::new(fl!(l, "display-hint"))
                            .size(11.0)
                            .color(pal.text_secondary),
                    )
                    .wrap(),
                );
            });
        });
        if apply {
            self.actions.push(Action::ApplyNow);
        }
    }

    // ----------------------------------------------------------------- input

    /// Input is on in the settings but the system refuses to create input devices.
    fn input_blocked(&self) -> bool {
        self.snap.config.service.input && self.snap.input_access != InputAccess::Ok
    }

    /// What happened to the last "Enable input" attempt, if there is something to say.
    fn input_fix_message(&self, l: &FluentLanguageLoader) -> Option<String> {
        match &self.snap.input_fix {
            InputFixState::Idle | InputFixState::Done => None,
            InputFixState::Running => Some(fl!(l, "input-enable-running")),
            InputFixState::Cancelled => Some(fl!(l, "input-enable-cancelled")),
            InputFixState::NoAgent => Some(fl!(l, "input-enable-no-agent")),
            InputFixState::StillDenied => Some(fl!(l, "input-enable-still-denied")),
            InputFixState::Failed(code) => {
                let code = i64::from(*code);
                Some(fl!(l, "input-enable-failed", code = code))
            }
            InputFixState::Error(reason) => {
                Some(fl!(l, "input-enable-error", reason = reason.as_str()))
            }
        }
    }

    /// The warning with the "Enable input" button; the outcome of the last attempt is its second
    /// line.
    fn enable_input_banner(
        &mut self,
        ui: &mut Ui,
        l: &FluentLanguageLoader,
        pal: Palette,
        title: &str,
    ) {
        let running = self.snap.input_fix == InputFixState::Running;
        let message = self.input_fix_message(l);
        let clicked = widgets::banner(
            ui,
            pal,
            Tone::Warning,
            glyphs::WARNING,
            title,
            message.as_deref(),
            |ui| {
                ui.add_enabled_ui(!running, |ui| {
                    widgets::primary_button(ui, pal, &fl!(l, "input-enable")).clicked()
                })
                .inner
            },
        )
        .inner;
        if clicked {
            self.actions.push(Action::EnableInput);
        }
    }

    fn input(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        if self.input_blocked() {
            let text = if self.snap.input_access == InputAccess::Missing {
                fl!(l, "input-permission-missing")
            } else {
                fl!(l, "input-permission-denied")
            };
            self.enable_input_banner(ui, l, pal, &text);
            ui.add_space(CARD_GAP);
        } else if self.snap.input_fix == InputFixState::Done {
            widgets::banner(
                ui,
                pal,
                Tone::Success,
                glyphs::CHECKMARK_CIRCLE,
                &fl!(l, "input-enable-done"),
                None,
                |_| {},
            );
            ui.add_space(CARD_GAP);
        }
        let cfg = &mut self.snap.config.service;
        let tablet = fl!(l, "pen-tablet");
        let mouse = fl!(l, "pen-mouse");
        let pen_options = [
            (PenChoice::Tablet, tablet.as_str()),
            (PenChoice::Mouse, mouse.as_str()),
        ];
        let pen_width = widgets::segmented_width(ui, &[tablet.as_str(), mouse.as_str()]);
        widgets::card(ui, pal, None, |ui| {
            widgets::toggle_row(
                ui,
                pal,
                &fl!(l, "input-enabled"),
                Some(&fl!(l, "input-description")),
                &mut cfg.input,
            );
            widgets::row_divider(ui, pal);
            ui.add_enabled_ui(cfg.input, |ui| {
                widgets::setting_row_wide(
                    ui,
                    pal,
                    &fl!(l, "setting-pen"),
                    Some(&fl!(l, "pen-description")),
                    pen_width,
                    |ui| widgets::segmented(ui, pal, &mut cfg.pen_mode, &pen_options),
                );
            });
            widgets::row_divider(ui, pal);
            widgets::toggle_row(
                ui,
                pal,
                &fl!(l, "telemetry"),
                Some(&fl!(l, "telemetry-description")),
                &mut cfg.telemetry,
            );
        });
    }

    // ------------------------------------------------------------ connection

    fn connection(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let adb_value = match &self.snap.adb.path {
            Some(p) if self.snap.adb.bundled => {
                fl!(l, "adb-bundled", path = p.display().to_string())
            }
            Some(p) => fl!(l, "adb-sdk", path = p.display().to_string()),
            None => fl!(l, "error-adb-not-found"),
        };
        let devices: Vec<DeviceInfo> = match &self.snap.status {
            HostStatus::Waiting { devices } => devices.clone(),
            HostStatus::Streaming(s) => s.devices.clone(),
            _ => Vec::new(),
        };
        let windows = self.snap.windows;
        let cfg = &mut self.snap.config.service;

        // Android Debug Bridge.
        let mut use_adb = cfg.adb != AdbMode::Off;
        widgets::card(
            ui,
            pal,
            Some((glyphs::PLUG_CONNECTED, &fl!(l, "card-adb"))),
            |ui| {
                widgets::info_row(ui, pal, "adb", &adb_value);
                widgets::row_divider(ui, pal);
                if widgets::toggle_row(
                    ui,
                    pal,
                    &fl!(l, "use-adb"),
                    Some(&fl!(l, "use-adb-description")),
                    &mut use_adb,
                )
                .changed()
                {
                    cfg.adb = if use_adb { AdbMode::Auto } else { AdbMode::Off };
                }
                widgets::row_divider(ui, pal);
                widgets::toggle_row(
                    ui,
                    pal,
                    &fl!(l, "open-app"),
                    None,
                    &mut cfg.open_app_on_connect,
                );
            },
        );
        ui.add_space(CARD_GAP);

        // Devices.
        let actions = &mut self.actions;
        widgets::card(
            ui,
            pal,
            Some((glyphs::TABLET, &fl!(l, "devices-heading"))),
            |ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(fl!(l, "devices-count", count = devices.len()))
                        .size(13.0)
                        .color(pal.text_secondary),
                );
                if devices.is_empty() {
                    widgets::empty_state(
                        ui,
                        pal,
                        glyphs::PLUG_DISCONNECTED,
                        &fl!(l, "devices-empty"),
                        Some(&fl!(l, "devices-none")),
                    );
                    if windows && ui.link(fl!(l, "samsung-driver")).clicked() {
                        actions.push(Action::OpenUrl(SAMSUNG_DRIVER_URL.into()));
                    }
                }
                for d in &devices {
                    widgets::row_divider(ui, pal);
                    device_row(ui, pal, l, d);
                }
            },
        );
        ui.add_space(CARD_GAP);

        // Advanced.
        widgets::disclosure(
            ui,
            pal,
            glyphs::SETTINGS,
            &fl!(l, "advanced"),
            &mut self.advanced_open,
            |ui| {
                let mut port = cfg.listen.port();
                widgets::labelled_row(
                    ui,
                    pal,
                    &fl!(l, "setting-port"),
                    Some(&fl!(l, "port-hint")),
                    None,
                    |ui| ui.add(egui::DragValue::new(&mut port).range(1024..=65535)),
                );
                if port != cfg.listen.port() {
                    cfg.listen.set_port(port);
                }
            },
        );
    }

    // --------------------------------------------------------------- general

    fn general(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let mut autostart = self.snap.autostart;
        widgets::card(
            ui,
            pal,
            Some((glyphs::PLAY, &fl!(l, "card-startup"))),
            |ui| {
                if widgets::toggle_row(
                    ui,
                    pal,
                    &fl!(l, "menu-autostart"),
                    Some(&fl!(l, "autostart-description")),
                    &mut autostart,
                )
                .changed()
                {
                    self.snap.autostart = autostart;
                    self.actions.push(Action::SetAutostart(autostart));
                }
                widgets::row_divider(ui, pal);
                widgets::toggle_row(
                    ui,
                    pal,
                    &fl!(l, "start-on-launch"),
                    Some(&fl!(l, "start-on-launch-description")),
                    &mut self.snap.config.start_on_launch,
                );
                if self.snap.windows {
                    widgets::row_divider(ui, pal);
                    let mut elevated = self.snap.config.run_elevated;
                    if widgets::toggle_row(
                        ui,
                        pal,
                        &fl!(l, "setting-run-elevated"),
                        Some(&fl!(l, "run-elevated-hint")),
                        &mut elevated,
                    )
                    .changed()
                    {
                        self.actions.push(Action::SetRunElevated(elevated));
                    }
                }
            },
        );
        ui.add_space(CARD_GAP);
        widgets::card(
            ui,
            pal,
            Some((glyphs::FOLDER_OPEN, &fl!(l, "card-diagnostics"))),
            |ui| {
                let logs = fl!(l, "menu-logs");
                widgets::setting_row_wide(
                    ui,
                    pal,
                    &fl!(l, "logs-title"),
                    Some(&fl!(l, "logs-description")),
                    widgets::button_width(ui, &logs),
                    |ui| {
                        if widgets::secondary_button(ui, pal, &logs).clicked() {
                            self.actions.push(Action::OpenLogs);
                        }
                    },
                );
            },
        );
        ui.add_space(CARD_GAP);
        widgets::card(ui, pal, Some((glyphs::INFO, &fl!(l, "card-about"))), |ui| {
            let licenses = fl!(l, "licenses");
            widgets::setting_row_wide(
                ui,
                pal,
                &fl!(l, "about-version", version = self.snap.version.as_str()),
                Some(&fl!(l, "about-description")),
                widgets::button_width(ui, &licenses),
                |ui| {
                    if widgets::secondary_button(ui, pal, &licenses).clicked() {
                        self.actions.push(if self.snap.has_licenses_dir {
                            Action::OpenLicenses
                        } else {
                            Action::OpenUrl(REPOSITORY_URL.into())
                        });
                    }
                },
            );
            widgets::row_divider(ui, pal);
            widgets::setting_row(ui, pal, &fl!(l, "repository"), None, |ui| {
                if ui.link(REPOSITORY_URL).clicked() {
                    self.actions.push(Action::OpenUrl(REPOSITORY_URL.into()));
                }
            });
        });
    }

    // ---------------------------------------------------------------- driver

    fn driver(&mut self, ui: &mut Ui, l: &FluentLanguageLoader, pal: Palette) {
        let Some(info) = self.snap.vdd.clone() else {
            return;
        };
        let state = if info.present {
            fl!(l, "driver-installed")
        } else {
            fl!(l, "driver-missing")
        };
        let owner = if !info.present {
            fl!(l, "driver-owner-none")
        } else if info.owned {
            fl!(l, "driver-owner-ours")
        } else {
            fl!(l, "driver-owner-other")
        };
        let mut rows = vec![
            (fl!(l, "driver-state"), state),
            (fl!(l, "driver-owner"), owner),
        ];
        if let Some(dir) = &info.driver_dir {
            rows.push((fl!(l, "driver-folder"), dir.clone()));
        }
        widgets::card(
            ui,
            pal,
            Some((glyphs::DEVELOPER_BOARD, &fl!(l, "card-driver"))),
            |ui| info_rows(ui, pal, &rows),
        );
        ui.add_space(CARD_GAP);

        let running = self.snap.repair == RepairState::Running;
        let result = match &self.snap.repair {
            RepairState::Idle => None,
            RepairState::Running => Some((fl!(l, "repair-running"), false)),
            RepairState::Cancelled => Some((fl!(l, "repair-cancelled"), false)),
            RepairState::Finished(0) => Some((fl!(l, "repair-done"), false)),
            RepairState::Finished(EXIT_REBOOT_REQUIRED) => Some((fl!(l, "repair-restart"), false)),
            RepairState::Finished(code) => {
                let code = i64::from(*code);
                Some((fl!(l, "repair-failed", code = code), true))
            }
            RepairState::Failed(reason) => {
                Some((fl!(l, "repair-error", reason = reason.as_str()), true))
            }
        };
        let log_line = matches!(
            self.snap.repair,
            RepairState::Finished(code) if code != 0 && code != EXIT_REBOOT_REQUIRED
        )
        .then(|| {
            fl!(
                l,
                "repair-log",
                path = r"%ProgramData%\ezdisplay\logs\vdd-install.log"
            )
        });
        widgets::card(
            ui,
            pal,
            Some((glyphs::WRENCH, &fl!(l, "card-maintenance"))),
            |ui| {
                let repair = fl!(l, "driver-repair");
                widgets::setting_row_wide(
                    ui,
                    pal,
                    &fl!(l, "driver-reinstall"),
                    Some(&fl!(l, "driver-hint")),
                    widgets::button_width(ui, &repair),
                    |ui| {
                        let r = ui.add_enabled_ui(!running, |ui| {
                            widgets::primary_button(ui, pal, &repair).clicked()
                        });
                        if r.inner {
                            self.actions.push(Action::Repair);
                        }
                    },
                );
                if let Some((text, failed)) = result {
                    widgets::row_divider(ui, pal);
                    ui.add_space(10.0);
                    let (icon, color) = if failed {
                        (glyphs::ERROR_CIRCLE, pal.error)
                    } else if running {
                        (glyphs::ARROW_SYNC, pal.text_secondary)
                    } else {
                        (glyphs::CHECKMARK_CIRCLE, pal.success)
                    };
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(icon).size(18.0).color(color));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(text).size(14.0).color(pal.text));
                            if let Some(line) = &log_line {
                                ui.label(RichText::new(line).size(12.0).color(pal.text_secondary));
                            }
                        });
                    });
                    ui.add_space(10.0);
                }
            },
        );
    }
}

// ------------------------------------------------------------------- helpers

/// Heading of a group of cards inside a page.
fn section_heading(ui: &mut Ui, pal: Palette, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong().color(pal.text));
    ui.add_space(8.0);
}

/// Read-only lines separated by dividers.
fn info_rows(ui: &mut Ui, pal: Palette, rows: &[(String, String)]) {
    for (i, (name, value)) in rows.iter().enumerate() {
        if i > 0 {
            widgets::row_divider(ui, pal);
        }
        widgets::info_row(ui, pal, name, value);
    }
}

/// An adb device: an icon coloured by its state and the line describing it.
fn device_row(ui: &mut Ui, pal: Palette, l: &FluentLanguageLoader, d: &DeviceInfo) {
    let color = match d.state {
        DeviceState::Ready => pal.success,
        DeviceState::Unauthorized => pal.warning,
        _ => pal.text_secondary,
    };
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyphs::TABLET).size(20.0).color(color));
        ui.label(RichText::new(device_line(l, d)).size(14.0).color(pal.text));
    });
    ui.add_space(10.0);
}

/// Stat tiles in as many columns (up to three) as fit.
fn tile_grid(ui: &mut Ui, pal: Palette, tiles: &[Tile]) {
    if tiles.is_empty() {
        return;
    }
    let width = ui.available_width();
    let columns = (((width + CARD_GAP) / (TILE_MIN_WIDTH + CARD_GAP)) as usize).clamp(1, 3);
    let tile_width = (width - CARD_GAP * (columns - 1) as f32) / columns as f32;
    for row in tiles.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for t in row {
                widgets::stat_tile(
                    ui,
                    pal,
                    t.icon,
                    &t.label,
                    &t.value,
                    t.unit.as_deref(),
                    tile_width,
                );
            }
        });
        ui.add_space(CARD_GAP);
    }
}

/// Why an HDR source is streamed as converted SDR.
fn sdr_reason_text(l: &FluentLanguageLoader, reason: SdrReason) -> String {
    match reason {
        SdrReason::TurnedOff => fl!(l, "hdr-reason-turned-off"),
        SdrReason::OldClient => fl!(l, "hdr-reason-old-client"),
        SdrReason::DisplayNotHdr => fl!(l, "hdr-reason-display-not-hdr"),
        SdrReason::NoMain10Decoder => fl!(l, "hdr-reason-no-main10-decoder"),
        SdrReason::NoMain10Encoder => fl!(l, "hdr-reason-no-main10-encoder"),
    }
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
