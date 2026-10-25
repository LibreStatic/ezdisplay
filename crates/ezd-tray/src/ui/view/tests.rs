//! Semantic tests of the window: egui's AccessKit tree, no rendering and no display needed.

use super::*;
use crate::i18n::loader_for;
use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ezd_host_core::LiveStats;
use ezd_host_service::LiveSession;
use ezd_proto::{ClientStats, Codec, StreamConfig};
use std::time::Duration;

fn live(stats: Option<LiveStats>) -> HostStatus {
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
        devices: vec![],
        stats,
        input: None,
    })
}

fn stats() -> LiveStats {
    LiveStats {
        window: Duration::from_secs(1),
        frames: 119,
        bytes: 5_000_000,
        client: Some(ClientStats {
            window_ms: 1000,
            frames_received: 119,
            frames_decoded: 119,
            frames_dropped: 2,
            decode_us_p50: 4_200,
            decode_us_p95: 6_100,
            e2e_us_p50: 38_000,
            e2e_us_p95: 52_500,
        }),
        transfer_us_p50: Some(3_000),
        transfer_us_p95: Some(5_000),
        input_latency_p50_us: Some(9_000),
        input_latency_p95_us: Some(14_000),
        ack_timeouts: 1,
        rtt_us: Some(2_500),
    }
}

fn snapshot(status: HostStatus) -> Snapshot {
    Snapshot {
        lifecycle: match status {
            HostStatus::Stopped => Lifecycle::Stopped,
            _ => Lifecycle::Running,
        },
        status,
        config: TrayConfig::default(),
        autostart: false,
        adb: AdbInfo {
            path: Some("/opt/android/platform-tools/adb".into()),
            bundled: false,
        },
        vdd: None,
        repair: RepairState::Idle,
        input_access: InputAccess::Ok,
        input_fix: InputFixState::Idle,
        has_licenses_dir: false,
        windows: false,
        version: "0.1.0".into(),
    }
}

fn harness(snap: Snapshot, lang: &str) -> Harness<'static, SettingsView> {
    let l = loader_for(&[lang.parse().unwrap()]);
    let mut h = Harness::builder()
        .with_size(egui::vec2(820.0, 560.0))
        .build_ui_state(
            move |ui, view| {
                theme::install(ui.ctx());
                view.show(ui, &l);
            },
            SettingsView::new(snap),
        );
    h.run();
    h
}

#[test]
fn streaming_status_in_english_and_spanish() {
    let h = harness(snapshot(live(None)), "en");
    h.get_by_label("Streaming to Galaxy Tab S9 \u{b7} 2960\u{d7}1848 at 120 Hz");
    h.get_by_label("Stop");
    h.get_by_label("Waiting for the first measurements\u{2026}");

    let h = harness(snapshot(live(None)), "es");
    h.get_by_label("Transmitiendo a Galaxy Tab S9 \u{b7} 2960\u{d7}1848 a 120 Hz");
    h.get_by_label("Detener");
    h.get_by_label("Esperando las primeras mediciones\u{2026}");
}

#[test]
fn waiting_stopped_and_error_states() {
    let h = harness(snapshot(HostStatus::Waiting { devices: vec![] }), "en");
    h.get_by_label("Waiting for the tablet");
    h.get_by_label("Stop");

    let h = harness(snapshot(HostStatus::Stopped), "es");
    h.get_by_label("Detenido");
    h.get_by_label("Iniciar");

    let err = HostStatus::Error(ezd_host_service::ServiceError::AdbNotFound);
    let h = harness(snapshot(err.clone()), "en");
    h.get_by_label("adb was not found");
    let h = harness(snapshot(err), "es");
    h.get_by_label("No se encontr\u{f3} adb");
}

