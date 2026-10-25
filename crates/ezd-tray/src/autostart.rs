//! Starting with the session: `~/.config/autostart/ezdisplay.desktop` on Linux. On Windows it is
//! the logon trigger of the `ezdisplay` scheduled task, which is also how the app is launched
//! elevated (see `launch`); the old `HKCU\...\Run` entry is migrated away. The entry launches the
//! executable without arguments (the tray).

use auto_launch::{AutoLaunch, AutoLaunchBuilder};

const NAME: &str = "ezdisplay";

fn handle() -> Result<AutoLaunch, String> {
    let exe = crate::paths::launcher();
    AutoLaunchBuilder::new()
        .set_app_name(NAME)
        .set_app_path(&exe.to_string_lossy())
        .build()
        .map_err(|e| e.to_string())
}

/// Whether the (Linux) entry or the (Windows) legacy Run entry exists.
fn entry_exists() -> bool {
    handle()
        .and_then(|a| a.is_enabled().map_err(|e| e.to_string()))
        .unwrap_or(false)
}

fn remove_entry() -> Result<(), String> {
    if entry_exists() {
        handle()?.disable().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

/// Whether autostart is on now (the checkbox state comes from here, not from the config).
#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    entry_exists()
}

/// Creates or removes the entry. `run_elevated` only matters on Windows.
#[cfg(not(windows))]
pub fn set(enabled: bool, run_elevated: bool) -> Result<(), String> {
    let _ = run_elevated;
    if enabled {
        handle()?.enable().map_err(|e| e.to_string())
    } else {
        remove_entry()
    }
}

/// Windows only; nothing to do elsewhere.
#[cfg(not(windows))]
pub fn set_run_elevated(_run_elevated: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
pub use windows_task::{is_enabled, migrate_legacy, set, set_run_elevated};

#[cfg(windows)]
mod windows_task {
    use super::{entry_exists, remove_entry};
    use ezd_host_windows as win;

    /// The task has a logon trigger. A legacy Run entry counts too until it is migrated.
    pub fn is_enabled() -> bool {
        win::task_autostart_enabled() || entry_exists()
    }

    /// Registers the task with or without the logon trigger and drops the legacy Run entry (it
    /// would start a second, unelevated copy).
    pub fn set(enabled: bool, run_elevated: bool) -> Result<(), String> {
        let r = if win::is_elevated() {
            register(enabled, run_elevated)
        } else {
            elevated_helper(if enabled {
                "--autostart on"
            } else {
                "--autostart off"
            })
        };
        let _ = remove_entry();
        r
    }

    /// Re-registers the task with the new run level, keeping its autostart state.
    pub fn set_run_elevated(run_elevated: bool) -> Result<(), String> {
        if win::is_elevated() {
            register(
                win::task_autostart_enabled() || entry_exists(),
                run_elevated,
            )
        } else {
            elevated_helper(if run_elevated {
                "--elevation on"
            } else {
                "--elevation off"
            })
        }
    }

    /// Moves a legacy Run entry to the task's logon trigger. Needs an elevated process; a no-op
    /// when there is no legacy entry.
    pub fn migrate_legacy(run_elevated: bool) {
        if !entry_exists() || !win::is_elevated() {
            return;
        }
        match register(true, run_elevated) {
            Ok(()) => {
                let _ = remove_entry();
                tracing::info!("moved the autostart entry to the scheduled task");
            }
            Err(e) => tracing::warn!("cannot move the autostart entry to the task: {e}"),
        }
    }

    fn register(autostart: bool, run_elevated: bool) -> Result<(), String> {
        win::task_register(&crate::paths::launcher(), autostart, run_elevated)
    }

    /// Runs this executable elevated with `args` (UAC prompt) and waits for it.
    fn elevated_helper(args: &str) -> Result<(), String> {
        match win::run_elevated(&crate::paths::launcher(), args)? {
            0 => Ok(()),
            code => Err(format!("the elevated helper failed (code {code})")),
        }
    }
}
