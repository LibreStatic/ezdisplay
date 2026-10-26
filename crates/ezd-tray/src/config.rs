//! The settings file: the service settings plus the tray's own.

use ezd_host_service::ServiceConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Everything stored in `config.toml`. Missing keys take their defaults and unknown keys are
/// ignored, so files survive upgrades and downgrades.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayConfig {
    /// Start streaming (listening for the tablet) when ezdisplay opens.
    pub start_on_launch: bool,
    /// Windows only: run the app elevated (through its scheduled task) so that pen and touch
    /// reach administrator windows. Ignored elsewhere.
    pub run_elevated: bool,
    /// Take the accent colour of the window from the system; off keeps the built-in colour.
    pub use_system_accent: bool,
    /// The host service settings.
    pub service: ServiceConfig,
}

impl Default for TrayConfig {
    fn default() -> Self {
        TrayConfig {
            start_on_launch: true,
            run_elevated: true,
            use_system_accent: true,
            service: ServiceConfig::default(),
        }
    }
}

/// Longest delay before the virtual monitor is removed.
pub const MAX_DETACH_GRACE_MS: u32 = 60_000;

impl TrayConfig {
    /// Brings out-of-range values (hand-edited files) back into what the service accepts.
    pub fn sanitized(mut self) -> Self {
        self.service.scale_permille = self.service.scale_permille.clamp(250, 1000);
        self.service.video_window = self.service.video_window.clamp(1, 8);
        self.service.detach_grace_ms = self.service.detach_grace_ms.min(MAX_DETACH_GRACE_MS);
        self
    }
}

/// Result of reading the file.
#[derive(Debug)]
pub struct Loaded {
    pub config: TrayConfig,
    /// Why the file could not be used (the defaults are in effect then).
    pub error: Option<String>,
}

/// Reads `path`. A missing file gives the defaults silently; a broken one gives the defaults and
/// the reason. The broken file is left untouched until the next [`save`].
pub fn load(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<TrayConfig>(&text) {
            Ok(config) => Loaded {
                config: config.sanitized(),
                error: None,
            },
            Err(e) => Loaded {
                config: TrayConfig::default(),
                error: Some(format!("{}: {e}", path.display())),
            },
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded {
            config: TrayConfig::default(),
            error: None,
        },
        Err(e) => Loaded {
            config: TrayConfig::default(),
            error: Some(format!("{}: {e}", path.display())),
        },
    }
}

/// Writes `config` to `path` (through a temporary file, so a crash never leaves half a file).
pub fn save(path: &Path, config: &TrayConfig) -> std::io::Result<()> {
    let text = toml::to_string_pretty(config).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp: PathBuf = path.to_path_buf();
    tmp.set_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezd_host_service::{AdbMode, PenChoice};

    #[test]
    fn defaults() {
        let c = TrayConfig::default();
        assert!(c.start_on_launch);
        assert!(c.run_elevated);
        assert!(c.use_system_accent);
        assert_eq!(c.service, ServiceConfig::default());
        let empty: TrayConfig = toml::from_str("").unwrap();
        assert_eq!(empty, c);
    }

    #[test]
    fn unknown_keys_are_ignored_and_missing_ones_default() {
        let c: TrayConfig = toml::from_str(
            r#"
            future_top_level = 1
            start_on_launch = false
            [service]
            adb = "off"
            scale_permille = 750
            future_service_key = "x"
            "#,
        )
        .unwrap();
        assert!(!c.start_on_launch);
        assert!(c.run_elevated);
        assert_eq!(c.service.adb, AdbMode::Off);
        assert_eq!(c.service.scale_permille, 750);
        assert_eq!(c.service.max_refresh_mhz, 120_000);
    }

    #[test]
    fn round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let c = TrayConfig {
            start_on_launch: false,
            run_elevated: false,
            use_system_accent: false,
            service: ServiceConfig {
                adb: AdbMode::Serial("R52X204YJNW".into()),
                bitrate_kbps: Some(40_000),
                encoder: Some("h264_nvenc".into()),
                pen_mode: PenChoice::Mouse,
                telemetry: true,
                ..ServiceConfig::default()
            },
        };
        save(&path, &c).unwrap();
        let back = load(&path);
        assert!(back.error.is_none());
        assert_eq!(back.config, c);
    }

    #[test]
    fn mapping_to_the_service_config() {
        let toml = r#"
            [service]
            listen = "127.0.0.1:4000"
            input = false
            prefer_dgpu = true
        "#;
        let c: TrayConfig = toml::from_str(toml).unwrap();
        assert_eq!(c.service.listen.port(), 4000);
        assert!(!c.service.input && c.service.prefer_dgpu);
    }

    #[test]
    fn broken_file_gives_defaults_and_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "start_on_launch = [oops").unwrap();
        let loaded = load(&path);
        assert!(loaded.error.is_some());
        assert_eq!(loaded.config, TrayConfig::default());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "start_on_launch = [oops"
        );
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("nope.toml"));
        assert!(loaded.error.is_none());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let c: TrayConfig =
            toml::from_str("[service]\nscale_permille = 5000\nvideo_window = 99").unwrap();
        let c = c.sanitized();
        assert_eq!(
            (c.service.scale_permille, c.service.video_window),
            (1000, 8)
        );
    }

    #[cfg(feature = "wifi")]
    #[test]
    fn the_wifi_listener_is_off_by_default_and_round_trips() {
        use ezd_host_service::WifiConfig;
        assert_eq!(TrayConfig::default().service.wifi, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = TrayConfig::default();
        c.service.wifi = Some(WifiConfig::default());
        save(&path, &c).unwrap();
        assert_eq!(load(&path).config, c);
        let from_file: TrayConfig =
            toml::from_str("[service.wifi]\nlisten = \"0.0.0.0:4000\"").unwrap();
        let wifi = from_file.service.wifi.unwrap();
        assert_eq!(wifi.listen.port(), 4000);
        assert!(!wifi.pair_on_start);
    }
}
