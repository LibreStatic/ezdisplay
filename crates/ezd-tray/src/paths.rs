//! Where the app keeps its files.

use std::path::PathBuf;

/// Directory name under the platform's config/data/state directories.
const DIR: &str = "ezdisplay";

/// `config.toml`: `%APPDATA%\ezdisplay` on Windows, `$XDG_CONFIG_HOME/ezdisplay` on Linux.
pub fn default_config_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(DIR)
        .join("config.toml")
}

/// Log files: `%LOCALAPPDATA%\ezdisplay\logs` on Windows, `$XDG_STATE_HOME/ezdisplay/logs` on
/// Linux.
pub fn log_dir() -> PathBuf {
    let base = if cfg!(windows) {
        dirs::data_local_dir()
    } else {
        dirs::state_dir().or_else(dirs::data_local_dir)
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join(DIR)
        .join("logs")
}

/// Directory of the running executable.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
}

/// `<exe dir>/licenses`, when the installer shipped it.
pub fn licenses_dir() -> Option<PathBuf> {
    exe_dir().map(|d| d.join("licenses")).filter(|d| d.is_dir())
}
