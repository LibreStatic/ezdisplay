//! Which Linux desktop the host runs on: picks the virtual output provider and the input backend.
//!
//! Detection is a pure function over the environment and two bus probes, so it can be tested
//! without a session: `EZD_LINUX_BACKEND`, then `XDG_CURRENT_DESKTOP`, then the compositors' bus
//! names (a host started over ssh or from a stripped environment has no desktop variable), then
//! the sway and Hyprland variables.

use std::fmt;

/// A desktop the Linux host can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxBackend {
    /// KDE Plasma 6 (KWin): portal VIRTUAL source, `kscreen-doctor`, KWin input mapping.
    Kde,
    /// GNOME (Mutter): `org.gnome.Mutter.ScreenCast` virtual monitor, `DisplayConfig`, gsettings.
    Gnome,
    /// Sway: i3 IPC headless output, direct Wayland capture, per-device input mapping.
    Sway,
    /// Hyprland: headless output over its command socket, direct Wayland capture, per-device
    /// input mapping.
    Hyprland,
}

impl LinuxBackend {
    /// Stable name: the `EZD_LINUX_BACKEND` value and the `backend` field of `ezd-host diag`.
    pub fn name(self) -> &'static str {
        match self {
            LinuxBackend::Kde => "kde",
            LinuxBackend::Gnome => "gnome",
            LinuxBackend::Sway => "sway",
            LinuxBackend::Hyprland => "hyprland",
        }
    }

    /// Parses an `EZD_LINUX_BACKEND` value (case-insensitive).
    pub fn parse(s: &str) -> Option<LinuxBackend> {
        match s.trim().to_ascii_lowercase().as_str() {
            "kde" => Some(LinuxBackend::Kde),
            "gnome" => Some(LinuxBackend::Gnome),
            "sway" => Some(LinuxBackend::Sway),
            "hyprland" => Some(LinuxBackend::Hyprland),
            _ => None,
        }
    }
}

impl fmt::Display for LinuxBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The environment detection looks at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectEnv {
    /// `EZD_LINUX_BACKEND`.
    pub override_backend: Option<String>,
    /// `XDG_CURRENT_DESKTOP`.
    pub desktop: Option<String>,
    /// `SWAYSOCK` is set.
    pub sway_sock: bool,
    /// `HYPRLAND_INSTANCE_SIGNATURE` is set.
    pub hyprland_signature: bool,
}

impl DetectEnv {
    /// Reads the process environment.
    pub fn from_process() -> DetectEnv {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        DetectEnv {
            override_backend: var("EZD_LINUX_BACKEND"),
            desktop: var("XDG_CURRENT_DESKTOP"),
            sway_sock: var("SWAYSOCK").is_some(),
            hyprland_signature: var("HYPRLAND_INSTANCE_SIGNATURE").is_some(),
        }
    }
}

/// Which compositor services are on the session bus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BusProbe {
    /// `org.kde.KWin`.
    pub kwin: bool,
    /// `org.gnome.Mutter.ScreenCast`.
    pub mutter: bool,
}

/// Backend named by the `XDG_CURRENT_DESKTOP` list (`ubuntu:GNOME`, `KDE`, ...).
pub fn from_desktop(desktop: &str) -> Option<LinuxBackend> {
    desktop
        .split(':')
        .find_map(|d| match d.trim().to_ascii_lowercase().as_str() {
            "kde" => Some(LinuxBackend::Kde),
            "gnome" | "gnome-classic" | "gnome-flashback" => Some(LinuxBackend::Gnome),
            "sway" => Some(LinuxBackend::Sway),
            "hyprland" => Some(LinuxBackend::Hyprland),
            _ => None,
        })
}

/// Detects the backend. `probe` is only called when the environment does not decide. `Err` is an
/// invalid `EZD_LINUX_BACKEND`; `Ok(None)` means nothing was recognized.
pub fn detect(
    env: &DetectEnv,
    probe: impl FnOnce() -> BusProbe,
) -> Result<Option<LinuxBackend>, String> {
    if let Some(v) = &env.override_backend {
        return LinuxBackend::parse(v).map(Some).ok_or_else(|| {
            format!("EZD_LINUX_BACKEND={v:?} is not one of kde, gnome, sway, hyprland")
        });
    }
    if let Some(b) = env.desktop.as_deref().and_then(from_desktop) {
        return Ok(Some(b));
    }
    let bus = probe();
    if bus.kwin {
        return Ok(Some(LinuxBackend::Kde));
    }
    if bus.mutter {
        return Ok(Some(LinuxBackend::Gnome));
    }
    if env.sway_sock {
        return Ok(Some(LinuxBackend::Sway));
    }
    if env.hyprland_signature {
        return Ok(Some(LinuxBackend::Hyprland));
    }
    Ok(None)
}

