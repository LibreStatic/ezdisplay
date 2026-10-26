//! GNOME provider: a virtual monitor from Mutter's own `org.gnome.Mutter.ScreenCast` service
//! (`RecordVirtual`), captured from the user's PipeWire daemon. Without that service on the bus
//! (or to capture an existing monitor) it falls back to the ScreenCast portal.
//!
//! Mutter sizes a virtual monitor from the PipeWire format negotiation (the size range the
//! capture offers, `pw.rs`), so there is no mode to set: [`VirtualOutputProvider::settle`]
//! reads the result back from `DisplayConfig`.

use crate::backend;
use crate::gnome_dc::{self, DisplayState};
use crate::portal::{PortalCapture, SourceKind};
use crate::provider::{FrameSource, OpenedOutput, OutputRequest, VirtualOutputProvider};
use crate::pw::Target;
use ezd_host_core::{DisplayMode, OutputInfo, SourceError};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, Value};

const SERVICE: &str = "org.gnome.Mutter.ScreenCast";
const ROOT: &str = "/org/gnome/Mutter/ScreenCast";
const SESSION_IFACE: &str = "org.gnome.Mutter.ScreenCast.Session";
const STREAM_IFACE: &str = "org.gnome.Mutter.ScreenCast.Stream";
/// `cursor-mode`: the cursor travels as stream metadata, not painted into the frames.
const CURSOR_METADATA: u32 = 2;
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

/// Provider for Mutter.
#[derive(Default)]
pub struct GnomeProvider {
    /// After `open`: the session bus connection and the monitors that existed before the session.
    /// Mutter adds the virtual monitor to `DisplayConfig` only once the stream has connected, so
    /// it is looked for in `settle`.
    watch: Option<(Connection, Option<DisplayState>)>,
}

impl GnomeProvider {
    /// A provider that has not opened anything yet.
    pub fn new() -> GnomeProvider {
        GnomeProvider::default()
    }
}

/// A Mutter ScreenCast session; stopping it removes the virtual monitor.
struct MutterSession {
    session: Proxy<'static>,
}

impl Drop for MutterSession {
    fn drop(&mut self) {
        if let Err(e) = self.session.call::<_, _, ()>("Stop", &()) {
            tracing::debug!("stopping the Mutter ScreenCast session: {e}");
        }
    }
}

/// Arguments of `Session.RecordVirtual`.
fn record_virtual_args() -> HashMap<&'static str, Value<'static>> {
    HashMap::from([
        ("cursor-mode", Value::U32(CURSOR_METADATA)),
        ("is-platform", Value::Bool(true)),
    ])
}

/// The output description of `connector` in `state`.
pub fn output_info(state: &DisplayState, connector: &str) -> Option<OutputInfo> {
    let mode = state.monitor(connector)?.current_mode()?;
    let origin = state.logical_of(connector).map_or((0, 0), |l| (l.x, l.y));
    Some(OutputInfo {
        name: connector.to_string(),
        mode: DisplayMode {
            width: mode.width,
            height: mode.height,
            refresh_mhz: (mode.refresh_hz * 1000.0).round() as u32,
        },
        origin,
        adapter: None,
    })
}

fn map_zbus<T>(what: &str, r: zbus::Result<T>) -> Result<T, SourceError> {
    r.map_err(|e| SourceError::msg(format!("Mutter ScreenCast {what}: {e}")))
}