#[test]
fn live_statistics_are_listed() {
    let h = harness(snapshot(live(Some(stats()))), "en");
    h.get_by_label("Live statistics");
    h.get_by_label("119 fps");
    h.get_by_label("40.0 Mbps");
    h.get_by_label("38.0 ms (95th percentile: 52.5 ms)");
    h.get_by_label("4.2 ms (95th percentile: 6.1 ms)");
    h.get_by_label("9.0 ms (95th percentile: 14.0 ms)");
    h.get_by_label("2 by the tablet, 1 late acknowledgement");
    h.get_by_label("2.5 ms");

    let h = harness(snapshot(live(Some(stats()))), "es");
    h.get_by_label("Estad\u{ed}sticas en vivo");
    h.get_by_label("38,0 ms (percentil 95: 52,5 ms)");
    h.get_by_label("40,0 Mbps");
    h.get_by_label("2 por la tablet, 1 confirmaci\u{f3}n tard\u{ed}a");

    let mut more = stats();
    more.ack_timeouts = 3;
    let h = harness(snapshot(live(Some(more.clone()))), "en");
    h.get_by_label("2 by the tablet, 3 late acknowledgements");
    let h = harness(snapshot(live(Some(more))), "es");
    h.get_by_label("2 por la tablet, 3 confirmaciones tard\u{ed}as");
}

#[test]
fn start_button_asks_to_start() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_label("Start").click();
    h.run();
    assert_eq!(h.state_mut().take_actions(), vec![Action::Start]);
}

#[test]
fn changing_the_scale_changes_the_config() {
    let mut h = harness(snapshot(HostStatus::Waiting { devices: vec![] }), "en");
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Scale").click();
    h.run();
    h.get_by_label("75%").click();
    h.run();
    assert_eq!(h.state().snap.config.service.scale_permille, 750);
    let actions = h.state_mut().take_actions();
    let [Action::SetConfig(c)] = actions.as_slice() else {
        panic!("expected one SetConfig, got {actions:?}");
    };
    assert_eq!(c.service.scale_permille, 750);
}

#[test]
fn apply_now_requests_a_restart_while_streaming() {
    let mut h = harness(snapshot(live(None)), "en");
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    h.get_by_label("Apply now (reconnects)").click();
    h.run();
    assert_eq!(h.state_mut().take_actions(), vec![Action::ApplyNow]);
}

#[test]
fn apply_now_is_disabled_when_not_streaming() {
    let mut h = harness(snapshot(HostStatus::Waiting { devices: vec![] }), "en");
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    h.get_by_label("Apply now (reconnects)").click();
    h.run();
    assert!(h.state_mut().take_actions().is_empty());
}

#[test]
fn input_section_edits_the_config() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    h.get_by_label("Show the telemetry overlay on the tablet")
        .click();
    h.run();
    assert!(h.state().snap.config.service.telemetry);
}

#[test]
fn connection_lists_devices_and_adb() {
    let devices = vec![DeviceInfo {
        serial: "R52X204YJNW".into(),
        state: DeviceState::Unauthorized,
        model: Some("SM-X910".into()),
    }];
    let mut h = harness(snapshot(HostStatus::Waiting { devices }), "en");
    h.get_by_role_and_label(Role::Button, "Connection").click();
    h.run();
    h.get_by_label("1 device");
    h.get_by_label("SM-X910 (R52X204YJNW): waiting for you to accept USB debugging");
    h.get_by_label("/opt/android/platform-tools/adb (Android SDK)");
}

#[test]
fn samsung_driver_link_only_on_windows_without_devices() {
    let mut snap = snapshot(HostStatus::Waiting { devices: vec![] });
    snap.windows = true;
    let mut h = harness(snap, "en");
    h.get_by_role_and_label(Role::Button, "Connection").click();
    h.run();
    h.get_by_label(
        "No devices listed. Connect the tablet with a USB cable and turn on USB debugging.",
    );
    h.get_by_label("Samsung Android USB driver").click();
    h.run();
    assert_eq!(
        h.state_mut().take_actions(),
        vec![Action::OpenUrl(SAMSUNG_DRIVER_URL.into())]
    );

    let mut h = harness(snapshot(HostStatus::Waiting { devices: vec![] }), "en");
    h.get_by_role_and_label(Role::Button, "Connection").click();
    h.run();
    assert!(h.query_by_label("Samsung Android USB driver").is_none());
}

