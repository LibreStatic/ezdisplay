//! Hyprland: a headless virtual output through the command socket
//! (`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`), captured directly over
//! Wayland (`wl_capture.rs`), and the input mapping of the touch, pen and pointer devices.
//!
//! Every request is one connection: the command is written, the reply read until the compositor
//! closes it. `output create headless NAME` makes the output, a `keyword monitor` rule gives it
//! its mode, and `output remove NAME` destroys it when the guard drops.

use crate::kscreen::KOutput;
use crate::portal::SourceKind;
use crate::provider::{FrameSource, OpenedOutput, OutputRequest, VirtualOutputProvider, rate_text};
use crate::uinput_sink::{DesktopInput, MappedDevices, PEN_NAME, POINTER_NAME, TOUCH_NAME};
use ezd_host_core::{DisplayMode, OutputInfo, SourceError};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(3);
/// Names of the virtual outputs: `ezdisplay-1`, `ezdisplay-2`, ...
const OUTPUT_PREFIX: &str = "ezdisplay-";

/// The command socket for `signature` (`$HYPRLAND_INSTANCE_SIGNATURE`). Hyprland moved it from
/// `/tmp/hypr` to the runtime directory; the old place is the fallback.
pub fn socket_path_for(runtime_dir: Option<&str>, signature: &str) -> PathBuf {
    match runtime_dir.filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d)
            .join("hypr")
            .join(signature)
            .join(".socket.sock"),
        None => PathBuf::from("/tmp/hypr")
            .join(signature)
            .join(".socket.sock"),
    }
}

fn socket_path() -> Result<PathBuf, String> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or("HYPRLAND_INSTANCE_SIGNATURE is not set (is Hyprland running in this session?)")?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok();
    let primary = socket_path_for(runtime.as_deref(), &sig);
    if primary.exists() {
        return Ok(primary);
    }
    let legacy = socket_path_for(None, &sig);
    Ok(if legacy.exists() { legacy } else { primary })
}

/// Sends one command and returns the reply text.
pub fn request(command: &str) -> Result<String, String> {
    let path = socket_path()?;
    let mut s = UnixStream::connect(&path)
        .map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    s.write_all(command.as_bytes())
        .map_err(|e| format!("Hyprland IPC write: {e}"))?;
    let mut reply = String::new();
    s.read_to_string(&mut reply)
        .map_err(|e| format!("Hyprland IPC read: {e}"))?;
    Ok(reply)
}

/// Runs a command that answers `ok` on success.
pub fn run(command: &str) -> Result<(), String> {
    expect_ok(command, &request(command)?)
}

/// Checks the reply of a command: `ok`, or an error text.
pub fn expect_ok(command: &str, reply: &str) -> Result<(), String> {
    match reply.trim() {
        "ok" => Ok(()),
        "" => Err(format!("Hyprland `{command}`: empty reply")),
        other => Err(format!("Hyprland `{command}`: {other}")),
    }
}

/// A monitor as `j/monitors` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct HyprMonitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub disabled: bool,
}

