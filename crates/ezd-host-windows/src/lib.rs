//! Windows host backend: GPU policy, virtual display (VDD), DXGI Desktop
//! Duplication capture, FFmpeg D3D11 hardware encode, cursor channel and `diag`.
//!
//! Contract (see `ezd_host_core::platform`): [`create_virtual_source`] builds the
//! capture + encode pipeline for a virtual display and [`diag_section`] feeds the
//! `platform` part of `ezd-host diag`.
//!
//! The pure logic (GPU policy, `vdd_settings.xml` editing, pipe protocol, pointer
//! shape conversion) is platform independent and unit-tested everywhere; the
//! Windows glue lives in the `cfg(windows)` module `win`. The hardware encoder
//! needs FFmpeg and is behind the default-on `ffmpeg` feature.

pub mod annexb;
pub mod cursor_shape;
pub mod ident;
pub mod input_map;
pub mod pipe_proto;
pub mod policy;
pub mod vdd_install;
pub mod vdd_xml;

#[cfg(windows)]
#[allow(unsafe_code)]
mod win;

pub use ezd_host_core::PlatformOptions;
use ezd_host_core::{InputBackend, PlatformError, VideoSource};
use std::sync::Arc;

/// Creates the virtual display + capture + encode pipeline.
pub fn create_virtual_source(
    opts: &PlatformOptions,
) -> Result<Box<dyn VideoSource>, PlatformError> {
    #[cfg(windows)]
    {
        win::create_virtual_source(opts)
    }
    #[cfg(not(windows))]
    {
        let _ = opts;
        Err(PlatformError::Unimplemented("non-Windows host"))
    }
}

/// Input injection backend (`None` = not available).
pub fn input_backend() -> Option<Arc<dyn InputBackend>> {
    #[cfg(windows)]
    {
        Some(Arc::new(win::input::WinInputBackend::new()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Platform-specific diagnostics for `ezd-host diag`.
pub fn diag_section() -> serde_json::Value {
    #[cfg(windows)]
    {
        win::diag::report()
    }
    #[cfg(not(windows))]
    {
        serde_json::json!({ "status": "not a Windows host" })
    }
}

/// Keeps child processes (and the `adb` server they start) from inheriting our stdio, which
/// would hold a pipe reading our output open after we exit. Call once at startup.
#[cfg(windows)]
pub fn isolate_std_handles() {
    win::util::isolate_std_handles()
}

/// `ezd-host vdd status`: report on the VDD installation (Windows only).
#[cfg(windows)]
pub fn vdd_status() -> serde_json::Value {
    win::vdd::status_json()
}

/// `ezd-host vdd setup`: grants BUILTIN\Users modify on `vdd_settings.xml`,
/// optionally sets the GPU and pre-adds modes, then reloads the driver.
/// Returns the lines to print.
#[cfg(windows)]
pub fn vdd_setup(gpu: Option<&str>, modes: &[vdd_xml::Mode]) -> Result<Vec<String>, String> {
    win::vdd::setup(gpu, modes)
}

/// `ezd-host vdd revert`: removes the grant made by `setup`.
#[cfg(windows)]
pub fn vdd_revert() -> Result<Vec<String>, String> {
    win::vdd::revert()
}

/// `ezd-host vdd install`: installs the bundled VDD from `driver_dir` and records ownership.
/// Needs elevation.
#[cfg(windows)]
pub fn vdd_install(
    driver_dir: &std::path::Path,
) -> Result<vdd_install::InstallOutcome, vdd_install::InstallError> {
    win::vdd_install::install(driver_dir)
}

/// `ezd-host vdd uninstall`: removes a VDD installed by `vdd_install`. The `nefconw.exe` comes
/// from `driver_dir`, or from the location recorded at install time when `None`. Needs elevation.
#[cfg(windows)]
pub fn vdd_uninstall(
    driver_dir: Option<&std::path::Path>,
) -> Result<vdd_install::UninstallOutcome, vdd_install::InstallError> {
    win::vdd_install::uninstall(driver_dir)
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    #[test]
    fn non_windows_reports_unavailable() {
        use super::*;
        assert!(matches!(
            create_virtual_source(&PlatformOptions::default()),
            Err(PlatformError::Unimplemented(_))
        ));
        assert!(diag_section().get("status").is_some());
    }
}
