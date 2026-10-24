//! Platform independent parts of the VDD install/uninstall: the `nefconw` command lines, its
//! exit codes and the hardware-ID matching of SetupAPI's `REG_MULTI_SZ` values. The Windows
//! glue that runs them is `win::vdd_install`.

use std::fmt;
use std::path::Path;

/// Hardware ID of the VDD root device.
pub const HARDWARE_ID: &str = r"Root\MttVDD";
/// File name of the driver package's INF.
pub const INF_NAME: &str = "MttVDD.inf";
/// Setup class GUID of display adapters.
pub const DISPLAY_CLASS_GUID: &str = "{4D36E968-E325-11CE-BFC1-08002BE10318}";
/// `ERROR_SUCCESS_REBOOT_REQUIRED`: the operation worked but a reboot is needed.
pub const EXIT_REBOOT_REQUIRED: i32 = 3010;

/// Successful `nefconw` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NefconStatus {
    Ok,
    RebootRequired,
}

/// Maps a `nefconw` exit code: 0 is success, 3010 is success with a reboot pending, anything
/// else is an error carrying the code.
pub fn map_exit_code(code: i32) -> Result<NefconStatus, i32> {
    match code {
        0 => Ok(NefconStatus::Ok),
        EXIT_REBOOT_REQUIRED => Ok(NefconStatus::RebootRequired),
        other => Err(other),
    }
}

fn log_arg(log: &Path) -> String {
    format!("--default-log-file={}", log.display())
}

/// `nefconw install <INF> Root\MttVDD --no-duplicates --remove-duplicates`: creates the root
/// device and installs the driver; with an existing node it only updates the driver. nefcon
/// takes this devcon syntax only with `install` as the first argument, so the log option goes
/// last (here and in the other commands, for consistency).
pub fn install_args(inf: &Path, log: &Path) -> Vec<String> {
    vec![
        "install".into(),
        inf.display().to_string(),
        HARDWARE_ID.into(),
        "--no-duplicates".into(),
        "--remove-duplicates".into(),
        log_arg(log),
    ]
}

/// `nefconw --remove-device-node`: removes every VDD device (and the driver once unused).
pub fn remove_device_args(log: &Path) -> Vec<String> {
    vec![
        "--remove-device-node".into(),
        "--hardware-id".into(),
        HARDWARE_ID.into(),
        "--class-guid".into(),
        DISPLAY_CLASS_GUID.into(),
        log_arg(log),
    ]
}

/// `nefconw --remove-driver-store-package`: purges leftover packages of the VDD INF.
pub fn remove_package_args(log: &Path) -> Vec<String> {
    vec![
        "--remove-driver-store-package".into(),
        "--inf-name".into(),
        INF_NAME.to_lowercase(),
        "--class-guid".into(),
        DISPLAY_CLASS_GUID.into(),
        log_arg(log),
    ]
}

/// Splits a `REG_MULTI_SZ` buffer (UTF-16, NUL separated, double-NUL terminated) into strings.
/// Tolerates a missing terminator and stops at the first empty entry.
pub fn parse_multi_sz(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// Whether any of `ids` equals `wanted`, ignoring case.
pub fn has_hardware_id<S: AsRef<str>>(ids: &[S], wanted: &str) -> bool {
    ids.iter()
        .any(|i| i.as_ref().trim().eq_ignore_ascii_case(wanted))
}

/// Why an install or uninstall failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// The process is not elevated.
    NotElevated,
    /// Anything else, with an English description.
    Failed(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::NotElevated => f.write_str("administrator rights are required"),
            InstallError::Failed(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<String> for InstallError {
    fn from(s: String) -> Self {
        InstallError::Failed(s)
    }
}

/// Result of `install`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The driver is installed and owned by ezdisplay.
    Installed { reboot_required: bool },
    /// A VDD installed by someone else exists; nothing was changed.
    PreexistingLeftAlone,
}

/// Result of `uninstall`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallOutcome {
    Removed {
        reboot_required: bool,
    },
    /// ezdisplay did not install the VDD; nothing was changed.
    NotOurs,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn exit_codes() {
        assert_eq!(map_exit_code(0), Ok(NefconStatus::Ok));
        assert_eq!(map_exit_code(3010), Ok(NefconStatus::RebootRequired));
        assert_eq!(map_exit_code(1), Err(1));
        assert_eq!(map_exit_code(-1), Err(-1));
        assert_eq!(map_exit_code(3011), Err(3011));
    }

    #[test]
    fn multi_sz_parsing() {
        let mut buf = w(r"Root\MttVDD");
        buf.push(0);
        buf.extend(w(r"Other\Id"));
        buf.extend([0, 0]);
        assert_eq!(parse_multi_sz(&buf), vec![r"Root\MttVDD", r"Other\Id"]);
        assert_eq!(parse_multi_sz(&w("abc")), vec!["abc"]);
        assert!(parse_multi_sz(&[0, 0]).is_empty());
        assert!(parse_multi_sz(&[]).is_empty());
        // Trailing garbage after the terminator is ignored.
        let mut buf = w("a");
        buf.extend([0, 0]);
        buf.extend(w("junk"));
        assert_eq!(parse_multi_sz(&buf), vec!["a"]);
    }

    #[test]
    fn hardware_id_matching() {
        let ids = vec!["ROOT\\MTTVDD".to_string(), "x".to_string()];
        assert!(has_hardware_id(&ids, HARDWARE_ID));
        assert!(!has_hardware_id(&["Root\\Other"], HARDWARE_ID));
        assert!(!has_hardware_id::<&str>(&[], HARDWARE_ID));
        assert!(has_hardware_id(&[" root\\mttvdd "], HARDWARE_ID));
    }

    #[test]
    fn argument_lists() {
        let log = PathBuf::from(r"C:\ProgramData\ezdisplay\logs\vdd.log");
        let inf = PathBuf::from(r"C:\App\driver\MttVDD.inf");
        let log_opt = r"--default-log-file=C:\ProgramData\ezdisplay\logs\vdd.log";
        // nefcon recognises the devcon syntax only when `install` is the first argument.
        assert_eq!(
            install_args(&inf, &log),
            [
                "install",
                r"C:\App\driver\MttVDD.inf",
                r"Root\MttVDD",
                "--no-duplicates",
                "--remove-duplicates",
                log_opt
            ]
        );
        assert_eq!(
            remove_device_args(&log),
            [
                "--remove-device-node",
                "--hardware-id",
                HARDWARE_ID,
                "--class-guid",
                DISPLAY_CLASS_GUID,
                log_opt
            ]
        );
        assert_eq!(
            remove_package_args(&log),
            [
                "--remove-driver-store-package",
                "--inf-name",
                "mttvdd.inf",
                "--class-guid",
                DISPLAY_CLASS_GUID,
                log_opt
            ]
        );
    }
}