/// Parses the reply of `j/monitors`.
pub fn parse_monitors(json: &str) -> Result<Vec<HyprMonitor>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad monitors reply: {e}"))?;
    let items = v.as_array().ok_or("bad monitors reply: not an array")?;
    Ok(items
        .iter()
        .filter_map(|m| {
            let int = |k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
            Some(HyprMonitor {
                name: m.get("name")?.as_str()?.to_string(),
                width: int("width") as u32,
                height: int("height") as u32,
                refresh_hz: m.get("refreshRate").and_then(Value::as_f64).unwrap_or(0.0),
                x: int("x") as i32,
                y: int("y") as i32,
                scale: m.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
                disabled: m.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

/// The first `ezdisplay-N` (N from 1) that no monitor uses.
pub fn free_output_name(monitors: &[HyprMonitor]) -> String {
    (1..)
        .map(|n| format!("{OUTPUT_PREFIX}{n}"))
        .find(|n| !monitors.iter().any(|m| &m.name == n))
        .unwrap_or_else(|| format!("{OUTPUT_PREFIX}1"))
}

/// `keyword monitor NAME,WxH@HZ,auto,1`: the mode, automatic placement, scale 1.
pub fn monitor_rule(name: &str, width: u32, height: u32, refresh_mhz: u32) -> String {
    format!(
        "keyword monitor {name},{width}x{height}@{},auto,1",
        rate_text(refresh_mhz.max(1000))
    )
}

/// The `version` of a `j/version` reply.
pub fn parse_version(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    let version = v
        .get("version")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let tag = v
        .get("tag")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    version.or(tag).map(str::to_string)
}

fn monitors() -> Result<Vec<HyprMonitor>, String> {
    parse_monitors(&request("j/monitors")?)
}

/// The output description of `name`.
pub fn output_info(monitors: &[HyprMonitor], name: &str) -> Option<OutputInfo> {
    let m = monitors.iter().find(|m| m.name == name)?;
    Some(OutputInfo {
        name: m.name.clone(),
        mode: DisplayMode {
            width: m.width,
            height: m.height,
            refresh_mhz: (m.refresh_hz * 1000.0).round() as u32,
        },
        origin: (m.x, m.y),
        adapter: None,
    })
}

/// Removes a virtual output when dropped.
struct OutputGuard {
    name: String,
}

impl Drop for OutputGuard {
    fn drop(&mut self) {
        match run(&format!("output remove {}", self.name)) {
            Ok(()) => tracing::info!(output = %self.name, "virtual output removed"),
            Err(e) => tracing::warn!("cannot remove {}: {e}", self.name),
        }
    }
}

/// Provider for Hyprland.
#[derive(Default)]
pub struct HyprlandProvider;

impl VirtualOutputProvider for HyprlandProvider {
    fn name(&self) -> &'static str {
        "hyprland"
    }

    fn open(&mut self, req: &OutputRequest) -> Result<OpenedOutput, SourceError> {
        let existing = monitors().map_err(SourceError::msg)?;
        if req.kind == SourceKind::Monitor {
            let name = req
                .output_name
                .clone()
                .ok_or_else(|| SourceError::msg("no output name to capture"))?;
            let info = output_info(&existing, &name).ok_or_else(|| {
                SourceError::msg(format!(
                    "Hyprland has no monitor called {name} (monitors: {})",
                    existing
                        .iter()
                        .map(|m| m.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            return Ok(OpenedOutput {
                frames: FrameSource::Wayland { output: name },
                info: Some(info),
                guard: Box::new(()),
            });
        }

        let name = free_output_name(&existing);
        run(&format!("output create headless {name}")).map_err(SourceError::msg)?;
        tracing::info!(output = %name, "Hyprland virtual output created");
        // From here on the guard removes the output on every early return.
        let guard = OutputGuard { name: name.clone() };
        run(&monitor_rule(&name, req.width, req.height, req.refresh_mhz))
            .map_err(SourceError::msg)?;
        let mut info = None;
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        loop {
            let all = monitors().map_err(SourceError::msg)?;
            info = output_info(&all, &name).or(info);
            if info
                .as_ref()
                .is_some_and(|i| (i.mode.width, i.mode.height) == (req.width, req.height))
            {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!(output = %name, ?info, "the mode was not applied in time");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(OpenedOutput {
            frames: FrameSource::Wayland { output: name },
            info,
            guard: Box::new(guard),
        })
    }
}

/// Hyprland's name of an input device: the kernel name in lowercase, spaces as dashes.
pub fn device_name(kernel_name: &str) -> String {
    kernel_name.trim().to_lowercase().replace(' ', "-")
}

/// The Hyprland device behind a label (`touch`, `pen`, `pointer`).
pub fn device_for(label: &str) -> Option<String> {
    let name = match label {
        "touch" => TOUCH_NAME,
        "pen" => PEN_NAME,
        "pointer" => POINTER_NAME,
        _ => return None,
    };
    Some(device_name(name))
}

/// `keyword device[NAME]:output OUTPUT`.
pub fn map_command(device: &str, output: &str) -> String {
    format!("keyword device[{device}]:output {output}")
}

/// The `output` option's default: no mapping.
const AUTO_OUTPUT: &str = "[[Auto]]";

/// Every device name in a `j/devices` reply (mice, keyboards, tablets, touch devices).
pub fn parse_device_names(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    if let Some(groups) = v.as_object() {
        for list in groups.values().filter_map(Value::as_array) {
            names.extend(
                list.iter()
                    .filter_map(|d| d.get("name").and_then(Value::as_str))
                    .map(str::to_string),
            );
        }
    }
    names
}

/// Puts the `output` option of the devices back when dropped.
struct Restore {
    devices: Vec<String>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        for d in &self.devices {
            if let Err(e) = run(&map_command(d, AUTO_OUTPUT)) {
                tracing::debug!("cannot clear the output mapping of {d}: {e}");
            }
        }
    }
}

/// Input mapping for Hyprland: touch, tablet and absolute pointer devices each get an `output`.
#[derive(Default)]
pub struct HyprlandDesktop;

impl DesktopInput for HyprlandDesktop {
    fn map_devices(&self, devices: &[(&str, &str)], output: &str) -> MappedDevices {
        let known = request("j/devices")
            .map(|r| parse_device_names(&r))
            .unwrap_or_default();
        let mut restore = Restore {
            devices: Vec::new(),
        };
        let results = devices
            .iter()
            .map(|(label, _)| {
                let dev = device_for(label).ok_or_else(|| format!("unknown device {label}"))?;
                if !known.is_empty() && !known.contains(&dev) {
                    tracing::debug!(device = %dev, "Hyprland does not list the device (yet)");
                }
                run(&map_command(&dev, output))?;
                tracing::info!(device = %dev, %output, "Hyprland output mapping set");
                restore.devices.push(dev);
                Ok(())
            })
            .collect();
        MappedDevices {
            results,
            guard: (!restore.devices.is_empty()).then(|| Box::new(restore) as Box<dyn Send>),
        }
    }

    fn outputs(&self) -> Option<Vec<KOutput>> {
        None
    }

    fn maps_pointer(&self) -> bool {
        true
    }
}

/// Hyprland's part of `ezd-host diag`.
pub fn diag() -> Value {
    let version = request("j/version").ok().and_then(|r| parse_version(&r));
    match monitors() {
        Ok(ms) => serde_json::json!({
            "hyprland_version": version,
            "monitors": ms.iter().map(|m| serde_json::json!({
                "name": m.name,
                "size": [m.width, m.height],
                "refresh_hz": m.refresh_hz,
                "position": [m.x, m.y],
                "scale": m.scale,
                "disabled": m.disabled,
            })).collect::<Vec<_>>(),
        }),
        Err(e) => serde_json::json!({ "hyprland_version": version, "error": e }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape of `hyprctl monitors -j` (Hyprland 0.4x), trimmed to the fields that matter.
    const MONITORS: &str = r#"[
        {"id": 0, "name": "eDP-1", "description": "Panel", "make": "BOE", "model": "0x0a1c",
         "width": 2560, "height": 1600, "physicalWidth": 290, "physicalHeight": 180,
         "refreshRate": 165.00400, "x": 0, "y": 0,
         "activeWorkspace": {"id": 1, "name": "1"}, "scale": 1.60, "transform": 0,
         "focused": true, "dpmsStatus": true, "vrr": false, "disabled": false},
        {"id": 1, "name": "ezdisplay-1", "description": "", "make": "", "model": "",
         "width": 1280, "height": 720, "refreshRate": 60.00000, "x": 1600, "y": 0,
         "scale": 1.00, "transform": 0, "focused": false, "disabled": false}
    ]"#;

    #[test]
    fn socket_locations() {
        assert_eq!(
            socket_path_for(Some("/run/user/1000"), "abc_123_9"),
            PathBuf::from("/run/user/1000/hypr/abc_123_9/.socket.sock")
        );
        assert_eq!(
            socket_path_for(None, "abc"),
            PathBuf::from("/tmp/hypr/abc/.socket.sock")
        );
        assert_eq!(
            socket_path_for(Some(""), "abc"),
            socket_path_for(None, "abc")
        );
    }

    #[test]
    fn replies() {
        assert_eq!(expect_ok("c", "ok"), Ok(()));
        assert_eq!(expect_ok("c", "ok\n"), Ok(()));
        assert!(
            expect_ok("c", "Invalid monitor")
                .unwrap_err()
                .contains("Invalid monitor")
        );
        assert!(expect_ok("c", "").is_err());
    }

    #[test]
    fn parses_monitors() {
        let ms = parse_monitors(MONITORS).unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].name, "eDP-1");
        assert_eq!((ms[0].width, ms[0].height, ms[0].scale), (2560, 1600, 1.6));
        let i = output_info(&ms, "ezdisplay-1").unwrap();
        assert_eq!(i.origin, (1600, 0));
        assert_eq!(
            (i.mode.width, i.mode.height, i.mode.refresh_mhz),
            (1280, 720, 60_000)
        );
        assert_eq!(output_info(&ms, "eDP-1").unwrap().mode.refresh_mhz, 165_004);
        assert!(output_info(&ms, "nope").is_none());
        assert!(parse_monitors("{}").is_err());
        assert!(parse_monitors("").is_err());
    }

    #[test]
    fn picks_a_free_output_name() {
        let ms = parse_monitors(MONITORS).unwrap();
        assert_eq!(free_output_name(&ms), "ezdisplay-2");
        assert_eq!(free_output_name(&ms[..1]), "ezdisplay-1");
        assert_eq!(free_output_name(&[]), "ezdisplay-1");
    }

    #[test]
    fn monitor_rules() {
        assert_eq!(
            monitor_rule("ezdisplay-1", 2560, 1600, 60_000),
            "keyword monitor ezdisplay-1,2560x1600@60,auto,1"
        );
        assert_eq!(
            monitor_rule("ezdisplay-2", 1920, 1080, 59_940),
            "keyword monitor ezdisplay-2,1920x1080@59.94,auto,1"
        );
    }

    #[test]
    fn version_text() {
        assert_eq!(
            parse_version(r#"{"branch":"main","commit":"abc","version":"0.45.2","tag":"v0.45.2"}"#)
                .as_deref(),
            Some("0.45.2")
        );
        assert_eq!(
            parse_version(r#"{"tag":"v0.50.0"}"#).as_deref(),
            Some("v0.50.0")
        );
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn device_names_are_lowercase_with_dashes() {
        assert_eq!(device_for("touch").unwrap(), "ezdisplay-touch");
        assert_eq!(device_for("pen").unwrap(), "ezdisplay-pen");
        assert_eq!(device_for("pointer").unwrap(), "ezdisplay-pointer");
        assert_eq!(device_for("keyboard"), None);
        assert_eq!(device_name("My Fancy  Pen"), "my-fancy--pen");
        assert_eq!(
            map_command("ezdisplay-touch", "ezdisplay-1"),
            "keyword device[ezdisplay-touch]:output ezdisplay-1"
        );
    }

    #[test]
    fn lists_device_names() {
        let json = r#"{"mice":[{"address":"0x1","name":"ezdisplay-pointer"}],
            "keyboards":[{"address":"0x2","name":"ezdisplay-keyboard","main":true}],
            "tablets":[{"address":"0x3","name":"ezdisplay-pen"}],
            "touch":[{"address":"0x4","name":"ezdisplay-touch"}],"switches":[]}"#;
        let mut names = parse_device_names(json);
        names.sort();
        assert_eq!(
            names,
            [
                "ezdisplay-keyboard",
                "ezdisplay-pen",
                "ezdisplay-pointer",
                "ezdisplay-touch"
            ]
        );
        assert!(parse_device_names("nope").is_empty());
    }
}
