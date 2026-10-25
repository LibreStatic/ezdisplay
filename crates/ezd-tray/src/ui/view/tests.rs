//! Semantic tests of the window: egui's AccessKit tree, no rendering and no display needed.

use super::*;
use crate::i18n::loader_for;
use crate::ui::fonts;
use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use ezd_host_core::LiveStats;
use ezd_host_service::{HdrChoice, LiveSession};
use ezd_proto::{ClientStats, Codec, DynamicRange, SdrReason, StreamConfig};
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
        dynamic_range: DynamicRange::Sdr,
        devices: vec![],
        stats,
        input: None,
    })
}

fn live_with(dynamic_range: DynamicRange) -> HostStatus {
    let HostStatus::Streaming(mut s) = live(None) else {
        unreachable!()
    };
    s.dynamic_range = dynamic_range;
    HostStatus::Streaming(s)
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
    harness_sized(snap, lang, egui::vec2(820.0, 560.0))
}

fn harness_sized(snap: Snapshot, lang: &str, size: egui::Vec2) -> Harness<'static, SettingsView> {
    let l = loader_for(&[lang.parse().unwrap()]);
    let mut h = Harness::builder().with_size(size).build_ui_state(
        move |ui, view| {
            theme::install(ui.ctx());
            fonts::install_bundled_fonts(ui.ctx());
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
    h.get_by_label("Streaming to Galaxy Tab S9");
    h.get_by_label("2960\u{d7}1848 at 120 Hz");
    h.get_by_label("Stop");
    h.get_by_label("Waiting for the first measurements\u{2026}");

    let h = harness(snapshot(live(None)), "es");
    h.get_by_label("Transmitiendo a Galaxy Tab S9");
    h.get_by_label("2960\u{d7}1848 a 120 Hz");
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
fn changing_hdr_changes_the_config() {
    let mut h = harness(snapshot(HostStatus::Waiting { devices: vec![] }), "en");
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    assert_eq!(h.state().snap.config.service.hdr, HdrChoice::Auto);
    h.get_by_role_and_label(Role::ComboBox, "HDR").click();
    h.run();
    h.get_by_label("Off").click();
    h.run();
    assert_eq!(h.state().snap.config.service.hdr, HdrChoice::Off);
    let actions = h.state_mut().take_actions();
    let [Action::SetConfig(c)] = actions.as_slice() else {
        panic!("expected one SetConfig, got {actions:?}");
    };
    assert_eq!(c.service.hdr, HdrChoice::Off);
}

#[cfg(target_os = "linux")]
#[test]
fn display_page_explains_that_kde_sends_no_hdr() {
    let mut h = harness(snapshot(HostStatus::Waiting { devices: vec![] }), "en");
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    h.get_by_label("KDE doesn't send HDR to virtual displays yet; the stream stays SDR.");
}

#[test]
fn hdr10_is_added_to_the_mode() {
    let hdr = DynamicRange::Hdr10 { codec: Codec::Hevc };
    let h = harness(snapshot(live_with(hdr)), "en");
    h.get_by_label("2960×1848 at 120 Hz, H264 HDR10");
    assert!(h.query_by_label("HDR").is_none());
    let h = harness(snapshot(live(None)), "en");
    h.get_by_label("2960×1848 at 120 Hz, H264");
    assert!(h.query_by_label("HDR").is_none());
}

#[test]
fn every_sdr_reason_has_its_line() {
    let cases = [
        (
            SdrReason::TurnedOff,
            "HDR is off in the settings; converting to SDR",
        ),
        (
            SdrReason::OldClient,
            "The tablet app is too old for HDR; update it",
        ),
        (
            SdrReason::DisplayNotHdr,
            "The tablet's screen doesn't support HDR10",
        ),
        (
            SdrReason::NoMain10Decoder,
            "The tablet can't decode 10-bit HDR video",
        ),
        (
            SdrReason::NoMain10Encoder,
            "This graphics card can't encode 10-bit HEVC; converting to SDR",
        ),
    ];
    for (reason, text) in cases {
        let h = harness(
            snapshot(live_with(DynamicRange::SdrToneMapped(reason))),
            "en",
        );
        h.get_by_label(text);
        h.get_by_label("HDR");
    }
    let h = harness(snapshot(live_with(DynamicRange::Sdr)), "en");
    assert!(h.query_by_label("HDR").is_none());
    let h = harness(
        snapshot(live_with(DynamicRange::SdrToneMapped(SdrReason::OldClient))),
        "es",
    );
    h.get_by_label("La app de la tablet es muy vieja para HDR; actualizala");
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
    h.get_by_label("No devices detected");
    h.get_by_label("Connect the tablet with a USB cable and turn on USB debugging.");
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
fn run_elevated_checkbox_is_windows_only() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_role_and_label(Role::Button, "General").click();
    h.run();
    assert!(
        h.query_by_label("Control administrator windows (run elevated)")
            .is_none()
    );

    for (lang, label) in [
        ("en", "Control administrator windows (run elevated)"),
        (
            "es",
            "Controlar ventanas de administrador (ejecutar con privilegios elevados)",
        ),
    ] {
        let mut snap = snapshot(HostStatus::Stopped);
        snap.windows = true;
        let mut h = harness(snap, lang);
        h.get_by_role_and_label(Role::Button, "General").click();
        h.run();
        h.get_by_label(label).click();
        h.run();
        assert_eq!(
            h.state_mut().take_actions(),
            vec![Action::SetRunElevated(false)],
            "{lang}"
        );
    }
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

fn devices(state: DeviceState) -> Vec<DeviceInfo> {
    vec![DeviceInfo {
        serial: "R52X204YJNW".into(),
        state,
        model: Some("SM-X910".into()),
    }]
}

#[test]
fn the_banner_tone_follows_the_state() {
    let err = HostStatus::Error(ezd_host_service::ServiceError::AdbNotFound);
    let waiting = |d| HostStatus::Waiting { devices: d };
    for (life, status, tone) in [
        (Lifecycle::Stopped, HostStatus::Stopped, Tone::Neutral),
        (
            Lifecycle::Stopping { restart: false },
            live(None),
            Tone::Neutral,
        ),
        (Lifecycle::Running, waiting(vec![]), Tone::Warning),
        (
            Lifecycle::Running,
            waiting(devices(DeviceState::Unauthorized)),
            Tone::Warning,
        ),
        (
            Lifecycle::Running,
            waiting(devices(DeviceState::Ready)),
            Tone::Warning,
        ),
        (Lifecycle::Running, live(None), Tone::Success),
        (Lifecycle::Running, err, Tone::Error),
    ] {
        assert_eq!(status_look(life, &status).0, tone, "{status:?} {life:?}");
    }
    // Waiting for the authorization has its own icon.
    let unauthorized = waiting(devices(DeviceState::Unauthorized));
    assert_ne!(
        status_look(Lifecycle::Running, &unauthorized).1,
        status_look(Lifecycle::Running, &waiting(vec![])).1
    );
}

#[test]
fn stopping_shows_the_stop_wording_and_a_start_button_is_not_offered() {
    let mut snap = snapshot(HostStatus::Stopped);
    snap.lifecycle = Lifecycle::Stopping { restart: false };
    let h = harness(snap, "en");
    h.get_by_label("Stopping\u{2026}");
}

#[test]
fn status_empty_states() {
    let h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_label("Devices");
    h.get_by_label("No devices detected");
    h.get_by_label("Live statistics");
    h.get_by_label("Statistics appear when streaming starts");

    let h = harness(
        snapshot(HostStatus::Waiting {
            devices: devices(DeviceState::Unauthorized),
        }),
        "en",
    );
    assert!(h.query_by_label("No devices detected").is_none());
    h.get_by_label("SM-X910 (R52X204YJNW): waiting for you to accept USB debugging");
    h.get_by_label("Statistics appear when streaming starts");

    // Streaming without measurements: waiting for the first ones, no stat tiles yet.
    let h = harness(snapshot(live(None)), "en");
    h.get_by_label("Waiting for the first measurements\u{2026}");
    assert!(
        h.query_by_label("Statistics appear when streaming starts")
            .is_some()
    );
    assert!(h.query_by_label("Frame rate").is_none());
    h.get_by_label("Tablet");
    h.get_by_label("Galaxy Tab S9");
}

#[test]
fn spanish_empty_states_are_localized() {
    let h = harness(snapshot(HostStatus::Stopped), "es");
    h.get_by_label("No se detectaron dispositivos");
    h.get_by_label("Las estad\u{ed}sticas aparecen cuando empieza la transmisi\u{f3}n");
}

#[test]
fn stat_tiles_show_every_measurement_and_wrap_when_narrow() {
    let wide = harness(snapshot(live(Some(stats()))), "en");
    for label in ["Frame rate", "Bitrate", "Round trip"] {
        wide.get_by_label(label);
    }
    let y = |h: &Harness<'static, SettingsView>, label: &str| h.get_by_label(label).rect().min.y;
    // Three columns at the default size: one row.
    assert_eq!(y(&wide, "Frame rate"), y(&wide, "Bitrate"));
    assert_eq!(y(&wide, "Frame rate"), y(&wide, "Round trip"));
    // The latencies that are not a single number are rows of the measurements card.
    wide.get_by_label("Measurements");
    wide.get_by_label("Capture-to-screen latency");

    // At the minimum size the grid drops to two columns.
    let narrow = harness_sized(
        snapshot(live(Some(stats()))),
        "en",
        egui::vec2(640.0, 420.0),
    );
    assert_eq!(y(&narrow, "Frame rate"), y(&narrow, "Bitrate"));
    assert!(y(&narrow, "Round trip") > y(&narrow, "Frame rate"));
}

#[test]
fn the_apply_footer_stays_at_the_bottom_of_the_display_page() {
    let mut h = harness(snapshot(live(None)), "en");
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    h.get_by_label(
        "Changes apply from the next connection. Use \"Apply now\" to reconnect right away.",
    );
    let apply = h.get_by_label("Apply now (reconnects)").rect();
    assert!(apply.max.y > 500.0 && apply.max.y <= 560.0, "{apply:?}");
    // The hint is not on the other pages.
    h.get_by_role_and_label(Role::Button, "General").click();
    h.run();
    assert!(h.query_by_label("Apply now (reconnects)").is_none());
}

#[test]
fn the_apply_footer_fits_the_minimum_window_in_german() {
    let mut h = harness_sized(snapshot(live(None)), "de", egui::vec2(640.0, 420.0));
    h.get_by_role_and_label(Role::Button, "Anzeige").click();
    h.run();
    let apply = h.get_by_label("Jetzt anwenden (verbindet neu)").rect();
    assert!(apply.max.y <= 420.0 && apply.max.x <= 640.0, "{apply:?}");
    // The hint wraps beside the button instead of running under it.
    let hint = h.get_by_label_contains("Verbindung. Nutze").rect();
    assert!(hint.max.x <= apply.min.x, "{hint:?} {apply:?}");
}

#[test]
fn the_apply_footer_hint_takes_at_most_two_lines_at_the_minimum_window() {
    let mut h = harness_sized(snapshot(live(None)), "en", egui::vec2(640.0, 420.0));
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    let hint = h
        .get_by_label_contains("Changes apply from the next")
        .rect();
    // Two lines of 11 px text.
    assert!(hint.height() <= 30.0, "{hint:?}");
}

#[test]
fn the_display_page_scrolls_to_a_last_row_above_the_footer() {
    let mut h = harness_sized(snapshot(live(None)), "en", egui::vec2(640.0, 420.0));
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    let apply = h.get_by_label("Apply now (reconnects)").rect();
    // The HDR row is the last one; its description depends on the platform.
    let last = if cfg!(target_os = "linux") {
        "KDE doesn't send HDR"
    } else {
        "Sends HDR10 when"
    };
    h.get_by_label_contains(last).scroll_to_me();
    h.run();
    let row = h.get_by_label_contains(last).rect();
    assert!(row.max.y <= apply.min.y, "{row:?} {apply:?}");
}

#[test]
fn the_display_cards_have_titles_and_helper_text() {
    let mut h = harness(snapshot(HostStatus::Waiting { devices: vec![] }), "en");
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    h.get_by_label("Picture quality");
    h.get_by_label("Encoding");
    h.get_by_label("Lower values send fewer pixels to the tablet.");
    h.get_by_label("The video encoder that compresses the stream.");
    h.get_by_role_and_label(Role::ComboBox, "Highest refresh rate");
    h.get_by_role_and_label(Role::ComboBox, "Encoder");
    // Automatic bitrate is a switch; the number only shows when it is off.
    h.get_by_role_and_label(Role::CheckBox, "Automatic").click();
    h.run();
    assert_eq!(h.state().snap.config.service.bitrate_kbps, Some(40_000));
}

#[test]
fn the_pen_choice_is_a_segmented_control_that_needs_input_on() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Mouse pointer")
        .click();
    h.run();
    assert_eq!(h.state().snap.config.service.pen_mode, PenChoice::Mouse);
    h.get_by_role_and_label(Role::Button, "Graphics tablet")
        .click();
    h.run();
    assert_eq!(h.state().snap.config.service.pen_mode, PenChoice::Tablet);

    // With input off the choice is disabled.
    let mut snap = snapshot(HostStatus::Stopped);
    snap.config.service.input = false;
    let mut h = harness(snap, "en");
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Mouse pointer")
        .click();
    h.run();
    assert_eq!(h.state().snap.config.service.pen_mode, PenChoice::Tablet);
}

#[test]
fn the_advanced_section_collapses_and_edits_the_port() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_role_and_label(Role::Button, "Connection").click();
    h.run();
    h.get_by_label("Android Debug Bridge");
    h.get_by_role_and_label(Role::CheckBox, "Connect over USB with adb");
    // Collapsed at first.
    assert!(h.query_by_label("Port").is_none());
    h.get_by_role_and_label(Role::Button, "Advanced").click();
    h.run();
    h.get_by_label("Port");
    h.get_by_label_contains("The tablet reaches this computer on this port");
    h.get_by_role_and_label(Role::Button, "Advanced").click();
    h.run();
    assert!(h.query_by_label("Port").is_none());
}

#[test]
fn turning_adb_off_changes_the_config() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_role_and_label(Role::Button, "Connection").click();
    h.run();
    h.get_by_role_and_label(Role::CheckBox, "Connect over USB with adb")
        .click();
    h.run();
    assert_eq!(h.state().snap.config.service.adb, AdbMode::Off);
}

