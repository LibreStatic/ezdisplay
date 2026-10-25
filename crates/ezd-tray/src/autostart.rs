//! Starting with the session: `HKCU\...\Run` (+ StartupApproved) on Windows,
//! `~/.config/autostart/ezdisplay.desktop` on Linux. The entry launches the executable without
//! arguments (the tray).

use auto_launch::{AutoLaunch, AutoLaunchBuilder};

const NAME: &str = "ezdisplay";

fn handle() -> Result<AutoLaunch, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    AutoLaunchBuilder::new()
        .set_app_name(NAME)
        .set_app_path(&exe.to_string_lossy())
        .build()
        .map_err(|e| e.to_string())
}

/// Whether the entry exists now (the checkbox state comes from here, not from the config).
pub fn is_enabled() -> bool {
    handle()
        .and_then(|a| a.is_enabled().map_err(|e| e.to_string()))
        .unwrap_or(false)
}

/// Creates or removes the entry.
pub fn set(enabled: bool) -> Result<(), String> {
    let auto = handle()?;
    if enabled {
        auto.enable().map_err(|e| e.to_string())
    } else if auto.is_enabled().unwrap_or(false) {
        auto.disable().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}
