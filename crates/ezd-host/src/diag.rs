//! `ezd-host diag`: a JSON report made of independent sections.
//!
//! To extend the report for a platform, implement [`Section`] and add it to
//! [`sections`] (behind `cfg`). Windows will add `dxgi` (adapters + LUIDs and
//! the parent adapter of the virtual display output); Linux backends can add
//! KDE/KWin and PipeWire details. Sections must never fail: errors are data.

use ezd_ffmpeg::{HwKind, probe_encoder_with};
use ezd_link::adb::{Adb, find_adb};
use serde_json::{Value, json};

/// One part of the report.
pub trait Section {
    /// Key in the top-level JSON object.
    fn name(&self) -> &'static str;
    /// Collects the data; must not panic or fail.
    fn collect(&self) -> Value;
}

/// All sections, in output order.
pub fn sections() -> Vec<Box<dyn Section>> {
    let mut v: Vec<Box<dyn Section>> = vec![Box::new(HostInfo)];
    #[cfg(target_os = "linux")]
    {
        v.push(Box::new(LinuxSession));
        v.push(Box::new(LinuxGpus));
    }
    v.push(Box::new(Platform));
    v.push(Box::new(Encoders));
    v.push(Box::new(AdbInfo));
    v
}

/// Builds the full report.
pub fn report() -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "tool".into(),
        json!({ "name": "ezd-host", "version": env!("CARGO_PKG_VERSION") }),
    );
    for s in sections() {
        map.insert(s.name().into(), s.collect());
    }
    Value::Object(map)
}

struct HostInfo;
impl Section for HostInfo {
    fn name(&self) -> &'static str {
        "os"
    }
    fn collect(&self) -> Value {
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut v = json!({ "family": std::env::consts::OS, "arch": std::env::consts::ARCH });
        #[cfg(target_os = "linux")]
        {
            v["kernel"] = read_trim("/proc/sys/kernel/osrelease");
            if let Ok(s) = std::fs::read_to_string("/etc/os-release") {
                let pretty = s
                    .lines()
                    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                    .map(|p| p.trim_matches('"').to_string());
                v["distro"] = pretty.into();
            }
        }
        v
    }
}

#[cfg(target_os = "linux")]
fn read_trim(path: impl AsRef<std::path::Path>) -> Value {
    match std::fs::read_to_string(path) {
        Ok(s) => Value::String(s.trim().to_string()),
        Err(_) => Value::Null,
    }
}

#[cfg(target_os = "linux")]
struct LinuxSession;
#[cfg(target_os = "linux")]
impl Section for LinuxSession {
    fn name(&self) -> &'static str {
        "session"
    }
    fn collect(&self) -> Value {
        let env = |k: &str| std::env::var(k).ok();
        json!({
            "XDG_CURRENT_DESKTOP": env("XDG_CURRENT_DESKTOP"),
            "XDG_SESSION_TYPE": env("XDG_SESSION_TYPE"),
            "WAYLAND_DISPLAY": env("WAYLAND_DISPLAY"),
            "DISPLAY": env("DISPLAY"),
        })
    }
}

#[cfg(target_os = "linux")]
fn render_nodes() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("renderD"))
        .collect();
    v.sort();
    v
}

#[cfg(target_os = "linux")]
struct LinuxGpus;
#[cfg(target_os = "linux")]
impl Section for LinuxGpus {
    fn name(&self) -> &'static str {
        "gpus"
    }
    fn collect(&self) -> Value {
        let gpus: Vec<Value> = render_nodes()
            .into_iter()
            .map(|n| {
                let dev = format!("/sys/class/drm/{n}/device");
                let driver = std::fs::read_link(format!("{dev}/driver"))
                    .ok()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
                let hex = |f: &str| read_trim(format!("{dev}/{f}"));
                json!({
                    "render_node": format!("/dev/dri/{n}"),
                    "driver": driver,
                    "pci_vendor": hex("vendor"),
                    "pci_device": hex("device"),
                    "pci_slot": std::fs::read_link(&dev).ok()
                        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())),
                })
            })
            .collect();
        Value::Array(gpus)
    }
}

/// Backend-specific section; the platform crates fill it in.
struct Platform;
impl Section for Platform {
    fn name(&self) -> &'static str {
        "platform"
    }
    fn collect(&self) -> Value {
        crate::platform::diag_section()
    }
}

struct Encoders;
impl Section for Encoders {
    fn name(&self) -> &'static str {
        "encoders"
    }
    fn collect(&self) -> Value {
        let probe = |name: &str, hw: Option<(HwKind, Option<&str>)>| {
            let r = probe_encoder_with(name, 1280, 720, hw);
            json!({ "ok": r.ok, "detail": r.detail })
        };
        let mut m = serde_json::Map::new();
        for n in [
            "libx264",
            "libopenh264",
            "h264_nvenc",
            "h264_qsv",
            "h264_amf",
        ] {
            m.insert(n.into(), probe(n, None));
        }
        #[cfg(target_os = "linux")]
        {
            let mut per_node = serde_json::Map::new();
            for n in render_nodes() {
                let path = format!("/dev/dri/{n}");
                per_node.insert(
                    path.clone(),
                    probe("h264_vaapi", Some((HwKind::Vaapi, Some(&path)))),
                );
            }
            m.insert("h264_vaapi".into(), Value::Object(per_node));
        }
        Value::Object(m)
    }
}

struct AdbInfo;
impl Section for AdbInfo {
    fn name(&self) -> &'static str {
        "adb"
    }
    fn collect(&self) -> Value {
        let Some(path) = find_adb() else {
            return json!({ "path": null, "error": "adb not found" });
        };
        let adb = Adb::new(&path);
        let devices = match adb.devices() {
            Ok(d) => Value::Array(
                d.into_iter()
                    .map(|d| {
                        json!({
                            "serial": d.serial,
                            "state": format!("{:?}", d.state),
                            "model": d.model,
                            "product": d.product,
                            "transport": format!("{:?}", d.transport),
                        })
                    })
                    .collect(),
            ),
            Err(e) => json!({ "error": e.to_string() }),
        };
        json!({ "path": path.display().to_string(), "version": adb.version().ok(), "devices": devices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_info_section_is_an_object_with_basics() {
        let v = HostInfo.collect();
        assert_eq!(v["family"], std::env::consts::OS);
        assert!(v["arch"].is_string());
    }

    #[test]
    fn section_names_are_unique() {
        let names: Vec<_> = sections().iter().map(|s| s.name()).collect();
        let mut dedup = names.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(names.len(), dedup.len());
    }
}
