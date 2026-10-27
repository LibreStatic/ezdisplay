//! Thin dispatch to the platform crate of the current target.

use crate::error::ServiceError;
use ezd_host_core::{AudioSource, InputBackend, PlatformOptions, VideoSource};
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

/// A PC audio capture source of this platform, if it has one. Cheap: capture starts in
/// [`AudioSource::start`].
pub fn audio_source() -> Option<Box<dyn AudioSource>> {
    #[cfg(any(target_os = "linux", windows))]
    {
        imp::audio_source()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

/// Puts back what a crashed session left changed in the PC audio output (Windows mutes it).
pub fn recover_audio_after_crash() {
    #[cfg(windows)]
    imp::recover_audio_after_crash();
}

/// Removes a virtual display left on the desktop for a quick reconnect (Windows).
pub fn release_virtual_display() {
    #[cfg(windows)]
    imp::release_virtual_display();
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