#[test]
fn general_section_actions() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_role_and_label(Role::Button, "General").click();
    h.run();
    h.get_by_label("Open logs folder").click();
    h.get_by_label("Licenses").click();
    h.get_by_label("Start with the session").click();
    h.run();
    h.get_by_label("ezdisplay 0.1.0");
    assert_eq!(
        h.state_mut().take_actions(),
        vec![
            Action::OpenLogs,
            // No licenses folder: the button opens the repository.
            Action::OpenUrl(REPOSITORY_URL.into()),
            Action::SetAutostart(true),
        ]
    );
}

#[test]
fn driver_page_exists_only_with_driver_info() {
    let h = harness(snapshot(HostStatus::Stopped), "en");
    assert!(
        h.query_by_role_and_label(Role::Button, "Display driver")
            .is_none()
    );

    let mut snap = snapshot(HostStatus::Stopped);
    snap.vdd = Some(VddInfo {
        present: true,
        owned: false,
        driver_dir: Some(r"C:\Program Files\ezdisplay\driver".into()),
    });
    let mut h = harness(snap, "en");
    h.get_by_role_and_label(Role::Button, "Display driver")
        .click();
    h.run();
    h.get_by_label("another program (left untouched)");
    h.get_by_label("Repair").click();
    h.run();
    assert_eq!(h.state_mut().take_actions(), vec![Action::Repair]);
}

#[test]
fn repair_results_are_explained() {
    let mut snap = snapshot(HostStatus::Stopped);
    snap.vdd = Some(VddInfo::default());
    for (state, text) in [
        (RepairState::Finished(0), "The driver is installed."),
        (
            RepairState::Finished(EXIT_REBOOT_REQUIRED),
            "Restart Windows to finish installing the driver.",
        ),
        (RepairState::Finished(1), "The repair failed (code 1)."),
        (RepairState::Cancelled, "Cancelled. Nothing was changed."),
        (RepairState::Running, "Repairing\u{2026}"),
    ] {
        snap.repair = state;
        let mut h = harness(snap.clone(), "en");
        h.get_by_role_and_label(Role::Button, "Display driver")
            .click();
        h.run();
        h.get_by_label(text);
    }
}

fn blocked(access: InputAccess) -> Snapshot {
    let mut snap = snapshot(HostStatus::Stopped);
    snap.input_access = access;
    snap
}

const DENIED: &str = "Touch and pen need permission to create input devices (/dev/uinput).";

#[test]
fn input_permission_card_offers_to_enable_input() {
    let mut h = harness(blocked(InputAccess::Denied), "en");
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    h.get_by_label(DENIED);
    h.get_by_role_and_label(Role::Button, "Enable input")
        .click();
    h.run();
    assert_eq!(h.state_mut().take_actions(), vec![Action::EnableInput]);

    let mut h = harness(blocked(InputAccess::Denied), "es");
    h.get_by_role_and_label(Role::Button, "Entrada").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Habilitar la entrada");
}

#[test]
fn missing_module_is_explained() {
    let mut h = harness(blocked(InputAccess::Missing), "en");
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    h.get_by_label_contains("uinput kernel module");
    h.get_by_role_and_label(Role::Button, "Enable input");
}

#[test]
fn status_page_shows_a_notice_with_the_button() {
    let mut h = harness(blocked(InputAccess::Denied), "en");
    h.get_by_label(
        "Touch and pen are blocked: ezdisplay has no permission to create input devices.",
    );
    h.get_by_role_and_label(Role::Button, "Enable input")
        .click();
    h.run();
    assert_eq!(h.state_mut().take_actions(), vec![Action::EnableInput]);
}

