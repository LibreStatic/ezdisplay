//! GPU policy: which adapter hosts the virtual display, the capture and the encoder.
//!
//! Pure logic (no Windows calls) so it is unit-tested on every platform. The
//! Windows layer fills [`AdapterInfo`] from DXGI and feeds it to [`decide`].
//!
//! Decision order (first that applies wins):
//! 1. `--adapter`: a name substring or a LUID (`0x<hex>` or `<high>:<low>` in hex).
//! 2. `--prefer-dgpu`: the DXGI high-performance adapter.
//! 3. The adapter that owns the primary display (what the desktop already runs on;
//!    in a MUX discrete-only mode that is the NVIDIA GPU, in hybrid mode the iGPU).
//!
//! The same adapter is used for the VDD, DDA capture and the encoder, so frames
//! never cross the PCIe bus.

use ezd_host_core::PlatformOptions;
use serde_json::json;

/// PCI vendor id of NVIDIA.
pub const VENDOR_NVIDIA: u32 = 0x10DE;
/// PCI vendor id of Intel.
pub const VENDOR_INTEL: u32 = 0x8086;
/// PCI vendor id of AMD.
pub const VENDOR_AMD: u32 = 0x1002;
/// PCI vendor id of Microsoft (Basic Render Driver).
pub const VENDOR_MICROSOFT: u32 = 0x1414;

/// A graphics adapter as seen by DXGI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
    /// Adapter description.
    pub name: String,
    /// PCI vendor id.
    pub vendor_id: u32,
    /// PCI device id.
    pub device_id: u32,
    /// LUID as `(high << 32) | low`.
    pub luid: u64,
    /// Dedicated video memory in bytes.
    pub dedicated_vram: u64,
    /// `DXGI_ADAPTER_FLAG` bits.
    pub flags: u32,
    /// Position in the `DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE` enumeration.
    pub high_perf_rank: Option<usize>,
    /// Position in the `DXGI_GPU_PREFERENCE_MINIMUM_POWER` enumeration.
    pub min_power_rank: Option<usize>,
    /// True if this adapter owns the output that is the primary monitor.
    pub owns_primary: bool,
}

/// `DXGI_ADAPTER_FLAG_SOFTWARE`.
pub const ADAPTER_FLAG_SOFTWARE: u32 = 2;

impl AdapterInfo {
    /// WARP / Basic Render Driver or any software adapter.
    pub fn is_software(&self) -> bool {
        self.flags & ADAPTER_FLAG_SOFTWARE != 0
            || self.vendor_id == VENDOR_MICROSOFT
            || self.name.contains("Basic Render")
    }

    /// Vendor name for reports.
    pub fn vendor_name(&self) -> &'static str {
        vendor_name(self.vendor_id)
    }

    /// Hex string of the LUID, e.g. `0x00000000000123ab`.
    pub fn luid_hex(&self) -> String {
        format!("0x{:016x}", self.luid)
    }

    /// JSON for the diagnostics report.
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "name": self.name,
            "vendor": self.vendor_name(),
            "vendor_id": format!("0x{:04x}", self.vendor_id),
            "device_id": format!("0x{:04x}", self.device_id),
            "luid": self.luid_hex(),
            "dedicated_vram_mib": self.dedicated_vram / (1024 * 1024),
            "flags": self.flags,
            "software": self.is_software(),
            "high_performance_rank": self.high_perf_rank,
            "minimum_power_rank": self.min_power_rank,
            "owns_primary_display": self.owns_primary,
        })
    }
}

/// Vendor name for a PCI vendor id.
pub fn vendor_name(vendor_id: u32) -> &'static str {
    match vendor_id {
        VENDOR_NVIDIA => "NVIDIA",
        VENDOR_INTEL => "Intel",
        VENDOR_AMD => "AMD",
        VENDOR_MICROSOFT => "Microsoft",
        _ => "unknown",
    }
}

/// Hardware H.264 encoder (FFmpeg name) for a vendor.
pub fn hw_encoder_for_vendor(vendor_id: u32) -> Option<&'static str> {
    match vendor_id {
        VENDOR_NVIDIA => Some("h264_nvenc"),
        VENDOR_INTEL => Some("h264_qsv"),
        VENDOR_AMD => Some("h264_amf"),
        _ => None,
    }
}

