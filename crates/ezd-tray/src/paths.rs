//! Where the app keeps its files.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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

/// The file to start the app from at login. Inside an AppImage the running executable lives in a
/// temporary mount, so the AppImage runtime's `$APPIMAGE` (the real file) wins.
pub fn launcher() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    launcher_from(std::env::var_os("APPIMAGE"), exe, Path::is_file)
}

fn launcher_from(
    appimage: Option<OsString>,
    exe: PathBuf,
    is_file: impl Fn(&Path) -> bool,
) -> PathBuf {
    match appimage.map(PathBuf::from) {
        Some(p) if p.is_absolute() && is_file(&p) => p,
        _ => exe,
    }
}

/// `<exe dir>/licenses`, when the installer shipped it.
pub fn licenses_dir() -> Option<PathBuf> {
    exe_dir().map(|d| d.join("licenses")).filter(|d| d.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from("/tmp/.mount_ezdXYZ/usr/bin/ezdisplay")
    }

    #[test]
    fn an_absolute_existing_appimage_wins() {
        let got = launcher_from(Some("/home/u/ezdisplay.AppImage".into()), exe(), |_| true);
        assert_eq!(got, PathBuf::from("/home/u/ezdisplay.AppImage"));
    }

    #[test]
    fn a_relative_or_missing_appimage_falls_back_to_the_executable() {
        assert_eq!(
            launcher_from(Some("ezdisplay.AppImage".into()), exe(), |_| true),
            exe()
        );
        assert_eq!(
            launcher_from(Some("/home/u/gone.AppImage".into()), exe(), |_| false),
            exe()
        );
    }

    #[test]
    fn without_appimage_the_executable_is_used() {
        assert_eq!(launcher_from(None, exe(), |_| true), exe());
    }
}