#[test]
fn the_general_page_has_its_three_cards() {
    let mut h = harness(snapshot(HostStatus::Stopped), "en");
    h.get_by_role_and_label(Role::Button, "General").click();
    h.run();
    for title in ["Startup", "Diagnostics", "About"] {
        h.get_by_label(title);
    }
    h.get_by_role_and_label(Role::CheckBox, "Stream when ezdisplay opens")
        .click();
    h.run();
    assert!(h.state().snap.config.start_on_launch != TrayConfig::default().start_on_launch);
}

#[test]
fn a_long_button_label_does_not_overlap_its_description() {
    let mut h = harness_sized(
        snapshot(HostStatus::Stopped),
        "de",
        egui::vec2(640.0, 420.0),
    );
    h.get_by_role_and_label(Role::Button, "Allgemein").click();
    h.run();
    let button = h.get_by_label("Protokollordner \u{f6}ffnen").rect();
    let description = h.get_by_label_contains("Protokolle schreibt").rect();
    assert!(
        !button.intersects(description),
        "{button:?} {description:?}"
    );
}

#[test]
fn the_driver_page_has_a_status_and_a_maintenance_card() {
    let mut snap = snapshot(HostStatus::Stopped);
    snap.vdd = Some(VddInfo {
        present: true,
        owned: true,
        driver_dir: Some(r"C:\ezdisplay\driver".into()),
    });
    let mut h = harness(snap.clone(), "en");
    h.get_by_role_and_label(Role::Button, "Display driver")
        .click();
    h.run();
    h.get_by_label("Driver status");
    h.get_by_label("Maintenance");
    h.get_by_label("Installed");
    h.get_by_label(r"C:\ezdisplay\driver");

    // Repair is disabled while it runs and the failure shows both lines.
    snap.repair = RepairState::Running;
    let mut h = harness(snap.clone(), "en");
    h.get_by_role_and_label(Role::Button, "Display driver")
        .click();
    h.run();
    h.get_by_label("Repair").click();
    h.run();
    assert!(h.state_mut().take_actions().is_empty());
    snap.repair = RepairState::Finished(1);
    let mut h = harness(snap, "en");
    h.get_by_role_and_label(Role::Button, "Display driver")
        .click();
    h.run();
    h.get_by_label("The repair failed (code 1).");
    h.get_by_label_contains("vdd-install.log");
}

