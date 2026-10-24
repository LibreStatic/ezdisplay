//! GPU discovery through sysfs: render nodes, their vendor/driver, and which one the compositor
//! renders on (the default for zero-copy capture + encode).

use std::fs;
use std::path::{Path, PathBuf};

/// A DRM render node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderNode {
    /// `/dev/dri/renderD129`.
    pub path: String,
    /// PCI vendor id (0 if unknown).
    pub vendor: u16,
    /// Kernel driver name (`i915`, `nvidia`, `amdgpu`, ...).
    pub driver: String,
    /// PCI bus id (`0000:00:02.0`).
    pub pci: String,
    /// True if the compositor renders on this GPU: KWin's primary GPU when `KWIN_DRM_DEVICES`
    /// names one, otherwise the GPU driving the internal panel or primary connector.
    pub compositor: bool,
}

impl RenderNode {
    /// Vendor name for display.
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor {
            0x8086 => "Intel",
            0x10de => "NVIDIA",
            0x1002 | 0x1022 => "AMD",
            _ => "unknown",
        }
    }

    /// `Intel GPU (i915, /dev/dri/renderD129)`.
    pub fn description(&self) -> String {
        format!(
            "{} GPU ({}, {})",
            self.vendor_name(),
            self.driver,
            self.path
        )
    }

    /// True for NVIDIA devices (NVENC, no VAAPI).
    pub fn is_nvidia(&self) -> bool {
        self.vendor == 0x10de
    }
}

/// `card2-eDP-1` -> `card2`.
pub fn connector_card(entry: &str) -> Option<&str> {
    let (card, rest) = entry.split_once('-')?;
    (card.starts_with("card") && card[4..].chars().all(|c| c.is_ascii_digit()) && !rest.is_empty())
        .then_some(card)
}

/// Internal panel connectors rank first, then any connected output.
fn connector_rank(entry: &str) -> u8 {
    let n = entry.split_once('-').map_or("", |(_, r)| r);
    if n.starts_with("eDP") || n.starts_with("LVDS") || n.starts_with("DSI") {
        0
    } else {
        1
    }
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// The `cardN` driving the primary display: a connected internal panel first, otherwise the first
/// connected connector.
pub fn compositor_card(drm_root: &Path) -> Option<String> {
    let mut conns: Vec<(u8, String, String)> = fs::read_dir(drm_root)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let card = connector_card(&name)?.to_string();
            (read_trim(e.path().join("status")).as_deref() == Some("connected"))
                .then(|| (connector_rank(&name), name, card))
        })
        .collect();
    conns.sort();
    conns.into_iter().next().map(|(_, _, card)| card)
}

/// The `cardN` that KWin uses as its primary GPU, from `KWIN_DRM_DEVICES`: a colon-separated list
/// of `/dev/dri/cardN` paths whose first entry renders every output (the others get a copy).
/// Symlinks such as `/dev/dri/by-path/...` are resolved.
pub fn kwin_primary_card(kwin_drm_devices: &str) -> Option<String> {
    let first = Path::new(kwin_drm_devices.split(':').next()?.trim());
    let resolved = fs::canonicalize(first).unwrap_or_else(|_| first.to_path_buf());
    let name = resolved.file_name()?.to_str()?;
    (name.starts_with("card") && name.len() > 4 && name[4..].chars().all(|c| c.is_ascii_digit()))
        .then(|| name.to_string())
}

