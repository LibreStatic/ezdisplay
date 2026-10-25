//! `ezd-host diag` platform section. Never opens the portal dialog.

use crate::adapter::list_render_nodes;
use crate::backend::{self, LinuxBackend};
use crate::encode::{Kind, Plan};
use crate::kscreen;
use crate::portal::{self, SourceKind};
use crate::source::plan_works;
use serde_json::{Value, json};
use std::process::Command;

fn kwin_version() -> Option<String> {
    let out = Command::new("kwin_wayland")
        .arg("--version")
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Builds the diagnostic JSON.
pub fn section() -> Value {
    let nodes = list_render_nodes();
    let probes: Vec<Value> = nodes
        .iter()
        .map(|n| {
            let mut encs = Vec::new();
            let plans = [if n.is_nvidia() {
                Plan { name: "h264_nvenc", kind: Kind::Nvenc, node: Some(n.path.clone()) }
            } else {
                Plan { name: "h264_vaapi", kind: Kind::Vaapi, node: Some(n.path.clone()) }
            }];
            // Only non-NVIDIA nodes are probed in diag so the dGPU is not woken up needlessly.
            for p in plans {
                if p.kind == Kind::Nvenc && !diag_probe_nvidia() {
                    encs.push(json!({"encoder": p.name, "probed": false, "note": "set EZD_DIAG_PROBE_NVIDIA=1 to probe (wakes the dGPU)"}));
                    continue;
                }
                match plan_works(&p) {
                    Ok(()) => encs.push(json!({"encoder": p.name, "ok": true})),
                    Err(e) => encs.push(json!({"encoder": p.name, "ok": false, "error": e})),
                }
            }
            json!({
                "path": n.path,
                "vendor": n.vendor_name(),
                "vendor_id": format!("{:#06x}", n.vendor),
                "driver": n.driver,
                "pci": n.pci,
                "compositor": n.compositor,
                "encoders": encs,
            })
        })
        .collect();
    let x264 = Plan {
        name: "libx264",
        kind: Kind::X264,
        node: None,
    };
    let sw = plan_works(&x264);
    let token = |k| portal::token_path(k).is_some_and(|p| p.exists());
    let detected = backend::current();
    let name = detected.as_ref().map_or("unknown", |b| b.name());
    let mut v = json!({
        "backend": format!("linux-{name}"),
        "backend_supported": detected.as_ref().is_ok_and(|b| b.supported()),
        "backend_error": detected.as_ref().err(),
        "xdg_current_desktop": std::env::var("XDG_CURRENT_DESKTOP").ok(),
        "wayland_display": std::env::var("WAYLAND_DISPLAY").ok(),
        "portal": portal::query_properties(),
        "render_nodes": probes,
        "software_encoder": { "libx264": sw.is_ok(), "error": sw.err() },
        "restore_token": {
            "virtual": token(SourceKind::Virtual),
            "monitor": token(SourceKind::Monitor),
            "path": portal::token_path(SourceKind::Virtual),
        },
        "input": { "uinput": uinput_state() },
    });
    let extra = match detected {
        Ok(LinuxBackend::Gnome) => json!({ "gnome": crate::gnome::diag() }),
        _ => json!({
            "kwin_version": kwin_version(),
            "kwin_drm_devices": crate::adapter::kwin_drm_devices(),
            "kscreen_doctor": kscreen::available(),
        }),
    };
    if let (Some(obj), Some(more)) = (v.as_object_mut(), extra.as_object()) {
        obj.extend(more.clone());
    }
    if matches!(detected, Ok(LinuxBackend::Kde) | Err(_))
        && let Some(input) = v.get_mut("input").and_then(Value::as_object_mut)
    {
        input.insert(
            "kwin_input_dbus".into(),
            json!(crate::kwin_input::manager_available()),
        );
    }
    v
}

/// Whether `/dev/uinput` can be opened (no device is created).
fn uinput_state() -> &'static str {
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
    {
        Ok(_) => "ok",
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => "permission denied",
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "error",
    }
}

fn diag_probe_nvidia() -> bool {
    std::env::var_os("EZD_DIAG_PROBE_NVIDIA").is_some()
}
