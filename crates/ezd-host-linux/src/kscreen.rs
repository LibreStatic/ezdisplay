//! `kscreen-doctor` glue: finds the KWin output created for a virtual screencast and gives it the
//! tablet's native mode.
//!
//! The command building and the JSON parsing are pure functions (unit-tested against a sample from
//! a real machine); [`run`]/[`list_outputs`]/[`apply_mode`] execute `kscreen-doctor`.

use serde_json::Value;
use std::process::Command;
use std::time::Duration;

/// One mode of an output.
#[derive(Debug, Clone, PartialEq)]
pub struct KMode {
    /// kscreen mode id (what `output.<name>.mode.<id>` accepts).
    pub id: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Refresh rate in Hz.
    pub refresh_hz: f64,
}

/// One output as reported by `kscreen-doctor -j`.
#[derive(Debug, Clone, PartialEq)]
pub struct KOutput {
    /// Output name (e.g. `eDP-1`, `Virtual-ezdisplay`).
    pub name: String,
    /// Numeric kscreen id.
    pub id: i64,
    /// Whether the output is enabled.
    pub enabled: bool,
    /// Whether the output is connected.
    pub connected: bool,
    /// Current size in pixels.
    pub size: (u32, u32),
    /// Position in the global compositor space.
    pub pos: (i32, i32),
    /// Id of the current mode.
    pub current_mode_id: String,
    /// All modes.
    pub modes: Vec<KMode>,
}

impl KOutput {
    /// The current mode, if it is in the list.
    #[cfg(test)]
    pub fn current_mode(&self) -> Option<&KMode> {
        self.modes.iter().find(|m| m.id == self.current_mode_id)
    }
}