/// Lists render nodes using `drm_root` (normally `/sys/class/drm`) and `dev_root` (`/dev/dri`).
/// `kwin_drm_devices` is the value of `KWIN_DRM_DEVICES`, if set.
pub fn list_render_nodes_in(
    drm_root: &Path,
    dev_root: &Path,
    kwin_drm_devices: Option<&str>,
) -> Vec<RenderNode> {
    let comp_dev: Option<PathBuf> = kwin_drm_devices
        .and_then(kwin_primary_card)
        .filter(|c| drm_root.join(c).exists())
        .or_else(|| compositor_card(drm_root))
        .and_then(|c| fs::canonicalize(drm_root.join(c).join("device")).ok());
    let mut nodes: Vec<RenderNode> = fs::read_dir(drm_root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("renderD")?
                .parse::<u32>()
                .ok()
                .map(|_| ())?;
            let dev = fs::canonicalize(e.path().join("device")).ok()?;
            let vendor = read_trim(dev.join("vendor"))
                .and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            let driver = fs::read_link(dev.join("driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "unknown".into());
            let pci = dev
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Some(RenderNode {
                path: dev_root.join(&name).to_string_lossy().into_owned(),
                vendor,
                driver,
                pci,
                compositor: comp_dev.as_deref() == Some(dev.as_path()),
            })
        })
        .collect();
    nodes.sort_by(|a, b| a.path.cmp(&b.path));
    nodes
}

/// Lists render nodes of this machine.
pub fn list_render_nodes() -> Vec<RenderNode> {
    list_render_nodes_in(
        Path::new("/sys/class/drm"),
        Path::new("/dev/dri"),
        std::env::var("KWIN_DRM_DEVICES").ok().as_deref(),
    )
}

/// Why an adapter was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// `--adapter` was given.
    Override,
    /// `--prefer-dgpu` picked the NVIDIA device.
    PreferDgpu,
    /// The GPU that drives the primary display.
    Compositor,
    /// Nothing better known: first render node.
    First,
}

/// The chosen render node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Node (for an override that is not in the list, a node with unknown vendor).
    pub node: RenderNode,
    /// Why.
    pub reason: Reason,
}

