//! Input plumbing: fake client scripts -> host session -> recording backend, over an in-memory link.

use ezd_fake_client::{
    ClientEnd, ClientReport, FakeClientConfig, InputScript, run_fake_client, script_messages,
};
use ezd_host_core::testing::{InputCall, Recording, RecordingBackend, ScriptedSource};
use ezd_host_core::{
    DisplayMode, EndReason, OutputInfo, SessionOptions, SessionSummary, run_host_session_with,
};
use ezd_link::Link;
use ezd_proto::{ClientOptions, InputStatus, Key, Message, PenSample, Prefs};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;

fn output() -> OutputInfo {
    OutputInfo {
        name: "Virtual-1".into(),
        mode: DisplayMode {
            width: 1280,
            height: 720,
            refresh_mhz: 60_000,
        },
        origin: (1920, 0),
        adapter: Some("/dev/dri/renderD128".into()),
    }
}

fn client_cfg(script: InputScript) -> FakeClientConfig {
    FakeClientConfig {
        width: 1280,
        height: 720,
        refresh_rates_mhz: vec![60_000],
        input_script: Some(script),
        stop_after_script: true,
        ping_interval: Duration::from_millis(20),
        max_duration: Some(Duration::from_secs(20)),
        verify_synthetic: false,
        ..Default::default()
    }
}

async fn run(client: FakeClientConfig, opts: SessionOptions) -> (ClientReport, SessionSummary) {
    let (host_link, client_link) = Link::duplex_pair(1 << 20);
    let src = ScriptedSource::new(Duration::from_millis(10), 200).with_output(output());
    let opts = SessionOptions {
        ping_interval: Duration::from_millis(20),
        ..opts
    };
    let host = tokio::spawn(run_host_session_with(
        host_link,
        Box::new(src),
        Prefs::default(),
        opts,
    ));
    let report = timeout(
        Duration::from_secs(20),
        run_fake_client(client_link, client),
    )
    .await
    .expect("client timed out")
    .unwrap();
    let summary = timeout(Duration::from_secs(10), host)
        .await
        .expect("host timed out")
        .unwrap()
        .unwrap();
    (report, summary)
}

fn opts_with(backend: &RecordingBackend) -> SessionOptions {
    SessionOptions {
        input: Some(Arc::new(backend.clone())),
        ..Default::default()
    }
}

fn status() -> InputStatus {
    InputStatus {
        caps: InputStatus::PEN | InputStatus::TOUCH,
        note: "recording".into(),
    }
}

/// The recorded calls (without `ReleaseAll`) as messages with zeroed timestamps.
fn injected(rec: &Mutex<Recording>) -> (Vec<Message>, bool) {
    let rec = rec.lock().unwrap();
    let released = rec.calls.last() == Some(&InputCall::ReleaseAll);
    let msgs = rec
        .calls
        .iter()
        .filter_map(|c| {
            Some(match c {
                InputCall::Pen(s) => Message::Pen(PenSample { t_us: 0, ..*s }),
                InputCall::Touch(f) => Message::Touch(ezd_proto::TouchFrame {
                    t_us: 0,
                    contacts: f.contacts.clone(),
                }),
                InputCall::Button(b) => {
                    Message::PointerButton(ezd_proto::PointerButton { t_us: 0, ..*b })
                }
                InputCall::Scroll(s) => Message::Scroll(ezd_proto::Scroll { t_us: 0, ..*s }),
                InputCall::Key(k) => Message::Key(Key { t_us: 0, ..*k }),
                InputCall::Keyboard(_) | InputCall::ReleaseAll => return None,
            })
        })
        .collect();
    (msgs, released)
}