/// Parses the output of `kscreen-doctor -j`.
pub fn parse_outputs(json: &str) -> Result<Vec<KOutput>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("kscreen json: {e}"))?;
    let outs = v
        .get("outputs")
        .and_then(Value::as_array)
        .ok_or("kscreen json has no \"outputs\" array")?;
    let u = |v: Option<&Value>| v.and_then(Value::as_u64).unwrap_or(0) as u32;
    let i = |v: Option<&Value>| v.and_then(Value::as_i64).unwrap_or(0) as i32;
    let mut res = Vec::new();
    for o in outs {
        let Some(name) = o.get("name").and_then(Value::as_str) else {
            continue;
        };
        let modes = o
            .get("modes")
            .and_then(Value::as_array)
            .map(|ms| {
                ms.iter()
                    .filter_map(|m| {
                        Some(KMode {
                            id: m.get("id")?.as_str()?.to_string(),
                            width: u(m.get("size")?.get("width")),
                            height: u(m.get("size")?.get("height")),
                            refresh_hz: m.get("refreshRate")?.as_f64()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        res.push(KOutput {
            name: name.to_string(),
            id: o.get("id").and_then(Value::as_i64).unwrap_or(0),
            enabled: o.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            connected: o.get("connected").and_then(Value::as_bool).unwrap_or(false),
            size: (u(o.get("size").and_then(|s| s.get("width"))), {
                u(o.get("size").and_then(|s| s.get("height")))
            }),
            pos: (
                i(o.get("pos").and_then(|s| s.get("x"))),
                i(o.get("pos").and_then(|s| s.get("y"))),
            ),
            current_mode_id: match o.get("currentModeId") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            },
            modes,
        });
    }
    Ok(res)
}

/// The output present in `after` but not in `before` (by name). If several are new, the
/// `Virtual-*` one wins.
pub fn find_new_output<'a>(before: &[KOutput], after: &'a [KOutput]) -> Option<&'a KOutput> {
    let mut new = after
        .iter()
        .filter(|a| !before.iter().any(|b| b.name == a.name));
    let first = new.next()?;
    if first.name.starts_with("Virtual") {
        return Some(first);
    }
    new.find(|o| o.name.starts_with("Virtual")).or(Some(first))
}

/// Fallback when there is no "before" snapshot: the last `Virtual-*` output.
pub fn find_virtual_output(outputs: &[KOutput]) -> Option<&KOutput> {
    outputs.iter().rev().find(|o| o.name.starts_with("Virtual"))
}

/// A mode of `out` with this size and a refresh rate within 0.5 Hz of `refresh_mhz`.
pub fn find_mode(out: &KOutput, width: u32, height: u32, refresh_mhz: u32) -> Option<&KMode> {
    let want = refresh_mhz as f64 / 1000.0;
    out.modes
        .iter()
        .filter(|m| m.width == width && m.height == height && (m.refresh_hz - want).abs() < 0.5)
        .min_by(|a, b| {
            (a.refresh_hz - want)
                .abs()
                .total_cmp(&(b.refresh_hz - want).abs())
        })
}

/// Argument for `kscreen-doctor` that adds a reduced-blanking custom mode.
pub fn add_custom_mode_arg(output: &str, width: u32, height: u32, refresh_mhz: u32) -> String {
    format!("output.{output}.addCustomMode.{width}.{height}.{refresh_mhz}.reduced")
}

/// Argument for `kscreen-doctor` that switches `output` to mode `mode` (id or `WxH@R`).
pub fn set_mode_arg(output: &str, mode: &str) -> String {
    format!("output.{output}.mode.{mode}")
}

/// Mode name usable with [`set_mode_arg`] when the id is not known.
pub fn mode_name(width: u32, height: u32, refresh_mhz: u32) -> String {
    format!("{width}x{height}@{}", (refresh_mhz as f64 / 1000.0).round())
}

/// True if `kscreen-doctor` can be executed.
pub fn available() -> bool {
    // Only "can it be started": --help may exit non-zero without a Wayland display.
    command().arg("--help").output().is_ok()
}

fn command() -> Command {
    let mut c = Command::new("kscreen-doctor");
    // kscreen-doctor prints nothing without a Wayland display.
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()) {
        c.env("WAYLAND_DISPLAY", "wayland-0");
    }
    c
}

/// Runs `kscreen-doctor <args>` and returns its stdout.
pub fn run(args: &[String]) -> Result<String, String> {
    let out = command()
        .args(args)
        .output()
        .map_err(|e| format!("cannot run kscreen-doctor: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "kscreen-doctor {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Lists all outputs (`kscreen-doctor -j`).
pub fn list_outputs() -> Result<Vec<KOutput>, String> {
    parse_outputs(&run(&["-j".to_string()])?)
}

/// Adds a custom `width`x`height`@`refresh_mhz` mode to `output` (if missing) and switches to it.
/// Returns the final output description.
pub fn apply_mode(
    output: &str,
    width: u32,
    height: u32,
    refresh_mhz: u32,
) -> Result<KOutput, String> {
    let find = |outs: &[KOutput]| -> Option<(KOutput, Option<String>)> {
        let o = outs.iter().find(|o| o.name == output)?;
        let m = find_mode(o, width, height, refresh_mhz).map(|m| m.id.clone());
        Some((o.clone(), m))
    };
    let (mut out, mut mode) =
        find(&list_outputs()?).ok_or_else(|| format!("output {output} not found"))?;
    if mode.is_none() {
        run(&[add_custom_mode_arg(output, width, height, refresh_mhz)])?;
        // KWin applies the new mode list asynchronously.
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(100));
            if let Some((o, m)) = find(&list_outputs()?)
                && m.is_some()
            {
                out = o;
                mode = m;
                break;
            }
        }
    }
    let target = mode.unwrap_or_else(|| mode_name(width, height, refresh_mhz));
    if out.current_mode_id != target {
        run(&[set_mode_arg(output, &target)])?;
    }
    for _ in 0..10 {
        if let Some(o) = list_outputs()?.into_iter().find(|o| o.name == output)
            && o.size == (width, height)
        {
            return Ok(o);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("{output} did not switch to {width}x{height}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDP: &str = include_str!("../testdata/kscreen-eDP.json");

    #[test]
    fn parses_real_sample() {
        let outs = parse_outputs(EDP).unwrap();
        assert_eq!(outs.len(), 1);
        let o = &outs[0];
        assert_eq!(o.name, "eDP-1");
        assert!(o.enabled && o.connected);
        assert_eq!(o.size, (1920, 1080));
        assert_eq!(o.pos, (0, 0));
        assert_eq!(o.current_mode_id, "1");
        let cur = o.current_mode().unwrap();
        assert_eq!((cur.width, cur.height), (1920, 1080));
        assert!((cur.refresh_hz - 60.0).abs() < 0.1);
        assert!(o.modes.len() > 5);
    }

    fn with_virtual() -> String {
        let mut v: Value = serde_json::from_str(EDP).unwrap();
        v["outputs"].as_array_mut().unwrap().push(serde_json::json!({
            "name": "Virtual-ezdisplay", "id": 2, "enabled": true, "connected": true,
            "currentModeId": "m0", "pos": {"x": 1920, "y": 0}, "size": {"width": 1920, "height": 1080},
            "modes": [
                {"id": "m0", "name": "1920x1080@60", "refreshRate": 60.0, "size": {"width": 1920, "height": 1080}},
                {"id": "m1", "name": "2560x1600@120", "refreshRate": 120.0, "size": {"width": 2560, "height": 1600}}
            ]
        }));
        v.to_string()
    }

    #[test]
    fn finds_new_output_by_diff() {
        let before = parse_outputs(EDP).unwrap();
        let after = parse_outputs(&with_virtual()).unwrap();
        assert_eq!(
            find_new_output(&before, &after).unwrap().name,
            "Virtual-ezdisplay"
        );
        assert!(find_new_output(&after, &after).is_none());
        assert_eq!(
            find_virtual_output(&after).unwrap().name,
            "Virtual-ezdisplay"
        );
        assert!(find_virtual_output(&before).is_none());
    }

    #[test]
    fn finds_modes() {
        let after = parse_outputs(&with_virtual()).unwrap();
        let v = &after[1];
        assert_eq!(find_mode(v, 2560, 1600, 120_000).unwrap().id, "m1");
        assert_eq!(find_mode(v, 2560, 1600, 119_880).unwrap().id, "m1");
        assert!(find_mode(v, 2560, 1600, 60_000).is_none());
        assert!(find_mode(v, 1280, 800, 120_000).is_none());
    }

    #[test]
    fn builds_commands() {
        assert_eq!(
            add_custom_mode_arg("Virtual-1", 2560, 1600, 120_000),
            "output.Virtual-1.addCustomMode.2560.1600.120000.reduced"
        );
        assert_eq!(
            set_mode_arg("Virtual-1", "2560x1600@120"),
            "output.Virtual-1.mode.2560x1600@120"
        );
        assert_eq!(mode_name(2560, 1600, 119_880), "2560x1600@120");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_outputs("nope").is_err());
        assert!(parse_outputs("{}").is_err());
    }
}
