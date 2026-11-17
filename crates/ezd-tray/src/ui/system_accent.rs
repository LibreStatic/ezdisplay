//! The accent colour of the operating system (Windows and Linux).

use eframe::egui::Color32;

/// Reads the system accent colour. Blocking and possibly slow (a registry read, a D-Bus call):
/// call it from a background thread. `None` when the platform has none or it cannot be read.
pub fn read() -> Option<Color32> {
    #[cfg(windows)]
    {
        ezd_host_windows::system_accent().map(|[r, g, b]| Color32::from_rgb(r, g, b))
    }
    #[cfg(target_os = "linux")]
    {
        linux::read().map(|[r, g, b]| Color32::from_rgb(r, g, b))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// The portal reports sRGB components in [0, 1]; anything outside means "no accent set".
#[cfg(any(target_os = "linux", test))]
fn portal_color(r: f64, g: f64, b: f64) -> Option<[u8; 3]> {
    let c = [r, g, b];
    if c.iter().any(|v| !(0.0..=1.0).contains(v)) {
        return None;
    }
    Some(c.map(|v| (v * 255.0).round() as u8))
}

/// `AccentColor=r,g,b` of the `[General]` group of `kdeglobals`.
#[cfg(any(target_os = "linux", test))]
fn parse_kdeglobals(text: &str) -> Option<[u8; 3]> {
    let mut in_general = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_general = line == "[General]";
        } else if in_general && let Some(v) = line.strip_prefix("AccentColor=") {
            let mut parts = v.split(',').map(|p| p.trim().parse::<u8>());
            let (r, g, b) = (parts.next()?, parts.next()?, parts.next()?);
            return Some([r.ok()?, g.ok()?, b.ok()?]);
        }
    }
    None
}

/// The accent of an Ubuntu GTK theme name. GNOME before 47 has no accent setting, and Ubuntu
/// encodes the accent chosen in its settings in the theme: `Yaru`, `Yaru-dark`, `Yaru-purple`,
/// `Yaru-purple-dark`. The colours are the theme's own (`common/accent-colors.scss.in`).
#[cfg(any(target_os = "linux", test))]
fn yaru_accent(theme: &str) -> Option<[u8; 3]> {
    let rest = theme.strip_prefix("Yaru")?;
    let rest = rest.strip_suffix("-dark").unwrap_or(rest);
    Some(match rest {
        "" => [0xE9, 0x54, 0x20],
        "-bark" => [0x78, 0x78, 0x59],
        "-sage" => [0x65, 0x7B, 0x69],
        "-olive" => [0x4B, 0x85, 0x01],
        "-viridian" => [0x03, 0x87, 0x5B],
        "-prussiangreen" => [0x30, 0x82, 0x80],
        "-blue" => [0x00, 0x73, 0xE5],
        "-purple" => [0x77, 0x64, 0xD8],
        "-magenta" => [0xB3, 0x4C, 0xB3],
        "-red" => [0xDA, 0x34, 0x50],
        "-yellow" => [0xC8, 0x88, 0x00],
        "-wartybrown" => [0xB3, 0x91, 0x69],
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{parse_kdeglobals, portal_color, yaru_accent};
    use std::time::Duration;

    /// A missing portal must not hold the caller up.
    const PORTAL_TIMEOUT: Duration = Duration::from_secs(1);

    pub fn read() -> Option<[u8; 3]> {
        match portal_accent() {
            Ok(Some(c)) => return Some(c),
            Ok(None) => tracing::debug!("the desktop portal has no accent colour"),
            Err(e) => tracing::debug!("reading the accent colour from the portal failed: {e}"),
        }
        match with_timeout(portal_gtk_theme) {
            Ok(theme) => {
                if let Some(c) = yaru_accent(&theme) {
                    return Some(c);
                }
                tracing::debug!("the GTK theme {theme:?} names no accent colour");
            }
            Err(e) => tracing::debug!("reading the GTK theme from the portal failed: {e}"),
        }
        kdeglobals_accent()
    }

    fn with_timeout<T: Send + 'static>(call: fn() -> zbus::Result<T>) -> Result<T, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(call().map_err(|e| e.to_string()));
        });
        rx.recv_timeout(PORTAL_TIMEOUT)
            .map_err(|_| "timed out".to_string())?
    }

    fn read_setting(namespace: &str, key: &str) -> zbus::Result<zbus::zvariant::OwnedValue> {
        let conn = zbus::blocking::Connection::session()?;
        let reply = conn.call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            "ReadOne",
            &(namespace, key),
        )?;
        reply.body().deserialize()
    }

    fn portal_gtk_theme() -> zbus::Result<String> {
        Ok(read_setting("org.gnome.desktop.interface", "gtk-theme")?.try_into()?)
    }

    fn portal_accent() -> Result<Option<[u8; 3]>, String> {
        with_timeout(portal_call)
    }

    fn portal_call() -> zbus::Result<Option<[u8; 3]>> {
        let value = read_setting("org.freedesktop.appearance", "accent-color")?;
        let (r, g, b): (f64, f64, f64) = value.try_into()?;
        Ok(portal_color(r, g, b))
    }

    fn kdeglobals_accent() -> Option<[u8; 3]> {
        let dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
        let text = std::fs::read_to_string(dir.join("kdeglobals")).ok()?;
        parse_kdeglobals(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_colour_in_range_is_scaled() {
        assert_eq!(portal_color(0.0, 0.5, 1.0), Some([0, 128, 255]));
    }

    #[test]
    fn portal_colour_out_of_range_is_unset() {
        assert_eq!(portal_color(-1.0, -1.0, -1.0), None);
        assert_eq!(portal_color(0.2, 1.5, 0.1), None);
        assert_eq!(portal_color(f64::NAN, 0.0, 0.0), None);
    }

    #[test]
    fn ubuntu_theme_names_carry_the_accent() {
        assert_eq!(yaru_accent("Yaru"), Some([0xE9, 0x54, 0x20]));
        assert_eq!(yaru_accent("Yaru-dark"), Some([0xE9, 0x54, 0x20]));
        assert_eq!(yaru_accent("Yaru-purple"), Some([0x77, 0x64, 0xD8]));
        assert_eq!(yaru_accent("Yaru-purple-dark"), Some([0x77, 0x64, 0xD8]));
        assert_eq!(yaru_accent("Yaru-unknown"), None);
        assert_eq!(yaru_accent("Adwaita"), None);
        assert_eq!(yaru_accent("Adwaita-dark"), None);
    }

    #[test]
    fn kdeglobals_accent_is_read_from_general() {
        let text = "[Colors:View]\nAccentColor=1,2,3\n[General]\nName=x\nAccentColor=61, 174,233\n";
        assert_eq!(parse_kdeglobals(text), Some([61, 174, 233]));
    }

    #[test]
    fn kdeglobals_without_a_valid_accent_is_none() {
        assert_eq!(parse_kdeglobals("[General]\nName=x\n"), None);
        assert_eq!(parse_kdeglobals("[General]\nAccentColor=1,2\n"), None);
        assert_eq!(parse_kdeglobals("[General]\nAccentColor=1,2,300\n"), None);
        assert_eq!(parse_kdeglobals("[Other]\nAccentColor=1,2,3\n"), None);
    }
}