impl GnomeProvider {
    fn open_mutter(&mut self) -> Result<OpenedOutput, SourceError> {
        let conn = map_zbus("connect", Connection::session())?;
        let before = gnome_dc::get_state(&conn).ok();
        let sc = map_zbus("proxy", Proxy::new(&conn, SERVICE, ROOT, SERVICE))?;
        let path: OwnedObjectPath = map_zbus(
            "CreateSession",
            sc.call("CreateSession", &(HashMap::<&str, Value>::new(),)),
        )?;
        let session = map_zbus(
            "session proxy",
            Proxy::new(&conn, SERVICE, path, SESSION_IFACE),
        )?;
        // Stops the session on every early return below.
        let guard = MutterSession {
            session: session.clone(),
        };
        let stream_path: OwnedObjectPath = map_zbus(
            "RecordVirtual",
            session.call("RecordVirtual", &(record_virtual_args(),)),
        )?;
        let stream = map_zbus(
            "stream proxy",
            Proxy::new(&conn, SERVICE, stream_path, STREAM_IFACE),
        )?;
        // Subscribe before Start: the node id comes in a signal sent while Start runs.
        let signals = map_zbus("subscribe", stream.receive_signal("PipeWireStreamAdded"))?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ezd-mutter-signal".into())
            .spawn(move || {
                for msg in signals {
                    if let Ok(node) = msg.body().deserialize::<u32>() {
                        let _ = tx.send(node);
                        break;
                    }
                }
            })
            .map_err(|e| SourceError::msg(format!("cannot start the signal thread: {e}")))?;
        map_zbus("Start", session.call::<_, _, ()>("Start", &()))?;
        let node = rx.recv_timeout(STREAM_TIMEOUT).map_err(|_| {
            SourceError::msg("Mutter did not report a PipeWire node for the virtual monitor")
        })?;
        tracing::info!(node, "Mutter ScreenCast virtual monitor started");

        self.watch = Some((conn, before));
        Ok(OpenedOutput {
            frames: FrameSource::PipeWire {
                fd: None,
                target: Target::Node(node),
            },
            info: None,
            guard: Box::new(guard),
        })
    }

    fn open_portal(&mut self, req: &OutputRequest) -> Result<OpenedOutput, SourceError> {
        let timeout = std::env::var("EZD_PORTAL_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120);
        let mut portal = PortalCapture::open(req.kind, Duration::from_secs(timeout))
            .map_err(SourceError::msg)?;
        tracing::info!(node = portal.node_id, size = ?portal.size, "screencast portal session open");
        let frames = FrameSource::PipeWire {
            fd: portal.fd.take(),
            target: Target::Node(portal.node_id),
        };
        Ok(OpenedOutput {
            frames,
            info: None,
            guard: Box::new(portal),
        })
    }
}

impl VirtualOutputProvider for GnomeProvider {
    fn name(&self) -> &'static str {
        "gnome"
    }

    fn open(&mut self, req: &OutputRequest) -> Result<OpenedOutput, SourceError> {
        self.watch = None;
        if req.kind == SourceKind::Virtual && backend::probe_bus().mutter {
            self.open_mutter()
        } else {
            tracing::info!(
                "using the ScreenCast portal (no Mutter ScreenCast service, or a monitor was asked for)"
            );
            self.open_portal(req)
        }
    }

