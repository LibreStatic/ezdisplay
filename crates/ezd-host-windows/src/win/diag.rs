//! Best-effort, non-mutating diagnostics for `ezd-host diag`.
//!
//! Every section is computed independently so that one failure never hides the others.

use super::capture;
use super::display;
use super::dxgi::{self, AdapterEntry};
use super::source;
use super::util::{dpi_awareness, err_str, windows_version};
use super::vdd;
use ezd_host_core::PlatformOptions;
use serde_json::{Value, json};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIResource,
};

/// The full report.
pub fn report() -> Value {
    let dpi_note = super::util::ensure_dpi_aware();
    let adapters = dxgi::enumerate();
    let (adapters_json, policy_json, vdd_output_json, dda_json, encoders_json) = match &adapters {
        Ok(a) => (
            adapters_section(a),
            policy_section(a),
            vdd_output_section(a),
            dda_section(a),
            encoders_section(a),
        ),
        Err(e) => {
            let err = json!({ "error": e });
            (err.clone(), err.clone(), err.clone(), err.clone(), err)
        }
    };
    json!({
        "windows": windows_version(),
        "dpi": { "awareness": dpi_awareness(), "set": dpi_note },
        "ffmpeg_feature": cfg!(feature = "ffmpeg"),
        "adapters": adapters_json,
        "policy": policy_json,
        "vdd": vdd::status_json(),
        "vdd_output": vdd_output_json,
        "displays": display::diag_json(),
        "input": super::input::diag_json(),
        "encoder_probes": encoders_json,
        "dda_test": dda_json,
    })
}

fn adapters_section(adapters: &[AdapterEntry]) -> Value {
    let order = |f: fn(&AdapterEntry) -> Option<usize>| {
        let mut v: Vec<_> = adapters
            .iter()
            .filter_map(|a| f(a).map(|r| (r, a.info.name.clone())))
            .collect();
        v.sort();
        v.into_iter().map(|(_, n)| n).collect::<Vec<_>>()
    };
    json!({
        "list": adapters.iter().map(|a| {
            let mut j = a.info.to_json();
            j["outputs"] = Value::Array(a.outputs.iter().map(|o| o.to_json()).collect());
            j
        }).collect::<Vec<_>>(),
        "high_performance_order": order(|a| a.info.high_perf_rank),
        "minimum_power_order": order(|a| a.info.min_power_rank),
    })
}

fn policy_section(adapters: &[AdapterEntry]) -> Value {
    let show = |opts: &PlatformOptions| match source::decide(adapters, opts) {
        Ok(d) => d.to_json(),
        Err(e) => json!({ "error": e.to_string() }),
    };
    json!({
        "default": show(&PlatformOptions::default()),
        "prefer_dgpu": show(&PlatformOptions { prefer_dgpu: true, ..Default::default() }),
    })
}

/// Where the VDD output lives, and whether that is the adapter the policy picks.
fn vdd_output_section(adapters: &[AdapterEntry]) -> Value {
    let monitors = display::find_vdd_monitors();
    let Some((ai, oi)) = vdd::find_vdd_output(adapters, &monitors) else {
        return json!({
            "found": false,
            "monitors": monitors.iter().map(|m| json!({"gdi_name": m.gdi_name, "how": m.how, "cloned": m.cloned})).collect::<Vec<_>>(),
            "note": "no VDD output visible to DXGI (driver not installed, no monitor enabled, or the monitor is detached)",
        });
    };
    let a = &adapters[ai];
    let o = &a.outputs[oi];
    let chosen = source::decide(adapters, &PlatformOptions::default()).ok();
    let mismatch = chosen.as_ref().map(|d| d.adapter.luid != a.info.luid);
    json!({
        "found": true,
        "output": o.to_json(),
        "adapter": a.info.name,
        "adapter_luid": a.info.luid_hex(),
        "policy_adapter": chosen.as_ref().map(|d| d.adapter.name.clone()),
        "adapter_mismatch": mismatch,
        "warning": mismatch.filter(|m| *m).map(|_| {
            "the VDD output is on a different GPU than the policy would choose; `ezd-host run` assigns the VDD to the chosen GPU (SETGPU) automatically, or run `ezd-host vdd setup --gpu NAME`"
        }),
    })
}