/// Chooses the render node to capture and encode on.
pub fn select(nodes: &[RenderNode], adapter: Option<&str>, prefer_dgpu: bool) -> Option<Selection> {
    if let Some(a) = adapter.filter(|a| !a.is_empty()) {
        let found = nodes
            .iter()
            .find(|n| n.path == a || n.path.ends_with(&format!("/{a}")) || n.pci == a);
        let node = found.cloned().unwrap_or_else(|| RenderNode {
            path: if a.contains('/') {
                a.to_string()
            } else {
                format!("/dev/dri/{a}")
            },
            vendor: 0,
            driver: "unknown".into(),
            pci: String::new(),
            compositor: false,
        });
        return Some(Selection {
            node,
            reason: Reason::Override,
        });
    }
    if prefer_dgpu {
        let pick = nodes
            .iter()
            .find(|n| n.is_nvidia())
            .or_else(|| nodes.iter().find(|n| !n.compositor));
        if let Some(n) = pick {
            return Some(Selection {
                node: n.clone(),
                reason: Reason::PreferDgpu,
            });
        }
    }
    if let Some(n) = nodes.iter().find(|n| n.compositor) {
        return Some(Selection {
            node: n.clone(),
            reason: Reason::Compositor,
        });
    }
    nodes.first().map(|n| Selection {
        node: n.clone(),
        reason: Reason::First,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn node(path: &str, vendor: u16, compositor: bool) -> RenderNode {
        RenderNode {
            path: path.into(),
            vendor,
            driver: "x".into(),
            pci: String::new(),
            compositor,
        }
    }

    #[test]
    fn connector_names() {
        assert_eq!(connector_card("card2-eDP-1"), Some("card2"));
        assert_eq!(connector_card("card1-HDMI-A-1"), Some("card1"));
        assert_eq!(connector_card("card1"), None);
        assert_eq!(connector_card("renderD128"), None);
        assert_eq!(connector_card("version"), None);
    }

    #[test]
    fn selection_policy() {
        let nodes = [
            node("/dev/dri/renderD128", 0x10de, false),
            node("/dev/dri/renderD129", 0x8086, true),
        ];
        let s = select(&nodes, None, false).unwrap();
        assert_eq!(
            (s.node.path.as_str(), s.reason),
            ("/dev/dri/renderD129", Reason::Compositor)
        );
        let s = select(&nodes, None, true).unwrap();
        assert_eq!(
            (s.node.path.as_str(), s.reason),
            ("/dev/dri/renderD128", Reason::PreferDgpu)
        );
        let s = select(&nodes, Some("renderD128"), false).unwrap();
        assert_eq!(
            (s.node.path.as_str(), s.reason),
            ("/dev/dri/renderD128", Reason::Override)
        );
        let s = select(&nodes, Some("/dev/dri/renderD200"), false).unwrap();
        assert_eq!(s.node.path, "/dev/dri/renderD200");
        assert_eq!(select(&[], None, false), None);
        // No compositor information: first node.
        let n = [node("/dev/dri/renderD128", 0x10de, false)];
        assert_eq!(select(&n, None, false).unwrap().reason, Reason::First);
    }

    #[test]
    fn sysfs_scan() {
        let base = std::env::temp_dir().join(format!("ezd-adapter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let drm = base.join("class/drm");
        fs::create_dir_all(&drm).unwrap();
        // Two PCI devices.
        for (pci, vendor, driver) in [
            ("0000:01:00.0", "0x10de", "nvidia"),
            ("0000:00:02.0", "0x8086", "i915"),
        ] {
            let d = base.join("devices").join(pci);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("vendor"), format!("{vendor}\n")).unwrap();
            let drv = base.join("drivers").join(driver);
            fs::create_dir_all(&drv).unwrap();
            symlink(&drv, d.join("driver")).unwrap();
        }
        let dev = |pci: &str| base.join("devices").join(pci);
        for (name, pci) in [
            ("renderD128", "0000:01:00.0"),
            ("card1", "0000:01:00.0"),
            ("renderD129", "0000:00:02.0"),
            ("card2", "0000:00:02.0"),
        ] {
            let e = drm.join(name);
            fs::create_dir_all(&e).unwrap();
            symlink(dev(pci), e.join("device")).unwrap();
        }
        for (c, st) in [
            ("card1-HDMI-A-1", "disconnected"),
            ("card2-eDP-1", "connected"),
        ] {
            fs::create_dir_all(drm.join(c)).unwrap();
            fs::write(drm.join(c).join("status"), format!("{st}\n")).unwrap();
        }
        assert_eq!(compositor_card(&drm).as_deref(), Some("card2"));
        let nodes = list_render_nodes_in(&drm, Path::new("/dev/dri"), None);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].path, "/dev/dri/renderD128");
        assert_eq!(nodes[0].driver, "nvidia");
        assert!(nodes[0].is_nvidia() && !nodes[0].compositor);
        assert_eq!(nodes[1].driver, "i915");
        assert_eq!(nodes[1].vendor_name(), "Intel");
        assert!(nodes[1].compositor);
        assert_eq!(nodes[1].pci, "0000:00:02.0");
        // KWin composing on the NVIDIA GPU while the panel stays on the Intel one.
        let nodes = list_render_nodes_in(
            &drm,
            Path::new("/dev/dri"),
            Some("/dev/dri/card1:/dev/dri/card2"),
        );
        assert!(nodes[0].compositor && !nodes[1].compositor);
        // A card that does not exist falls back to the panel.
        let nodes = list_render_nodes_in(&drm, Path::new("/dev/dri"), Some("/dev/dri/card7"));
        assert!(!nodes[0].compositor && nodes[1].compositor);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn kwin_drm_devices_first_entry() {
        assert_eq!(
            kwin_primary_card("/dev/dri/card1:/dev/dri/card2").as_deref(),
            Some("card1")
        );
        assert_eq!(
            kwin_primary_card("/dev/dri/card2").as_deref(),
            Some("card2")
        );
        assert_eq!(kwin_primary_card(""), None);
        assert_eq!(kwin_primary_card("/dev/dri/renderD128"), None);
        assert_eq!(kwin_primary_card("/dev/dri/card"), None);
    }

    #[test]
    fn internal_panel_wins() {
        let base = std::env::temp_dir().join(format!("ezd-adapter2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        for (c, st) in [("card1-DP-1", "connected"), ("card2-eDP-1", "connected")] {
            fs::create_dir_all(base.join(c)).unwrap();
            fs::write(base.join(c).join("status"), st).unwrap();
        }
        assert_eq!(compositor_card(&base).as_deref(), Some("card2"));
        let _ = fs::remove_dir_all(&base);
    }
}
