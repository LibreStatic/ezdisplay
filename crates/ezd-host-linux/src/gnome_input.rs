//! GNOME input mapping: the virtual touchscreen and graphics tablet are tied to the virtual
//! monitor through gsettings, the way GNOME's own control center does it.
//!
//! Mutter reads `output` from the relocatable schemas
//! `org.gnome.desktop.peripherals.touchscreen` at `/org/gnome/desktop/peripherals/touchscreens/<vendor>:<product>/`
//! and `org.gnome.desktop.peripherals.tablet` at `/org/gnome/desktop/peripherals/tablets/<vendor>:<product>/`
//! (lowercase hex ids). The value is the monitor's EDID triple `['vendor', 'product', 'serial']`;
//! `['', '', '']` means automatic mapping. The previous value is restored when the session ends.

use crate::gnome_dc::{self, DisplayState};
use crate::kscreen::KOutput;
use crate::uinput_sink::{DesktopInput, MappedDevices, PEN_PRODUCT, TOUCH_PRODUCT, VENDOR};
use std::process::Command;
use std::sync::Mutex;
use zbus::blocking::Connection;

const TOUCH_SCHEMA: &str = "org.gnome.desktop.peripherals.touchscreen";
const TABLET_SCHEMA: &str = "org.gnome.desktop.peripherals.tablet";
const KEY: &str = "output";
/// The key's default: no mapping.
const AUTOMATIC: &str = "['', '', '']";

/// `<schema>:<path>` as `gsettings` takes a relocatable schema.
pub fn schema_with_path(schema: &str, dir: &str, vendor: u16, product: u16) -> String {
    format!("{schema}:/org/gnome/desktop/peripherals/{dir}/{vendor:04x}:{product:04x}/")
}

/// The touchscreen schema of the virtual touch device.
pub fn touch_schema() -> String {
    schema_with_path(TOUCH_SCHEMA, "touchscreens", VENDOR, TOUCH_PRODUCT)
}

/// The tablet schema of the virtual pen device.
pub fn tablet_schema() -> String {
    schema_with_path(TABLET_SCHEMA, "tablets", VENDOR, PEN_PRODUCT)
}

/// A GVariant text string literal (`'...'`) for `s`.
fn quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// The `output` value for a monitor's EDID triple: `['vendor', 'product', 'serial']`.
pub fn output_value(edid: &[String; 3]) -> String {
    format!(
        "[{}, {}, {}]",
        quote(&edid[0]),
        quote(&edid[1]),
        quote(&edid[2])
    )
}

/// Arguments for `gsettings` that read the key.
pub fn get_args(schema: &str) -> Vec<String> {
    vec!["get".into(), schema.into(), KEY.into()]
}

/// Arguments for `gsettings` that set the key.
pub fn set_args(schema: &str, value: &str) -> Vec<String> {
    vec!["set".into(), schema.into(), KEY.into(), value.into()]
}

/// Arguments for `gsettings` that reset the key to its default.
pub fn reset_args(schema: &str) -> Vec<String> {
    vec!["reset".into(), schema.into(), KEY.into()]
}

/// Arguments that put back `previous` (as `gsettings get` printed it): a reset when it was the
/// default, so no explicit value is left behind in dconf.
pub fn restore_args(schema: &str, previous: &str) -> Vec<String> {
    if previous.trim() == AUTOMATIC {
        reset_args(schema)
    } else {
        set_args(schema, previous.trim())
    }
}

fn gsettings(args: &[String]) -> Result<String, String> {
    let out = Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run gsettings: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "gsettings {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Restores the keys it changed when dropped.
struct Restore {
    /// `(schema:path, previous value)`.
    keys: Vec<(String, String)>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        for (schema, previous) in &self.keys {
            if let Err(e) = gsettings(&restore_args(schema, previous)) {
                tracing::warn!("cannot restore the output mapping: {e}");
            }
        }
    }
}