#[cfg(feature = "ffmpeg")]
fn encoders_section(adapters: &[AdapterEntry]) -> Value {
    let (w, h) = source::PROBE_SIZE;
    let list: Vec<Value> = adapters
        .iter()
        .filter(|a| !a.info.is_software())
        .map(|a| {
            let mut names: Vec<&str> = Vec::new();
            if let Some(hw) = crate::policy::hw_encoder_for_vendor(a.info.vendor_id) {
                names.push(hw);
            }
            // The HDR10 encoder (HEVC Main10); there is no software one.
            if let Some(hdr) = crate::policy::hdr_encoder_for_vendor(a.info.vendor_id) {
                names.push(hdr);
            }
            names.push("libx264");
            let probes: serde_json::Map<String, Value> = names
                .into_iter()
                .map(|n| {
                    let r = match source::probe_encoder(a, n) {
                        Ok(note) => json!({ "ok": true, "detail": note }),
                        Err(e) => json!({ "ok": false, "detail": e }),
                    };
                    (n.to_string(), r)
                })
                .collect();
            json!({ "adapter": a.info.name, "luid": a.info.luid_hex(), "encoders": probes })
        })
        .collect();
    json!({ "size": format!("{w}x{h}"), "frames": 1, "adapters": list })
}

#[cfg(not(feature = "ffmpeg"))]
fn encoders_section(_adapters: &[AdapterEntry]) -> Value {
    json!({ "status": "built without ffmpeg" })
}

/// Duplicates the VDD output (if attached) and acquires one frame. Does not change any setting.
fn dda_section(adapters: &[AdapterEntry]) -> Value {
    let monitors = display::find_vdd_monitors();
    let Some((ai, oi)) = vdd::find_vdd_output(adapters, &monitors) else {
        return json!({ "status": "skipped: no VDD output" });
    };
    let (a, o) = (&adapters[ai], &adapters[ai].outputs[oi]);
    if !o.attached {
        return json!({ "status": "skipped: the VDD output is not attached to the desktop", "output": o.device_name });
    }
    let device = match dxgi::create_device(&a.adapter) {
        Ok(d) => d,
        Err(e) => return json!({ "status": "failed", "stage": "device", "error": e }),
    };
    let dup = match capture::duplicate(&device, &o.output, o.color.is_some_and(|c| c.hdr)) {
        Ok(d) => d,
        Err(e) => {
            return json!({ "status": "failed", "stage": "DuplicateOutput", "error": e, "adapter": a.info.name });
        }
    };
    let mut acquired = None;
    let started = std::time::Instant::now();
    for _ in 0..4 {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        // SAFETY: valid out-parameters; released below on success.
        match unsafe { dup.dup.AcquireNextFrame(250, &mut info, &mut res) } {
            Ok(()) => {
                acquired = Some(json!({
                    "last_present_time": info.LastPresentTime,
                    "accumulated_frames": info.AccumulatedFrames,
                    "pointer_shape_bytes": info.PointerShapeBufferSize,
                }));
                drop(res);
                // SAFETY: a frame was acquired above.
                let _ = unsafe { dup.dup.ReleaseFrame() };
                break;
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
            Err(e) => {
                return json!({ "status": "failed", "stage": "AcquireNextFrame", "error": err_str(&e), "via": dup.via });
            }
        }
    }
    json!({
        "status": "ok",
        "via": dup.via,
        "adapter": a.info.name,
        "output": o.device_name,
        "mode": format!("{}x{}", dup.desc.ModeDesc.Width, dup.desc.ModeDesc.Height),
        "format": format!("{:?}", dup.desc.ModeDesc.Format),
        "in_system_memory": dup.desc.DesktopImageInSystemMemory.as_bool(),
        "frame": acquired,
        "note": if acquired.is_none() { Some("duplication works, but no frame was presented within 1 s (an idle virtual display only presents on change)") } else { None },
        "elapsed_ms": started.elapsed().as_millis() as u64,
    })
}
