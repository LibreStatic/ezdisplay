//! Linux input permission: whether the user may create `/dev/uinput` devices, and installing the
//! udev rule that grants it (the distro packages ship the rule; an AppImage cannot, so the
//! settings window offers to install it through polkit).

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::process::Command;
use std::time::{Duration, Instant};

/// The udev rule, the same file the packages install.
const RULE: &str = include_str!("../../../packaging/linux/70-ezdisplay-uinput.rules");

/// How long to wait for udev to apply the access control list after the rule is triggered.
const SETTLE: Duration = Duration::from_secs(2);

/// Runs as root with the rule as `$1` (never interpolated into the script).
const SCRIPT: &str = "set -e
modprobe uinput 2>/dev/null || true
install -d /etc/udev/rules.d
printf '%s\\n' \"$1\" > /etc/udev/rules.d/70-ezdisplay-uinput.rules
chmod 644 /etc/udev/rules.d/70-ezdisplay-uinput.rules
udevadm control --reload
udevadm trigger --name-match=uinput
udevadm settle --timeout=5 || true";

/// Whether this user can open `/dev/uinput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Ok,
    /// The node exists but this user may not open it.
    Denied,
    /// No `/dev/uinput`: the kernel module is not loaded.
    Missing,
}

/// Why [`install`] did not leave input usable.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The user dismissed the authentication dialog (pkexec exit 126).
    Cancelled,
    /// No authentication agent, not authorized, or no pkexec (exit 127, or not found).
    NoAgent,
    /// The script failed with this exit code.
    Failed(i32),
    /// The rule is installed but `/dev/uinput` is still not accessible (for example the session
    /// predates the rule).
    StillDenied,
    /// pkexec could not run, or was killed.
    Io(String),
}

pub fn access() -> Access {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
    {
        Ok(_) => Access::Ok,
        Err(e) if e.kind() == ErrorKind::NotFound => Access::Missing,
        Err(e) => {
            if e.kind() != ErrorKind::PermissionDenied {
                tracing::warn!("cannot probe /dev/uinput: {e}");
            }
            Access::Denied
        }
    }
}

/// Program and arguments of the privileged install: `pkexec /bin/sh -c <script> sh <rule>`.
fn pkexec_command() -> (&'static str, Vec<String>) {
    (
        "pkexec",
        vec![
            "/bin/sh".into(),
            "-c".into(),
            SCRIPT.into(),
            "sh".into(),
            RULE.into(),
        ],
    )
}

/// Maps a pkexec exit code to the outcome (`0` is success).
fn classify_exit(code: i32) -> Result<(), InstallError> {
    match code {
        0 => Ok(()),
        126 => Err(InstallError::Cancelled),
        127 => Err(InstallError::NoAgent),
        c => Err(InstallError::Failed(c)),
    }
}

/// Installs the udev rule and loads the module, asking for authorization through polkit. Blocks
/// until the dialog is answered: call it from a background thread.
pub fn install() -> Result<(), InstallError> {
    let (program, args) = pkexec_command();
    let status = match Command::new(program).args(&args).status() {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(InstallError::NoAgent),
        Err(e) => return Err(InstallError::Io(e.to_string())),
    };
    match status.code() {
        Some(code) => classify_exit(code)?,
        None => return Err(InstallError::Io("pkexec was terminated by a signal".into())),
    }
    // The access control list is applied asynchronously after the trigger.
    let deadline = Instant::now() + SETTLE;
    loop {
        if access() == Access::Ok {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(InstallError::StillDenied);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkexec_gets_the_rule_unmodified_as_the_last_argument() {
        let (program, args) = pkexec_command();
        assert_eq!(program, "pkexec");
        assert_eq!(&args[..3], ["/bin/sh", "-c", SCRIPT]);
        assert_eq!(args[3], "sh");
        assert_eq!(args.last().map(String::as_str), Some(RULE));
        assert!(RULE.contains("uinput"));
    }

    #[test]
    fn script_writes_the_target_file_from_the_first_argument() {
        assert!(SCRIPT.contains("> /etc/udev/rules.d/70-ezdisplay-uinput.rules"));
        assert!(SCRIPT.contains("\"$1\""));
        assert!(!SCRIPT.contains(RULE.trim()));
    }

    #[test]
    fn exit_codes_map_to_outcomes() {
        assert_eq!(classify_exit(0), Ok(()));
        assert_eq!(classify_exit(126), Err(InstallError::Cancelled));
        assert_eq!(classify_exit(127), Err(InstallError::NoAgent));
        assert_eq!(classify_exit(1), Err(InstallError::Failed(1)));
    }
}
