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

pub mod accent;
pub mod annexb;
pub mod audio_math;
pub mod cursor_shape;
pub mod display_topology;
pub mod hdr_math;
pub mod ident;
pub mod input_map;
pub mod pipe_proto;
pub mod policy;
pub mod task_xml;
pub mod touch_keyboard;
pub mod vdd_install;
pub mod vdd_xml;

#[cfg(windows)]
#[allow(unsafe_code)]
mod win;

pub use ezd_host_core::PlatformOptions;
use ezd_host_core::{AudioSource, InputBackend, PlatformError, VideoSource};
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

/// System audio capture (WASAPI loopback of the default output device); `None` on other
/// targets. Creating it does no device work: that starts in `AudioSource::start`. The PC keeps
/// playing the audio locally.
pub fn audio_source() -> Option<Box<dyn AudioSource>> {
    #[cfg(windows)]
    {
        Some(Box::new(win::audio::WasapiLoopback::new()))
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

/// Takes the virtual monitor off the desktop now if a session ended and its grace period is
/// still running. Call when the host stops serving.
pub fn release_virtual_display() {
    #[cfg(windows)]
    win::vdd::detach_now();
}

/// Keeps child processes (and the `adb` server they start) from inheriting our stdio, which
/// would hold a pipe reading our output open after we exit. Call once at startup.
#[cfg(windows)]
pub fn isolate_std_handles() {
    win::util::isolate_std_handles()
}

/// Makes the process Per-Monitor DPI aware v2 (screen capture and input injection depend on real
/// pixels). Call first thing in `main`, before any window exists. Returns what happened.
#[cfg(windows)]
pub fn ensure_dpi_aware() -> String {
    win::util::ensure_dpi_aware()
}

/// The accent colour (RGB) the user picked in Windows settings, if it can be read.
#[cfg(windows)]
pub fn system_accent() -> Option<[u8; 3]> {
    win::util::system_accent()
}

/// Message text of the `Err` that [`run_elevated`] returns when the user declined the UAC prompt.
pub const ELEVATION_CANCELLED: &str = "cancelled";

/// Waits up to `timeout` for window messages of the calling thread and dispatches them: the loop
/// a thread that owns a tray icon must run.
#[cfg(windows)]
pub fn pump_messages(timeout: std::time::Duration) {
    win::shell::pump_messages(timeout)
}

/// Runs `exe args` elevated (UAC prompt, hidden), waits for it and returns its exit code.
/// `Err(ELEVATION_CANCELLED)` means the user declined the prompt.
#[cfg(windows)]
pub fn run_elevated(exe: &std::path::Path, args: &str) -> Result<u32, String> {
    win::shell::run_elevated(exe, args)
}

/// True if this process runs elevated (administrator token).
#[cfg(windows)]
pub fn is_elevated() -> bool {
    win::vdd::is_elevated()
}

/// SID string of the account this process runs as.
#[cfg(windows)]
pub fn current_user_sid() -> Option<String> {
    win::task::current_user_sid()
}

/// Creates or replaces the `ezdisplay` scheduled task that launches `exe` (see [`task_xml`]).
/// Needs elevation.
#[cfg(windows)]
pub fn task_register(exe: &std::path::Path, autostart: bool, elevated: bool) -> Result<(), String> {
    win::task::register(exe, autostart, elevated)
}

/// Removes the `ezdisplay` scheduled task (succeeds when there is none).
#[cfg(windows)]
pub fn task_delete() -> Result<(), String> {
    win::task::delete()
}

/// Whether the `ezdisplay` scheduled task exists.
#[cfg(windows)]
pub fn task_exists() -> bool {
    win::task::exists()
}

/// Whether the task exists and runs with the highest privileges (elevated).
#[cfg(windows)]
pub fn task_runs_elevated() -> bool {
    win::task::runs_elevated()
}

/// Whether the task exists and starts the app at logon.
#[cfg(windows)]
pub fn task_autostart_enabled() -> bool {
    win::task::autostart_enabled()
}

/// Starts the `ezdisplay` scheduled task now.
#[cfg(windows)]
pub fn task_run() -> Result<(), String> {
    win::task::run()
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
