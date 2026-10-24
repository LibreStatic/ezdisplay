//! Thin dispatch to the platform crate of the current target.

use crate::error::ServiceError;
use ezd_host_core::{InputBackend, PlatformOptions, VideoSource};
use std::sync::Arc;

#[cfg(target_os = "linux")]
use ezd_host_linux as imp;
#[cfg(windows)]
use ezd_host_windows as imp;

/// Builds the virtual-display source of this platform.
pub fn create_virtual_source(opts: &PlatformOptions) -> Result<Box<dyn VideoSource>, ServiceError> {
    #[cfg(any(target_os = "linux", windows))]
    {
        imp::create_virtual_source(opts).map_err(|e| ServiceError::Source(e.to_string()))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = opts;
        Err(ServiceError::UnsupportedPlatform)
    }
}

/// Input injection backend of this platform, if it has one.
pub fn input_backend() -> Option<Arc<dyn InputBackend>> {
    #[cfg(any(target_os = "linux", windows))]
    {
        imp::input_backend()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
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