/// Input mapping for GNOME.
#[derive(Default)]
pub struct GnomeDesktop {
    conn: Mutex<Option<Connection>>,
}

impl GnomeDesktop {
    /// A mapper that connects to the session bus on first use.
    pub fn new() -> GnomeDesktop {
        GnomeDesktop::default()
    }

    fn state(&self) -> Result<DisplayState, String> {
        let mut slot = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(Connection::session().map_err(|e| format!("session bus: {e}"))?);
        }
        let conn = slot.as_ref().ok_or("no session bus")?;
        let r = gnome_dc::get_state(conn);
        if r.is_err() {
            // A dead connection would fail every later call too.
            *slot = None;
        }
        r
    }
}

impl DesktopInput for GnomeDesktop {
    fn map_devices(&self, devices: &[(&str, &str)], output: &str) -> MappedDevices {
        let edid = self.state().and_then(|s| {
            s.monitor(output)
                .map(|m| m.edid())
                .ok_or_else(|| format!("Mutter does not list the monitor {output}"))
        });
        let edid = match edid {
            Ok(e) => e,
            Err(e) => {
                return MappedDevices {
                    results: devices.iter().map(|_| Err(e.clone())).collect(),
                    guard: None,
                };
            }
        };
        let value = output_value(&edid);
        let mut restore = Restore { keys: Vec::new() };
        let results = devices
            .iter()
            .map(|(label, _)| {
                let schema = match *label {
                    "touch" => touch_schema(),
                    _ => tablet_schema(),
                };
                let previous = gsettings(&get_args(&schema))?;
                gsettings(&set_args(&schema, &value))?;
                restore.keys.push((schema, previous));
                tracing::info!(device = *label, %output, %value, "GNOME output mapping set");
                Ok(())
            })
            .collect();
        MappedDevices {
            results,
            guard: (!restore.keys.is_empty()).then(|| Box::new(restore) as Box<dyn Send>),
        }
    }

    fn outputs(&self) -> Option<Vec<KOutput>> {
        self.state().ok().map(|s| s.outputs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_paths_use_lowercase_hex_ids() {
        assert_eq!(
            touch_schema(),
            "org.gnome.desktop.peripherals.touchscreen:/org/gnome/desktop/peripherals/touchscreens/1209:edd1/"
        );
        assert_eq!(
            tablet_schema(),
            "org.gnome.desktop.peripherals.tablet:/org/gnome/desktop/peripherals/tablets/1209:edd2/"
        );
        assert_eq!(
            schema_with_path("s", "d", 0x0a1b, 0x00ff),
            "s:/org/gnome/desktop/peripherals/d/0a1b:00ff/"
        );
    }

    #[test]
    fn output_value_is_an_edid_triple_with_escaping() {
        let e = ["MetaVendor", "MetaVirtualMonitor", "0x01"].map(String::from);
        assert_eq!(
            output_value(&e),
            "['MetaVendor', 'MetaVirtualMonitor', '0x01']"
        );
        let e = ["it's", "a\\b", ""].map(String::from);
        assert_eq!(output_value(&e), r"['it\'s', 'a\\b', '']");
    }

    #[test]
    fn gsettings_arguments() {
        let s = touch_schema();
        assert_eq!(get_args(&s), vec!["get", &s, "output"]);
        assert_eq!(
            set_args(&s, "['a', 'b', 'c']"),
            vec!["set", &s, "output", "['a', 'b', 'c']"]
        );
        assert_eq!(reset_args(&s), vec!["reset", &s, "output"]);
    }

    #[test]
    fn restoring_the_default_resets_the_key() {
        let s = tablet_schema();
        assert_eq!(restore_args(&s, "['', '', '']\n"), reset_args(&s));
        assert_eq!(
            restore_args(&s, "['X', 'Y', 'Z']"),
            set_args(&s, "['X', 'Y', 'Z']")
        );
    }
}