#[test]
fn input_fix_messages_are_inside_the_warning_banner() {
    let mut snap = blocked(InputAccess::Denied);
    snap.input_fix = InputFixState::Cancelled;
    let mut h = harness(snap, "en");
    // On the status page too.
    h.get_by_label("Cancelled. Nothing was changed.");
    h.get_by_role_and_label(Role::Button, "Enable input");
    h.get_by_role_and_label(Role::Button, "Input").click();
    h.run();
    h.get_by_label(DENIED);
    h.get_by_label("Cancelled. Nothing was changed.");
}

#[test]
fn every_page_fits_the_minimum_size_in_german_and_spanish() {
    let mut snap = snapshot(live(Some(stats())));
    snap.vdd = Some(VddInfo::default());
    snap.windows = true;
    for (lang, labels) in [
        (
            "de",
            ["Status", "Anzeige", "Eingabe", "Verbindung", "Allgemein"],
        ),
        (
            "es",
            ["Estado", "Pantalla", "Entrada", "Conexi\u{f3}n", "General"],
        ),
    ] {
        let mut h = harness_sized(snap.clone(), lang, egui::vec2(640.0, 420.0));
        for label in labels {
            h.get_by_role_and_label(Role::Button, label).click();
            h.run();
            // Nothing is drawn past the right edge (the page scrolls vertically).
            for n in h.query_all_by_role(Role::Label) {
                let r = n.rect();
                assert!(
                    r.max.x <= 640.0,
                    "{lang} {label}: {:?} reaches {}",
                    n.accesskit_node().label(),
                    r.max.x
                );
            }
        }
    }
}

