//! Thin dispatch to the platform crate of the current target.

use anyhow::Result;
use ezd_host_core::{PlatformOptions, VideoSource};

#[cfg(target_os = "linux")]
use ezd_host_linux as imp;
#[cfg(windows)]
use ezd_host_windows as imp;

/// Builds the virtual-display source of this platform.
pub fn create_virtual_source(opts: &PlatformOptions) -> Result<Box<dyn VideoSource>> {
    #[cfg(any(target_os = "linux", windows))]
    {
        Ok(imp::create_virtual_source(opts)?)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = opts;
        anyhow::bail!("no virtual display backend for this platform")
    }
}

/// Platform part of the diagnostics report.
pub fn diag_section() -> serde_json::Value {
    #[cfg(any(target_os = "linux", windows))]
    {
        imp::diag_section()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        serde_json::json!({ "status": "no backend for this platform" })
    }
}