#[test]
fn nothing_shows_when_access_is_fine_or_input_is_off() {
    let mut h = harness(blocked(InputAccess::Ok), "en");
    assert!(h.query_by_label_contains("Enable input").is_none());
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    assert!(h.query_by_label(DENIED).is_none());
    assert!(h.query_by_label_contains("Enable input").is_none());

    let mut snap = blocked(InputAccess::Denied);
    snap.config.service.input = false;
    let mut h = harness(snap, "en");
    assert!(h.query_by_label_contains("Enable input").is_none());
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    assert!(h.query_by_label(DENIED).is_none());
    assert!(h.query_by_label_contains("Enable input").is_none());
}

#[test]
fn enable_input_results_are_explained() {
    for (state, text) in [
        (InputFixState::Running, "Waiting for authorization\u{2026}"),
        (InputFixState::Cancelled, "Cancelled. Nothing was changed."),
        (
            InputFixState::NoAgent,
            "No authentication agent is available. Install the udev rule by hand, see the README.",
        ),
        (
            InputFixState::StillDenied,
            "The rule is installed, but access is still denied. Log out and back in, then try again.",
        ),
        (InputFixState::Failed(3), "Enabling input failed (code 3)."),
    ] {
        let running = state == InputFixState::Running;
        let mut snap = blocked(InputAccess::Denied);
        snap.input_fix = state;
        let mut h = harness(snap, "en");
        h.get_by_role_and_label(Role::Button, "Input").click();
        h.run();
        h.get_by_label(text);
        // The button is disabled while the authorization dialog is open.
        h.get_by_role_and_label(Role::Button, "Enable input")
            .click();
        h.run();
        assert_eq!(h.state_mut().take_actions().is_empty(), running);
    }
}

#[test]
fn enabled_input_is_confirmed_once() {
    let mut snap = blocked(InputAccess::Ok);
    snap.input_fix = InputFixState::Done;
    let mut h = harness(snap, "en");
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    h.get_by_label("Input is enabled.");
}

#[cfg(feature = "snapshots")]
mod snapshots {
    //! PNG snapshots (`cargo test -p ezd-tray --features snapshots`; refresh them with
    //! `UPDATE_SNAPSHOTS=1`). They need a GPU and are not committed.
    use super::*;

    fn shot(
        theme: egui::Theme,
        lang: &str,
        name: &str,
        results: &mut egui_kittest::SnapshotResults,
    ) {
        let l = loader_for(&[lang.parse().unwrap()]);
        let mut h = Harness::builder()
            .with_size(egui::vec2(820.0, 560.0))
            .with_theme(theme)
            .wgpu()
            .build_ui_state(
                move |ui, view| {
                    theme::install(ui.ctx());
                    view.show(ui, &l);
                },
                SettingsView::new(snapshot(live(Some(stats())))),
            );
        h.run();
        h.snapshot(name);
        results.extend_harness(&mut h);
    }

    fn input_permission_snapshots(results: &mut egui_kittest::SnapshotResults) {
        for (theme, t) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
            for lang in ["en", "es"] {
                let l = loader_for(&[lang.parse().unwrap()]);
                let mut snap = blocked(InputAccess::Denied);
                snap.input_fix = InputFixState::NoAgent;
                let mut view = SettingsView::new(snap);
                view.section = Section::Input;
                let mut h = Harness::builder()
                    .with_size(egui::vec2(820.0, 560.0))
                    .with_theme(theme)
                    .wgpu()
                    .build_ui_state(
                        move |ui, view| {
                            theme::install(ui.ctx());
                            view.show(ui, &l);
                        },
                        view,
                    );
                h.run();
                h.snapshot(format!("input_permission_{t}_{lang}"));
                results.extend_harness(&mut h);
            }
        }
    }

    #[test]
    fn window_snapshots() {
        let mut results = egui_kittest::SnapshotResults::new();
        for (theme, t) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
            for lang in ["en", "es"] {
                shot(theme, lang, &format!("status_{t}_{lang}"), &mut results);
            }
        }
        // One test: concurrent GPU harnesses can deadlock.
        input_permission_snapshots(&mut results);
    }
}