/// True if `name` has an owner on the session bus. Own thread: callers may be inside a runtime.
fn bus_name_owned(names: &'static [&'static str]) -> Vec<bool> {
    std::thread::spawn(move || {
        let Ok(conn) = zbus::blocking::Connection::session() else {
            return vec![false; names.len()];
        };
        let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else {
            return vec![false; names.len()];
        };
        names
            .iter()
            .map(|n| {
                zbus::names::BusName::try_from(*n)
                    .ok()
                    .and_then(|n| dbus.name_has_owner(n).ok())
                    .unwrap_or(false)
            })
            .collect()
    })
    .join()
    .unwrap_or_else(|_| vec![false; names.len()])
}

/// Probes the session bus for the compositor services.
pub fn probe_bus() -> BusProbe {
    let r = bus_name_owned(&["org.kde.KWin", "org.gnome.Mutter.ScreenCast"]);
    BusProbe {
        kwin: r[0],
        mutter: r[1],
    }
}

/// Detects the backend of this process. Nothing recognized falls back to KDE (the behaviour
/// before other desktops existed); an invalid override is an error.
pub fn current() -> Result<LinuxBackend, String> {
    match detect(&DetectEnv::from_process(), probe_bus)? {
        Some(b) => Ok(b),
        None => {
            tracing::warn!(
                "no supported desktop recognized; assuming KDE (set EZD_LINUX_BACKEND=kde|gnome to choose)"
            );
            Ok(LinuxBackend::Kde)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(desktop: Option<&str>) -> DetectEnv {
        DetectEnv {
            desktop: desktop.map(str::to_string),
            ..Default::default()
        }
    }

    const NO_BUS: BusProbe = BusProbe {
        kwin: false,
        mutter: false,
    };

    #[test]
    fn override_wins_over_everything() {
        let mut e = env(Some("KDE"));
        e.override_backend = Some("GNOME".into());
        let bus = BusProbe {
            kwin: true,
            mutter: false,
        };
        assert_eq!(detect(&e, || bus), Ok(Some(LinuxBackend::Gnome)));
        e.override_backend = Some("hyprland".into());
        assert_eq!(detect(&e, || bus), Ok(Some(LinuxBackend::Hyprland)));
    }

    #[test]
    fn invalid_override_is_an_error() {
        let mut e = env(Some("KDE"));
        e.override_backend = Some("cinnamon".into());
        let err = detect(&e, || NO_BUS).unwrap_err();
        assert!(err.contains("cinnamon") && err.contains("gnome"));
    }

    #[test]
    fn desktop_variable_lists() {
        for (d, b) in [
            ("KDE", LinuxBackend::Kde),
            ("ubuntu:GNOME", LinuxBackend::Gnome),
            ("GNOME-Classic:GNOME", LinuxBackend::Gnome),
            ("sway", LinuxBackend::Sway),
            ("Hyprland", LinuxBackend::Hyprland),
            ("X-Cinnamon:KDE", LinuxBackend::Kde),
        ] {
            assert_eq!(
                detect(&env(Some(d)), || panic!("no probe")),
                Ok(Some(b)),
                "{d}"
            );
        }
    }

    #[test]
    fn bus_probes_decide_when_the_desktop_is_unknown() {
        let kwin = BusProbe {
            kwin: true,
            mutter: false,
        };
        let mutter = BusProbe {
            kwin: false,
            mutter: true,
        };
        assert_eq!(detect(&env(None), || kwin), Ok(Some(LinuxBackend::Kde)));
        assert_eq!(
            detect(&env(Some("X-Unknown")), || mutter),
            Ok(Some(LinuxBackend::Gnome))
        );
        // KWin wins when (unusually) both are on the bus.
        let both = BusProbe {
            kwin: true,
            mutter: true,
        };
        assert_eq!(detect(&env(None), || both), Ok(Some(LinuxBackend::Kde)));
    }

    #[test]
    fn sway_and_hyprland_variables_come_last() {
        let mut e = env(None);
        e.sway_sock = true;
        assert_eq!(detect(&e, || NO_BUS), Ok(Some(LinuxBackend::Sway)));
        e.sway_sock = false;
        e.hyprland_signature = true;
        assert_eq!(detect(&e, || NO_BUS), Ok(Some(LinuxBackend::Hyprland)));
        // A bus name beats the variables.
        let kwin = BusProbe {
            kwin: true,
            mutter: false,
        };
        assert_eq!(detect(&e, || kwin), Ok(Some(LinuxBackend::Kde)));
        assert_eq!(detect(&env(None), || NO_BUS), Ok(None));
    }

    #[test]
    fn names_round_trip() {
        for b in [
            LinuxBackend::Kde,
            LinuxBackend::Gnome,
            LinuxBackend::Sway,
            LinuxBackend::Hyprland,
        ] {
            assert_eq!(LinuxBackend::parse(b.name()), Some(b));
        }
    }
}