    fn settle(&mut self, negotiated: (u32, u32)) -> Option<OutputInfo> {
        let (conn, before) = self.watch.as_ref()?;
        let Some(before) = before else {
            tracing::warn!("DisplayConfig was unreadable before the session; no output info");
            return None;
        };
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let mut last = None;
        loop {
            if let Ok(state) = gnome_dc::get_state(conn)
                && let Some(m) = state.new_monitor(before)
            {
                let size = m.current_mode().map(|m| (m.width, m.height));
                let connector = m.connector.clone();
                last = output_info(&state, &connector);
                if size == Some(negotiated) {
                    tracing::info!("{connector} is {}x{}", negotiated.0, negotiated.1);
                    break;
                }
            }
            if Instant::now() >= deadline {
                match &last {
                    Some(o) => tracing::warn!(
                        "Mutter's virtual monitor {} is {}x{} but the stream negotiated {}x{}",
                        o.name,
                        o.mode.width,
                        o.mode.height,
                        negotiated.0,
                        negotiated.1
                    ),
                    None => tracing::warn!("the virtual monitor never showed up in DisplayConfig"),
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        last
    }
}

/// Mutter's part of `ezd-host diag`: version and the two D-Bus services the backend uses.
pub fn diag() -> serde_json::Value {
    let version = std::process::Command::new("mutter")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty());
    let probe = || -> serde_json::Value {
        let Ok(conn) = Connection::session() else {
            return serde_json::json!({ "screencast": false, "display_config": false });
        };
        let screencast = Proxy::new(&conn, SERVICE, ROOT, SERVICE)
            .ok()
            .and_then(|p| p.get_property::<i32>("Version").ok());
        let display = gnome_dc::get_state(&conn);
        serde_json::json!({
            "screencast": screencast.is_some(),
            "screencast_version": screencast,
            "display_config": display.is_ok(),
            "monitors": display.as_ref().ok().map(|s| s.monitors.len()),
            "display_config_error": display.err(),
        })
    };
    // Own thread: callers may be inside a tokio runtime.
    let mut v = std::thread::spawn(probe)
        .join()
        .unwrap_or_else(|_| serde_json::json!({ "error": "D-Bus probe panicked" }));
    if let Some(o) = v.as_object_mut() {
        o.insert("mutter_version".into(), serde_json::json!(version));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::LinuxSource;
    use ezd_host_core::{CursorSink, PlatformOptions, VideoSource};
    use ezd_proto::{Codec, StreamConfig};

    #[test]
    fn record_virtual_requests_cursor_metadata_on_a_platform_monitor() {
        let a = record_virtual_args();
        assert_eq!(a["cursor-mode"], Value::U32(2));
        assert_eq!(a["is-platform"], Value::Bool(true));
    }

    /// Captures a few frames from a virtual monitor of a headless Mutter and checks the output
    /// info, the cursor metadata request and the touch / pen gsettings mapping. Run it with
    /// `tools/gnome-headless-test.sh`: it starts Mutter on a private D-Bus session (keeping
    /// `XDG_RUNTIME_DIR`, for the user's PipeWire daemon) plus a Wayland client that keeps drawing,
    /// because Mutter only sends frames when the screen changes.
    ///
    /// `EZD_GNOME_TEST_ENCODER` picks the encoder (default: the first offered; `libx264` is
    /// always there) and `EZD_NO_DMABUF=1` forces shared-memory capture.
    #[test]
    fn headless_mutter_frames() {
        if std::env::var_os("EZD_GNOME_TEST").is_none_or(|v| v != "1") {
            eprintln!("skipped: set EZD_GNOME_TEST=1 inside a headless Mutter session");
            return;
        }
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let mut src = LinuxSource::new(&PlatformOptions::default(), Box::new(GnomeProvider::new()));
        let encoders = src.encoders();
        let wanted = std::env::var("EZD_GNOME_TEST_ENCODER").ok();
        let enc = encoders
            .iter()
            .find(|e| wanted.as_deref().is_none_or(|w| e.name == w) && e.codec == Codec::H264)
            .expect("an H.264 encoder")
            .name
            .clone();
        let cfg = StreamConfig {
            width: 1280,
            height: 720,
            refresh_mhz: 60_000,
            codec: Codec::H264,
            bitrate_kbps: 8000,
            encoder_name: enc,
        };
        src.start(&cfg, CursorSink::null()).expect("start");
        let out = src.output().expect("output info");
        eprintln!("output: {out:?}");
        assert_eq!((out.mode.width, out.mode.height), (1280, 720));
        // Input mapping: the gsettings keys name the new monitor, then go back to the default.
        {
            use crate::gnome_input::{GnomeDesktop, get_args, tablet_schema, touch_schema};
            use crate::uinput_sink::DesktopInput;
            let desktop = GnomeDesktop::new();
            let layout = desktop.outputs().expect("layout");
            assert!(layout.iter().any(|o| o.name == out.name));
            let mapped = desktop.map_devices(&[("touch", "event0"), ("pen", "event1")], &out.name);
            assert!(
                mapped.results.iter().all(Result::is_ok),
                "{:?}",
                mapped.results
            );
            let get = |schema: String| {
                let o = std::process::Command::new("gsettings")
                    .args(get_args(&schema))
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            };
            let want = format!("['MetaVendor', 'Virtual remote monitor', '{}']", {
                let state =
                    crate::gnome_dc::get_state(&zbus::blocking::Connection::session().unwrap())
                        .unwrap();
                state.monitor(&out.name).unwrap().serial.clone()
            });
            assert_eq!(get(touch_schema()), want);
            assert_eq!(get(tablet_schema()), want);
            drop(mapped.guard);
            assert_eq!(get(touch_schema()), "['', '', '']");
            assert_eq!(get(tablet_schema()), "['', '', '']");
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut frames = Vec::new();
        while frames.len() < 5 && Instant::now() < deadline {
            if let Some(f) = src.next_frame(Duration::from_millis(500)).expect("frame") {
                frames.push(f);
            }
        }
        src.stop();
        eprintln!("frames: {}", frames.len());
        assert!(frames.len() >= 3, "got {} frames", frames.len());
        assert!(frames[0].keyframe && frames[0].codec_config.is_some());
    }
}
