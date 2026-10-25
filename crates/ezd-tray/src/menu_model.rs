//! The tray menu as a pure function of the state: no tray, no display needed to test it.

use crate::state::Lifecycle;
use crate::text;
use ezd_host_service::HostStatus;
use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed_fl::fl;

/// Scale choices of the menu, in thousandths of the tablet's native size.
pub const SCALES: [u16; 5] = [1000, 900, 750, 660, 500];

/// What the menu depends on.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuState {
    pub lifecycle: Lifecycle,
    pub status: HostStatus,
    pub scale_permille: u16,
    pub telemetry: bool,
    pub autostart: bool,
}

/// Start/Stop item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toggle {
    pub label: String,
    /// What a click does.
    pub starts: bool,
}

/// One scale choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleItem {
    pub permille: u16,
    pub label: String,
    pub checked: bool,
}

/// Texts and states of every menu item, top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuModel {
    /// Disabled line with the state.
    pub status: String,
    pub toggle: Toggle,
    pub scale_title: String,
    pub scales: Vec<ScaleItem>,
    pub telemetry: (String, bool),
    pub settings: String,
    pub autostart: (String, bool),
    pub logs: String,
    pub quit: String,
}

/// Percentage text of a scale: 660 -> "66".
pub fn percent(permille: u16) -> String {
    let p = f32::from(permille) / 10.0;
    if (p - p.round()).abs() < 0.05 {
        format!("{}", p.round() as u32)
    } else {
        format!("{p:.1}")
    }
}

pub fn menu_model(l: &FluentLanguageLoader, s: &MenuState) -> MenuModel {
    let starts = !s.lifecycle.wants_running();
    MenuModel {
        status: text::status_line(l, s.lifecycle, &s.status),
        toggle: Toggle {
            label: if starts {
                fl!(l, "menu-start")
            } else {
                fl!(l, "menu-stop")
            },
            starts,
        },
        scale_title: fl!(l, "menu-scale"),
        scales: SCALES
            .iter()
            .map(|&p| ScaleItem {
                permille: p,
                label: fl!(l, "menu-scale-option", percent = percent(p)),
                checked: p == s.scale_permille,
            })
            .collect(),
        telemetry: (fl!(l, "menu-telemetry"), s.telemetry),
        settings: fl!(l, "menu-settings"),
        autostart: (fl!(l, "menu-autostart"), s.autostart),
        logs: fl!(l, "menu-logs"),
        quit: fl!(l, "menu-quit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::loader_for;
    use ezd_host_service::{DeviceInfo, DeviceState, LiveSession, ServiceError};
    use ezd_proto::{Codec, StreamConfig};

    fn state(status: HostStatus) -> MenuState {
        MenuState {
            lifecycle: Lifecycle::Running,
            status,
            scale_permille: 1000,
            telemetry: false,
            autostart: false,
        }
    }

    fn dev(state: DeviceState) -> DeviceInfo {
        DeviceInfo {
            serial: "R52X204YJNW".into(),
            state,
            model: Some("SM-X910".into()),
        }
    }

    fn streaming() -> HostStatus {
        HostStatus::Streaming(LiveSession {
            device_name: "Galaxy Tab S9".into(),
            stream: StreamConfig {
                width: 2960,
                height: 1848,
                refresh_mhz: 120_000,
                codec: Codec::H264,
                bitrate_kbps: 40_000,
                encoder_name: "h264_nvenc".into(),
            },
            dynamic_range: ezd_proto::DynamicRange::Sdr,
            devices: vec![],
            stats: None,
            input: None,
        })
    }

    fn en() -> FluentLanguageLoader {
        loader_for(&["en".parse().unwrap()])
    }

    #[test]
    fn streaming_line_and_stop_label() {
        let m = menu_model(&en(), &state(streaming()));
        assert_eq!(
            m.status,
            "Streaming to Galaxy Tab S9 \u{b7} 2960\u{d7}1848 at 120 Hz"
        );
        assert_eq!(m.toggle.label, "Stop");
        assert!(!m.toggle.starts);
    }

    #[test]
    fn waiting_and_unauthorized() {
        let waiting = menu_model(&en(), &state(HostStatus::Waiting { devices: vec![] }));
        assert_eq!(waiting.status, "Waiting for the tablet");
        let unauth = menu_model(
            &en(),
            &state(HostStatus::Waiting {
                devices: vec![dev(DeviceState::Unauthorized)],
            }),
        );
        assert_eq!(unauth.status, "Accept USB debugging on the tablet");
        // Another device is ready: the tablet may just not have connected yet.
        let mixed = menu_model(
            &en(),
            &state(HostStatus::Waiting {
                devices: vec![dev(DeviceState::Unauthorized), dev(DeviceState::Ready)],
            }),
        );
        assert_eq!(mixed.status, "Waiting for the tablet");
    }

    #[test]
    fn error_is_localized() {
        let st = state(HostStatus::Error(ServiceError::AdbNotFound));
        assert_eq!(menu_model(&en(), &st).status, "adb was not found");
        let es = loader_for(&["es".parse().unwrap()]);
        assert_eq!(menu_model(&es, &st).status, "No se encontr\u{f3} adb");
    }

    #[test]
    fn stopped_offers_start() {
        let mut st = state(HostStatus::Stopped);
        st.lifecycle = Lifecycle::Stopped;
        let m = menu_model(&en(), &st);
        assert_eq!(m.status, "Stopped");
        assert_eq!((m.toggle.label.as_str(), m.toggle.starts), ("Start", true));
    }

    #[test]
    fn stopping_and_queued_start() {
        let mut st = state(HostStatus::Stopped);
        st.lifecycle = Lifecycle::Stopping { restart: false };
        let m = menu_model(&en(), &st);
        assert_eq!(m.status, "Stopping\u{2026}");
        assert!(m.toggle.starts);
        st.lifecycle = Lifecycle::Stopping { restart: true };
        let m = menu_model(&en(), &st);
        assert!(!m.toggle.starts, "a queued start is cancelled by Stop");
    }

    #[test]
    fn scale_radio_and_checks() {
        let mut st = state(HostStatus::Stopped);
        st.scale_permille = 750;
        st.telemetry = true;
        st.autostart = true;
        let m = menu_model(&en(), &st);
        let labels: Vec<_> = m.scales.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["100%", "90%", "75%", "66%", "50%"]);
        let checked: Vec<_> = m.scales.iter().filter(|s| s.checked).collect();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].permille, 750);
        assert!(m.telemetry.1 && m.autostart.1);
        // A scale that is not in the menu (set in the window) leaves all unchecked.
        st.scale_permille = 800;
        assert!(menu_model(&en(), &st).scales.iter().all(|s| !s.checked));
    }

    #[test]
    fn percent_text() {
        assert_eq!(percent(660), "66");
        assert_eq!(percent(1000), "100");
        assert_eq!(percent(825), "82.5");
    }
}
