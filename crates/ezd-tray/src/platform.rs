//! Small operating-system helpers: opening folders and links, adb lookup, the display driver.

use crate::ui::view::{AdbInfo, InputAccess, InputFixState, RepairState, VddInfo};
use std::path::Path;
use std::process::Command;

/// Opens a folder or a URL with the system's default handler. Failures are logged.
pub fn open(target: &str) {
    let result = if cfg!(windows) {
        Command::new("explorer.exe").arg(target).spawn()
    } else {
        Command::new("xdg-open").arg(target).spawn()
    };
    match result {
        Ok(mut child) => {
            // Reap it in the background; the opener usually returns at once.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::warn!("cannot open {target}: {e}"),
    }
}

pub fn open_path(path: &Path) {
    open(&path.display().to_string());
}

/// The adb that the service will use, and whether it is the bundled copy.
pub fn adb_info() -> AdbInfo {
    let path = ezd_link::adb::find_adb();
    let bundled = match (&path, crate::paths::exe_dir()) {
        (Some(p), Some(dir)) => p.starts_with(dir.join("adb")),
        _ => false,
    };
    AdbInfo { path, bundled }
}

/// State of the virtual display driver; `None` outside Windows (no such page).
pub fn vdd_info() -> Option<VddInfo> {
    #[cfg(windows)]
    {
        let v = ezd_host_windows::vdd_status();
        Some(VddInfo {
            present: v
                .pointer("/pipe/present")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            owned: v
                .get("owned")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            driver_dir: v
                .get("driver_dir")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Runs `ezd-host.exe vdd install --driver-dir "<exe dir>\driver"` elevated and waits for it.
/// Blocking: call it from a background thread.
pub fn repair_driver() -> RepairState {
    #[cfg(windows)]
    {
        let Some(dir) = crate::paths::exe_dir() else {
            return RepairState::Failed("cannot find the install folder".into());
        };
        let exe = dir.join("ezd-host.exe");
        let args = format!(
            "vdd install --driver-dir \"{}\"",
            dir.join("driver").display()
        );
        match ezd_host_windows::run_elevated(&exe, &args) {
            Ok(code) => RepairState::Finished(code),
            Err(e) if e == ezd_host_windows::ELEVATION_CANCELLED => RepairState::Cancelled,
            Err(e) => RepairState::Failed(e),
        }
    }
    #[cfg(not(windows))]
    {
        RepairState::Failed("the display driver is managed on Windows only".into())
    }
}

/// The system's permission to create input devices, behind a trait so tests do not touch it.
pub trait InputPermission: Send + Sync {
    fn access(&self) -> InputAccess;
    /// Tries to grant the permission, asking for authorization. Blocking: call it from a
    /// background thread. Returns the outcome to show.
    fn enable(&self) -> InputFixState;
}

/// The real thing: `/dev/uinput` plus polkit on Linux; nothing to grant elsewhere.
pub struct SystemInput;

impl InputPermission for SystemInput {
    fn access(&self) -> InputAccess {
        #[cfg(target_os = "linux")]
        {
            match crate::uinput::access() {
                crate::uinput::Access::Ok => InputAccess::Ok,
                crate::uinput::Access::Denied => InputAccess::Denied,
                crate::uinput::Access::Missing => InputAccess::Missing,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            InputAccess::Ok
        }
    }

    fn enable(&self) -> InputFixState {
        #[cfg(target_os = "linux")]
        {
            use crate::uinput::InstallError;
            match crate::uinput::install() {
                Ok(()) => InputFixState::Done,
                Err(InstallError::Cancelled) => InputFixState::Cancelled,
                Err(InstallError::NoAgent) => InputFixState::NoAgent,
                Err(InstallError::StillDenied) => InputFixState::StillDenied,
                Err(InstallError::Failed(code)) => InputFixState::Failed(code),
                Err(InstallError::Io(e)) => InputFixState::Error(e),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            InputFixState::Done
        }
    }
}