fn expected(script: InputScript) -> Vec<Message> {
    script_messages(script)
        .into_iter()
        .map(|(_, m)| m)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drag_reaches_the_sink_in_order() {
    let backend = RecordingBackend::new(status());
    let (report, summary) = run(client_cfg(InputScript::Drag), opts_with(&backend)).await;

    let want = expected(InputScript::Drag);
    let (got, released) = injected(&backend.recording);
    assert_eq!(report.inputs_sent, want.len() as u64);
    assert_eq!(got.len(), want.len());
    assert_eq!(
        got, want,
        "scripted pen sequence arrives intact and in order"
    );
    assert!(matches!(&got[0], Message::Pen(p) if p.flags == PenSample::IN_RANGE));
    assert!(matches!(got.last(), Some(Message::Pen(p)) if p.flags == 0));
    assert!(released, "release_all ran last");

    assert_eq!(report.input_status, Some(status()));
    assert_eq!(report.client_options, Some(ClientOptions { flags: 0 }));
    assert_eq!(report.end, ClientEnd::FramesReached);

    let rec = backend.recording.lock().unwrap();
    let target = rec.target.as_ref().unwrap();
    assert_eq!(target.output, Some(output()));
    assert_eq!(target.client, client_cfg(InputScript::Drag).caps());
    assert_eq!(target.stream, report.stream);
    drop(rec);

    assert_eq!(summary.input_events, want.len() as u64);
    assert!(summary.input_latency_p50_us.is_some_and(|us| us < 100_000));
    assert!(summary.input_latency_p95_us.is_some());
    assert_eq!(summary.end, EndReason::ClientBye("client done".into()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn touch_and_scroll_scripts_arrive_intact() {
    for script in [InputScript::Touch2, InputScript::Tap, InputScript::Scroll] {
        let backend = RecordingBackend::new(status());
        let (_report, summary) = run(client_cfg(script), opts_with(&backend)).await;
        let want = expected(script);
        let (got, released) = injected(&backend.recording);
        assert_eq!(got, want, "{script:?}");
        assert!(released, "{script:?}");
        assert_eq!(summary.input_events, want.len() as u64, "{script:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyboard_state_reaches_the_sink() {
    let backend = RecordingBackend::new(status());
    let client = FakeClientConfig {
        keyboard_attached: Some(true),
        ..client_cfg(InputScript::Tap)
    };
    run(client, opts_with(&backend)).await;
    let rec = backend.recording.lock().unwrap();
    assert!(rec.calls.contains(&InputCall::Keyboard(true)));
    assert_eq!(rec.calls.last(), Some(&InputCall::ReleaseAll));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyboard_state_is_not_sent_by_an_older_client_minor() {
    let backend = RecordingBackend::new(status());
    let client = FakeClientConfig {
        keyboard_attached: Some(true),
        proto_minor: Some(ezd_proto::MINOR_KEYBOARD - 1),
        ..client_cfg(InputScript::Tap)
    };
    run(client, opts_with(&backend)).await;
    let rec = backend.recording.lock().unwrap();
    assert!(
        !rec.calls
            .iter()
            .any(|c| matches!(c, InputCall::Keyboard(_)))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telemetry_flag_reaches_the_client() {
    let backend = RecordingBackend::new(status());
    let opts = SessionOptions {
        telemetry: true,
        ..opts_with(&backend)
    };
    let (report, _) = run(client_cfg(InputScript::Tap), opts).await;
    assert_eq!(
        report.client_options,
        Some(ClientOptions {
            flags: ClientOptions::TELEMETRY
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_input_is_announced_and_input_is_dropped() {
    let (report, summary) = run(client_cfg(InputScript::Drag), SessionOptions::default()).await;
    assert_eq!(
        report.input_status,
        Some(InputStatus {
            caps: 0,
            note: "input is disabled on the host".into()
        })
    );
    assert_eq!(report.client_options, Some(ClientOptions { flags: 0 }));
    assert!(report.inputs_sent > 0);
    assert_eq!(summary.input_events, 0);
    assert_eq!(summary.end, EndReason::ClientBye("client done".into()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backend_failure_is_reported_to_the_client() {
    let backend = RecordingBackend::failing("x");
    let (report, summary) = run(client_cfg(InputScript::Drag), opts_with(&backend)).await;
    assert_eq!(
        report.input_status,
        Some(InputStatus {
            caps: 0,
            note: "x".into()
        })
    );
    assert_eq!(summary.input_events, 0);
    assert_eq!(summary.end, EndReason::ClientBye("client done".into()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_client_gets_no_input_messages() {
    let backend = RecordingBackend::new(status());
    let mut cfg = client_cfg(InputScript::Tap);
    cfg.proto_minor = Some(2);
    let (report, summary) = run(cfg, opts_with(&backend)).await;
    assert_eq!(report.input_status, None);
    assert_eq!(report.client_options, None);
    assert_eq!(summary.input_events, 0);
    assert!(backend.recording.lock().unwrap().target.is_none());
    assert_eq!(summary.end, EndReason::ClientBye("client done".into()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keys_reach_the_sink_in_order_when_announced() {
    let st = InputStatus {
        caps: InputStatus::PEN | InputStatus::KEYS,
        note: String::new(),
    };
    let backend = RecordingBackend::new(st.clone());
    let (report, summary) = run(client_cfg(InputScript::Keys), opts_with(&backend)).await;
    assert_eq!(report.input_status, Some(st));
    let want = expected(InputScript::Keys);
    let (got, released) = injected(&backend.recording);
    assert_eq!(got, want, "keys and pen samples arrive intact and in order");
    assert_eq!(report.inputs_sent, want.len() as u64);
    assert_eq!(summary.input_events, want.len() as u64);
    assert!(released);
    let ctrl_down = got
        .iter()
        .position(|m| matches!(m, Message::Key(k) if k.usage == Key::LEFT_CTRL && k.pressed));
    let contact = got
        .iter()
        .position(|m| matches!(m, Message::Pen(p) if p.flags & PenSample::CONTACT != 0));
    assert!(ctrl_down.unwrap() < contact.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keys_are_dropped_when_the_host_lacks_the_capability() {
    let backend = RecordingBackend::new(status()); // PEN | TOUCH, no KEYS
    let (_report, _summary) = run(client_cfg(InputScript::Keys), opts_with(&backend)).await;
    let (got, _) = injected(&backend.recording);
    assert!(!got.is_empty());
    assert!(got.iter().all(|m| !matches!(m, Message::Key(_))));
}