/// Hardware HEVC encoder (FFmpeg name) for a vendor: the only HDR10 (Main10) encoders. There is
/// no software fallback, a failed probe means the stream falls back to tone-mapped SDR.
pub fn hdr_encoder_for_vendor(vendor_id: u32) -> Option<&'static str> {
    match vendor_id {
        VENDOR_NVIDIA => Some("hevc_nvenc"),
        VENDOR_INTEL => Some("hevc_qsv"),
        VENDOR_AMD => Some("hevc_amf"),
        _ => None,
    }
}

/// Why an adapter was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Selected with `--adapter`.
    Explicit,
    /// `--prefer-dgpu`: the high-performance adapter.
    PreferDgpu,
    /// The adapter that drives the primary display.
    PrimaryDisplay,
    /// No primary display found: the high-performance adapter.
    HighPerformanceFallback,
}

impl Reason {
    /// Short stable string for reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Explicit => "explicit --adapter",
            Reason::PreferDgpu => "--prefer-dgpu (high-performance adapter)",
            Reason::PrimaryDisplay => "adapter that drives the primary display",
            Reason::HighPerformanceFallback => {
                "no primary display adapter found; high-performance adapter"
            }
        }
    }
}

/// Result of [`decide`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The chosen adapter.
    pub adapter: AdapterInfo,
    /// Why it was chosen.
    pub reason: Reason,
    /// Hardware encoder expected on this adapter, `None` for unknown vendors.
    pub hw_encoder: Option<&'static str>,
    /// Encoders to try in order, ending with the software fallback.
    pub encoder_chain: Vec<&'static str>,
    /// HDR10 (HEVC Main10) encoders to try in order; empty for unknown vendors. Never software.
    pub hdr_chain: Vec<&'static str>,
}

impl Decision {
    /// JSON for the diagnostics report.
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "adapter": self.adapter.name,
            "luid": self.adapter.luid_hex(),
            "vendor": self.adapter.vendor_name(),
            "reason": self.reason.as_str(),
            "encoder_chain": self.encoder_chain,
            "hdr_chain": self.hdr_chain,
        })
    }
}

/// Policy failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// `--adapter` matched nothing.
    AdapterNotFound {
        /// What the user passed.
        spec: String,
        /// Names of the adapters that exist.
        available: Vec<String>,
    },
    /// No usable hardware adapter at all.
    NoAdapters,
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::AdapterNotFound { spec, available } => write!(
                f,
                "no GPU matches --adapter {spec:?}; available: {}",
                available.join(", ")
            ),
            PolicyError::NoAdapters => write!(f, "no hardware GPU found"),
        }
    }
}

impl std::error::Error for PolicyError {}

/// Parses a LUID spec: `0x<hex>` (64 bit) or `<high>:<low>` (hex, as printed by tools).
pub fn parse_luid(spec: &str) -> Option<u64> {
    let s = spec.trim();
    let hex = |t: &str| {
        let t = t.trim();
        u64::from_str_radix(
            t.strip_prefix("0x")
                .or_else(|| t.strip_prefix("0X"))
                .unwrap_or(t),
            16,
        )
        .ok()
    };
    if let Some((hi, lo)) = s.split_once(':') {
        let (hi, lo) = (hex(hi)?, hex(lo)?);
        return (hi <= u32::MAX as u64 && lo <= u32::MAX as u64).then_some((hi << 32) | lo);
    }
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .and_then(|h| u64::from_str_radix(h, 16).ok())
}

/// Adapters (non-software) matching `spec`: a LUID, or a case-insensitive
/// substring of the name. Best match first (most dedicated memory).
pub fn match_adapters<'a>(adapters: &'a [AdapterInfo], spec: &str) -> Vec<&'a AdapterInfo> {
    let mut v: Vec<&AdapterInfo> = if let Some(luid) = parse_luid(spec) {
        adapters.iter().filter(|a| a.luid == luid).collect()
    } else {
        let needle = spec.trim().to_lowercase();
        adapters
            .iter()
            .filter(|a| !needle.is_empty() && a.name.to_lowercase().contains(&needle))
            .collect()
    };
    v.retain(|a| !a.is_software());
    v.sort_by_key(|a| std::cmp::Reverse(a.dedicated_vram));
    v
}