#[test]
fn general_buttons_stay_inside_their_cards_at_the_minimum_size() {
    for (lang, general) in [("de", "Allgemein"), ("es", "General")] {
        let mut h = harness_sized(
            snapshot(live(Some(stats()))),
            lang,
            egui::vec2(640.0, 420.0),
        );
        h.get_by_role_and_label(Role::Button, general).click();
        h.run();
        // The card ends 24 px before the window edge.
        let limit = 640.0 - 24.0;
        for n in h.query_all_by_role(Role::Button) {
            let r = n.rect();
            if r.min.x < 200.0 {
                continue; // navigation
            }
            assert!(
                r.max.x <= limit,
                "{lang}: {:?} reaches {}",
                n.accesskit_node().label(),
                r.max.x
            );
        }
    }
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
                    fonts::install_bundled_fonts(ui.ctx());
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
                            fonts::install_bundled_fonts(ui.ctx());
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

    fn hdr_snapshots(results: &mut egui_kittest::SnapshotResults) {
        for lang in ["en", "es"] {
            let reason = DynamicRange::SdrToneMapped(SdrReason::NoMain10Encoder);
            for (section, name, status) in [
                (Section::Status, "hdr_status", live_with(reason)),
                (
                    Section::Display,
                    "hdr_display",
                    live_with(DynamicRange::Sdr),
                ),
            ] {
                let mut view = SettingsView::new(snapshot(status));
                view.section = section;
                let l = loader_for(&[lang.parse().unwrap()]);
                let mut h = Harness::builder()
                    .with_size(egui::vec2(820.0, 560.0))
                    .with_theme(egui::Theme::Light)
                    .wgpu()
                    .build_ui_state(
                        move |ui, view| {
                            theme::install(ui.ctx());
                            fonts::install_bundled_fonts(ui.ctx());
                            view.show(ui, &l);
                        },
                        view,
                    );
                h.run();
                h.snapshot(format!("{name}_{lang}"));
                results.extend_harness(&mut h);
            }
        }
    }

    /// Renders `view` and compares it with the PNG called `name`.
    fn render(
        view: SettingsView,
        theme: egui::Theme,
        lang: &str,
        size: egui::Vec2,
        name: &str,
        results: &mut egui_kittest::SnapshotResults,
    ) {
        let l = loader_for(&[lang.parse().unwrap()]);
        let mut h = Harness::builder()
            .with_size(size)
            .with_theme(theme)
            .wgpu()
            .build_ui_state(
                move |ui, view| {
                    theme::install(ui.ctx());
                    fonts::install_bundled_fonts(ui.ctx());
                    view.show(ui, &l);
                },
                view,
            );
        h.run();
        h.snapshot(name);
        results.extend_harness(&mut h);
    }

    fn page(snap: Snapshot, section: Section) -> SettingsView {
        let mut view = SettingsView::new(snap);
        view.section = section;
        view
    }

    fn connected_tablet() -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            serial: "R52X204YJNW".into(),
            state: DeviceState::Ready,
            model: Some("SM-X910".into()),
        }]
    }

    fn streaming() -> Snapshot {
        let HostStatus::Streaming(mut s) = live(Some(stats())) else {
            unreachable!()
        };
        s.devices = connected_tablet();
        snapshot(HostStatus::Streaming(s))
    }

    fn waiting() -> Snapshot {
        snapshot(HostStatus::Waiting {
            devices: vec![DeviceInfo {
                serial: "R52X204YJNW".into(),
                state: DeviceState::Unauthorized,
                model: Some("SM-X910".into()),
            }],
        })
    }

    fn driver_snapshot() -> Snapshot {
        let mut snap = snapshot(HostStatus::Stopped);
        snap.vdd = Some(VddInfo {
            present: true,
            owned: true,
            driver_dir: Some(r"C:\Program Files\ezdisplay\driver".into()),
        });
        snap
    }

    fn blocked_input() -> Snapshot {
        blocked(InputAccess::Denied)
    }

    fn windows_streaming() -> Snapshot {
        let mut snap = streaming();
        snap.windows = true;
        snap
    }

    /// One snapshot per page, matching the mockups (`01` to `07`), the minimum size and the
    /// longest languages.
    fn page_snapshots(results: &mut egui_kittest::SnapshotResults) {
        let default = egui::vec2(820.0, 560.0);
        let (light, dark) = (egui::Theme::Light, egui::Theme::Dark);
        // The mockups, in English.
        let mut connection = SettingsView::new(streaming());
        connection.section = Section::Connection;
        connection.advanced_open = true;
        let mut cases = vec![
            (
                page(streaming(), Section::Status),
                dark,
                "page_status_streaming_dark",
            ),
            (
                page(waiting(), Section::Status),
                light,
                "page_status_waiting_light",
            ),
            (
                page(windows_streaming(), Section::Display),
                light,
                "page_display_light",
            ),
            (
                page(blocked_input(), Section::Input),
                dark,
                "page_input_warning_dark",
            ),
            (connection, light, "page_connection_light"),
            (
                page(snapshot(HostStatus::Stopped), Section::General),
                dark,
                "page_general_dark",
            ),
            (
                page(driver_snapshot(), Section::Driver),
                light,
                "page_driver_light",
            ),
        ];
        for (view, theme, name) in cases.drain(..) {
            render(view, theme, "en", default, name, results);
        }
        // The whole of a long page.
        render(
            page(streaming(), Section::Status),
            light,
            "en",
            egui::vec2(820.0, 900.0),
            "page_status_tall_light",
            results,
        );
        // The minimum size.
        let min = egui::vec2(640.0, 420.0);
        render(
            page(streaming(), Section::Status),
            light,
            "en",
            min,
            "page_status_min_light",
            results,
        );
        render(
            page(windows_streaming(), Section::Display),
            light,
            "en",
            min,
            "page_display_min_light",
            results,
        );
        render(
            page(blocked_input(), Section::Input),
            dark,
            "en",
            min,
            "page_input_min_dark",
            results,
        );
        // Longer strings must not clip.
        for lang in ["es", "de"] {
            for (snap, section, name) in [
                (streaming(), Section::Status, "status"),
                (windows_streaming(), Section::Display, "display"),
                (blocked_input(), Section::Input, "input_warning"),
                (streaming(), Section::Connection, "connection"),
                (driver_snapshot(), Section::Driver, "driver"),
            ] {
                render(
                    page(snap, section),
                    light,
                    lang,
                    default,
                    &format!("page_{name}_{lang}"),
                    results,
                );
            }
            render(
                page(streaming(), Section::General),
                dark,
                lang,
                min,
                &format!("page_general_min_{lang}"),
                results,
            );
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
        hdr_snapshots(&mut results);
        page_snapshots(&mut results);
    }
}
