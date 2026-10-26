//! Linux firewall detection, used by `diag` and by the Wi-Fi start-up hint. Needs no root: it reads
//! `/etc/ufw/ufw.conf` and asks systemd whether `firewalld` is running. Nothing here changes the
//! firewall; opening the port is the user's decision (README, Wi-Fi section).

use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

const UFW_CONF: &str = "/etc/ufw/ufw.conf";
const FIREWALLD_UNITS: [&str; 3] = [
    "/usr/lib/systemd/system/firewalld.service",
    "/lib/systemd/system/firewalld.service",
    "/etc/systemd/system/firewalld.service",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ufw {
    Enabled,
    Disabled,
    Absent,
    /// The config exists but cannot be read.
    Unknown,
}

impl Ufw {
    fn as_str(self) -> &'static str {
        match self {
            Ufw::Enabled => "enabled",
            Ufw::Disabled => "disabled",
            Ufw::Absent => "absent",
            Ufw::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Firewalld {
    Active,
    Inactive,
    Absent,
}

impl Firewalld {
    fn as_str(self) -> &'static str {
        match self {
            Firewalld::Active => "active",
            Firewalld::Inactive => "inactive",
            Firewalld::Absent => "absent",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct State {
    pub ufw: Ufw,
    pub firewalld: Firewalld,
}

impl State {
    /// Whether a firewall that may block inbound UDP is running.
    pub fn any_active(self) -> bool {
        self.ufw == Ufw::Enabled || self.firewalld == Firewalld::Active
    }
}

/// Whether `ufw.conf` text has `ENABLED=yes` (case-insensitive, optional quotes, `#` comments).
pub fn parse_ufw_enabled(conf: &str) -> bool {
    conf.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .rfind(|(k, _)| k.trim().eq_ignore_ascii_case("ENABLED"))
        .is_some_and(|(_, v)| {
            v.split('#')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(['"', '\''])
                .eq_ignore_ascii_case("yes")
        })
}

fn ufw_state() -> Ufw {
    match std::fs::read_to_string(UFW_CONF) {
        Ok(s) if parse_ufw_enabled(&s) => Ufw::Enabled,
        Ok(_) => Ufw::Disabled,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ufw::Absent,
        Err(_) => Ufw::Unknown,
    }
}

fn firewalld_state() -> Firewalld {
    let active = Command::new("systemctl")
        .args(["is-active", "--quiet", "firewalld"])
        .status()
        .is_ok_and(|s| s.success());
    if active {
        Firewalld::Active
    } else if FIREWALLD_UNITS.iter().any(|p| Path::new(p).exists()) {
        Firewalld::Inactive
    } else {
        Firewalld::Absent
    }
}

pub fn detect() -> State {
    State {
        ufw: ufw_state(),
        firewalld: firewalld_state(),
    }
}

/// The `firewall` object of the diag report.
pub fn diag_value(state: State) -> Value {
    let hint = state.any_active().then_some(
        "A firewall is active; if tablets cannot connect over Wi-Fi, allow inbound UDP port 27183 \
         (ufw application profile or firewalld service `ezdisplay`; see the README).",
    );
    json!({
        "ufw": state.ufw.as_str(),
        "firewalld": state.firewalld.as_str(),
        "hint": hint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ufw_conf_enabled() {
        assert!(parse_ufw_enabled("# comment\nENABLED=yes\nLOGLEVEL=low\n"));
        assert!(parse_ufw_enabled("  enabled = \"YES\"  # on\n"));
        assert!(parse_ufw_enabled("ENABLED=yes\r\n"));
    }

    #[test]
    fn ufw_conf_disabled() {
        assert!(!parse_ufw_enabled("ENABLED=no\n"));
        assert!(!parse_ufw_enabled("#ENABLED=yes\n"));
        assert!(!parse_ufw_enabled("OTHER_ENABLED=yes\n"));
        assert!(!parse_ufw_enabled(""));
        assert!(!parse_ufw_enabled("ENABLED=yesno\n"));
    }

    #[test]
    fn ufw_conf_last_assignment_wins() {
        assert!(!parse_ufw_enabled("ENABLED=yes\nENABLED=no\n"));
        assert!(parse_ufw_enabled("ENABLED=no\nENABLED=yes\n"));
    }

    #[test]
    fn diag_hint_only_when_active() {
        let off = State {
            ufw: Ufw::Disabled,
            firewalld: Firewalld::Inactive,
        };
        assert!(diag_value(off)["hint"].is_null());
        let on = State {
            ufw: Ufw::Absent,
            firewalld: Firewalld::Active,
        };
        assert!(diag_value(on)["hint"].is_string());
        assert_eq!(diag_value(on)["firewalld"], "active");
    }
}