fn high_performance<'a>(hw: &[&'a AdapterInfo]) -> Option<&'a AdapterInfo> {
    // DXGI rank when available, otherwise the one with the most dedicated memory.
    hw.iter()
        .copied()
        .filter(|a| a.high_perf_rank.is_some())
        .min_by_key(|a| a.high_perf_rank)
        .or_else(|| hw.iter().copied().max_by_key(|a| a.dedicated_vram))
}

/// Picks the adapter for the virtual display, capture and encoder.
pub fn decide(adapters: &[AdapterInfo], opts: &PlatformOptions) -> Result<Decision, PolicyError> {
    let hw: Vec<&AdapterInfo> = adapters.iter().filter(|a| !a.is_software()).collect();
    if hw.is_empty() {
        return Err(PolicyError::NoAdapters);
    }
    let (chosen, reason) = if let Some(spec) = opts.adapter.as_deref() {
        match match_adapters(adapters, spec).first() {
            Some(a) => (*a, Reason::Explicit),
            None => {
                return Err(PolicyError::AdapterNotFound {
                    spec: spec.to_string(),
                    available: hw.iter().map(|a| a.name.clone()).collect(),
                });
            }
        }
    } else if opts.prefer_dgpu {
        (
            high_performance(&hw).ok_or(PolicyError::NoAdapters)?,
            Reason::PreferDgpu,
        )
    } else if let Some(a) = hw.iter().copied().find(|a| a.owns_primary) {
        (a, Reason::PrimaryDisplay)
    } else {
        (
            high_performance(&hw).ok_or(PolicyError::NoAdapters)?,
            Reason::HighPerformanceFallback,
        )
    };
    let hw_encoder = hw_encoder_for_vendor(chosen.vendor_id);
    let mut encoder_chain: Vec<&'static str> = hw_encoder.into_iter().collect();
    encoder_chain.push("libx264");
    Ok(Decision {
        adapter: chosen.clone(),
        reason,
        hw_encoder,
        encoder_chain,
        hdr_chain: hdr_encoder_for_vendor(chosen.vendor_id)
            .into_iter()
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(
        name: &str,
        vendor: u32,
        luid: u64,
        vram_mib: u64,
        hp: usize,
        mp: usize,
        primary: bool,
    ) -> AdapterInfo {
        AdapterInfo {
            name: name.into(),
            vendor_id: vendor,
            device_id: 0x1234,
            luid,
            dedicated_vram: vram_mib << 20,
            flags: 0,
            high_perf_rank: Some(hp),
            min_power_rank: Some(mp),
            owns_primary: primary,
        }
    }

    fn nvidia(primary: bool) -> AdapterInfo {
        gpu(
            "NVIDIA GeForce RTX 3050 Laptop GPU",
            VENDOR_NVIDIA,
            0x1000,
            4096,
            0,
            1,
            primary,
        )
    }
    fn intel(primary: bool) -> AdapterInfo {
        gpu(
            "Intel(R) UHD Graphics",
            VENDOR_INTEL,
            0x2000,
            128,
            1,
            0,
            primary,
        )
    }
    fn amd(primary: bool) -> AdapterInfo {
        gpu(
            "AMD Radeon(TM) Graphics",
            VENDOR_AMD,
            0x3000,
            512,
            1,
            0,
            primary,
        )
    }
    fn warp() -> AdapterInfo {
        AdapterInfo {
            name: "Microsoft Basic Render Driver".into(),
            vendor_id: VENDOR_MICROSOFT,
            device_id: 0x8c,
            luid: 0x9000,
            dedicated_vram: 0,
            flags: ADAPTER_FLAG_SOFTWARE,
            high_perf_rank: None,
            min_power_rank: None,
            owns_primary: false,
        }
    }

    fn opts(adapter: Option<&str>, prefer_dgpu: bool) -> PlatformOptions {
        PlatformOptions {
            adapter: adapter.map(str::to_string),
            prefer_dgpu,
            output_name: None,
        }
    }

    #[test]
    fn table() {
        struct Case {
            label: &'static str,
            adapters: Vec<AdapterInfo>,
            opts: PlatformOptions,
            want_name: &'static str,
            want_reason: Reason,
            want_chain: &'static [&'static str],
        }
        // The HDR chain follows the vendor of the chosen adapter and has no software entry.
        fn hdr_for(chain: &[&str]) -> &'static [&'static str] {
            match chain[0] {
                "h264_nvenc" => &["hevc_nvenc"],
                "h264_qsv" => &["hevc_qsv"],
                "h264_amf" => &["hevc_amf"],
                _ => &[],
            }
        }
        let cases = [
            Case {
                label: "discrete-only MUX: NVIDIA drives the panel",
                adapters: vec![nvidia(true), intel(false), warp()],
                opts: opts(None, false),
                want_name: "NVIDIA GeForce RTX 3050 Laptop GPU",
                want_reason: Reason::PrimaryDisplay,
                want_chain: &["h264_nvenc", "libx264"],
            },
            Case {
                label: "hybrid Intel+NVIDIA: iGPU owns the panel",
                adapters: vec![nvidia(false), intel(true), warp()],
                opts: opts(None, false),
                want_name: "Intel(R) UHD Graphics",
                want_reason: Reason::PrimaryDisplay,
                want_chain: &["h264_qsv", "libx264"],
            },
            Case {
                label: "hybrid AMD+NVIDIA: iGPU owns the panel",
                adapters: vec![nvidia(false), amd(true)],
                opts: opts(None, false),
                want_name: "AMD Radeon(TM) Graphics",
                want_reason: Reason::PrimaryDisplay,
                want_chain: &["h264_amf", "libx264"],
            },
            Case {
                label: "prefer_dgpu in hybrid Intel+NVIDIA",
                adapters: vec![nvidia(false), intel(true)],
                opts: opts(None, true),
                want_name: "NVIDIA GeForce RTX 3050 Laptop GPU",
                want_reason: Reason::PreferDgpu,
                want_chain: &["h264_nvenc", "libx264"],
            },
            Case {
                label: "prefer_dgpu without DXGI ranks falls back to VRAM",
                adapters: {
                    let mut a = vec![intel(true), nvidia(false)];
                    for x in &mut a {
                        x.high_perf_rank = None;
                    }
                    a
                },
                opts: opts(None, true),
                want_name: "NVIDIA GeForce RTX 3050 Laptop GPU",
                want_reason: Reason::PreferDgpu,
                want_chain: &["h264_nvenc", "libx264"],
            },
            Case {
                label: "explicit adapter by name beats prefer_dgpu",
                adapters: vec![nvidia(false), intel(true)],
                opts: opts(Some("intel"), true),
                want_name: "Intel(R) UHD Graphics",
                want_reason: Reason::Explicit,
                want_chain: &["h264_qsv", "libx264"],
            },
            Case {
                label: "explicit adapter by LUID (0x)",
                adapters: vec![nvidia(false), intel(true)],
                opts: opts(Some("0x1000"), false),
                want_name: "NVIDIA GeForce RTX 3050 Laptop GPU",
                want_reason: Reason::Explicit,
                want_chain: &["h264_nvenc", "libx264"],
            },
            Case {
                label: "explicit adapter by LUID (high:low)",
                adapters: vec![nvidia(false), intel(true)],
                opts: opts(Some("0:2000"), false),
                want_name: "Intel(R) UHD Graphics",
                want_reason: Reason::Explicit,
                want_chain: &["h264_qsv", "libx264"],
            },
            Case {
                label: "no primary found: high-performance adapter",
                adapters: vec![intel(false), nvidia(false)],
                opts: opts(None, false),
                want_name: "NVIDIA GeForce RTX 3050 Laptop GPU",
                want_reason: Reason::HighPerformanceFallback,
                want_chain: &["h264_nvenc", "libx264"],
            },
        ];
        for c in cases {
            let d = decide(&c.adapters, &c.opts).unwrap_or_else(|e| panic!("{}: {e}", c.label));
            assert_eq!(d.adapter.name, c.want_name, "{}", c.label);
            assert_eq!(d.reason, c.want_reason, "{}", c.label);
            assert_eq!(d.encoder_chain, c.want_chain, "{}", c.label);
            assert_eq!(d.hdr_chain, hdr_for(c.want_chain), "{}", c.label);
        }
    }

    #[test]
    fn missing_adapter_is_an_error_listing_the_available_ones() {
        let adapters = vec![nvidia(true), intel(false), warp()];
        let e = decide(&adapters, &opts(Some("radeon"), false)).unwrap_err();
        match &e {
            PolicyError::AdapterNotFound { spec, available } => {
                assert_eq!(spec, "radeon");
                assert_eq!(available.len(), 2, "software adapter is not offered");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(e.to_string().contains("NVIDIA"));
        // A LUID that does not exist, and the software adapter's LUID, are both missing.
        assert!(decide(&adapters, &opts(Some("0xdead"), false)).is_err());
        assert!(decide(&adapters, &opts(Some("0x9000"), false)).is_err());
    }

    #[test]
    fn only_software_adapters_means_no_adapters() {
        assert_eq!(
            decide(&[warp()], &opts(None, false)).unwrap_err(),
            PolicyError::NoAdapters
        );
        assert_eq!(
            decide(&[], &opts(None, true)).unwrap_err(),
            PolicyError::NoAdapters
        );
    }

    #[test]
    fn unknown_vendor_uses_software_encoder_only() {
        let mut g = gpu("Mystery GPU", 0xABCD, 5, 100, 0, 0, true);
        g.name = "Mystery GPU".into();
        let d = decide(&[g], &opts(None, false)).unwrap();
        assert_eq!(d.hw_encoder, None);
        assert_eq!(d.encoder_chain, ["libx264"]);
        assert!(d.hdr_chain.is_empty(), "no software HDR10 encoder");
    }

    #[test]
    fn hdr_chain_per_vendor() {
        assert_eq!(hdr_encoder_for_vendor(VENDOR_NVIDIA), Some("hevc_nvenc"));
        assert_eq!(hdr_encoder_for_vendor(VENDOR_INTEL), Some("hevc_qsv"));
        assert_eq!(hdr_encoder_for_vendor(VENDOR_AMD), Some("hevc_amf"));
        assert_eq!(hdr_encoder_for_vendor(VENDOR_MICROSOFT), None);
        assert_eq!(hdr_encoder_for_vendor(0xABCD), None);
        // Each H.264 encoder has an HEVC sibling of the same vendor.
        for v in [VENDOR_NVIDIA, VENDOR_INTEL, VENDOR_AMD] {
            let h264 = hw_encoder_for_vendor(v).unwrap();
            let hevc = hdr_encoder_for_vendor(v).unwrap();
            assert_eq!(
                h264.split_once('_').unwrap().1,
                hevc.split_once('_').unwrap().1
            );
        }
    }

    #[test]
    fn luid_parsing() {
        assert_eq!(parse_luid("0x1a2b"), Some(0x1a2b));
        assert_eq!(parse_luid("1:2"), Some(0x1_0000_0002));
        assert_eq!(parse_luid("0x1:0xff"), Some(0x1_0000_00ff));
        assert_eq!(parse_luid("nvidia"), None);
        assert_eq!(parse_luid("0xzz"), None);
    }

    #[test]
    fn name_matching_is_case_insensitive_and_prefers_more_vram() {
        let a = vec![
            gpu("NVIDIA A", VENDOR_NVIDIA, 1, 100, 0, 0, false),
            gpu("NVIDIA B", VENDOR_NVIDIA, 2, 8000, 1, 1, false),
        ];
        let m = match_adapters(&a, "nvidia");
        assert_eq!(m[0].name, "NVIDIA B");
        assert!(match_adapters(&a, "").is_empty());
    }
}
