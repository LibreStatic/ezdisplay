//! Remembers where the user put Mutter's virtual monitor.
//!
//! Mutter stores monitor layouts keyed by the physical monitors only, and every virtual monitor
//! gets a new serial, so a virtual monitor always comes back with the default scale to the right
//! of the others. The host therefore saves the layout when a session ends and applies it again
//! (`DisplayConfig.ApplyMonitorsConfig`) when the virtual monitor of the next session shows up
//! next to the same physical monitors with the same mode.

use crate::gnome_dc::{self, DisplayState};
use crate::portal::token_path_from;
use serde_json::{Value as Json, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

const FILE: &str = "gnome-layouts.json";
/// `ApplyMonitorsConfig` method: apply without writing `monitors.xml`.
const METHOD_TEMPORARY: u32 = 1;

/// A monitor of a saved logical monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedMonitor {
    /// The connector, `None` for the virtual monitor (its connector changes between sessions).
    pub connector: Option<String>,
    /// The mode id it had. Unused for the virtual monitor, whose mode comes from the session.
    pub mode_id: String,
}

/// A logical monitor of a saved layout.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedLogical {
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: u32,
    pub primary: bool,
    pub monitors: Vec<SavedMonitor>,
}

/// The whole desktop layout while a virtual monitor was up.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedLayout {
    pub logical: Vec<SavedLogical>,
}

/// `(connector, mode id, properties)` of `ApplyMonitorsConfig`.
type ApplyMonitor = (String, String, HashMap<String, Value<'static>>);
/// A logical monitor of `ApplyMonitorsConfig`: `(iiduba(ssa{sv}))`.
pub type ApplyLogical = (i32, i32, f64, u32, bool, Vec<ApplyMonitor>);

/// Identifies the situation a layout belongs to: the mode of the virtual monitor, the layout
/// mode and the physical monitors that are connected. `None` when `virtual_connector` is not a
/// virtual monitor (a physical one plugged in at the same moment), not the only one, or is off.
pub fn key(state: &DisplayState, virtual_connector: &str) -> Option<String> {
    let monitor = state.monitor(virtual_connector)?;
    if !monitor.is_virtual() {
        return None;
    }
    let mode = monitor.current_mode()?;
    let mut physical = Vec::new();
    for m in &state.monitors {
        if m.connector == virtual_connector {
            continue;
        }
        if m.is_virtual() {
            return None;
        }
        physical.push(format!(
            "{}/{}/{}/{}",
            m.connector, m.vendor, m.product, m.serial
        ));
    }
    physical.sort();
    Some(format!(
        "{}x{}|{}|{}",
        mode.width,
        mode.height,
        if state.physical_layout {
            "physical"
        } else {
            "logical"
        },
        physical.join("|")
    ))
}

/// The current layout, with `virtual_connector` made anonymous.
pub fn capture(state: &DisplayState, virtual_connector: &str) -> Option<SavedLayout> {
    state.logical_of(virtual_connector)?;
    let mut logical = Vec::new();
    for l in &state.logical {
        let mut monitors = Vec::new();
        for connector in &l.monitors {
            let mode_id = state.monitor(connector)?.current_mode()?.id.clone();
            monitors.push(SavedMonitor {
                connector: (connector != virtual_connector).then(|| connector.clone()),
                mode_id,
            });
        }
        logical.push(SavedLogical {
            x: l.x,
            y: l.y,
            scale: l.scale,
            transform: l.transform,
            primary: l.primary,
            monitors,
        });
    }
    Some(SavedLayout { logical })
}

/// The `ApplyMonitorsConfig` argument that restores `saved` in `state`. `None` when there is
/// nothing to do or the layout no longer fits: a monitor is gone, or the user has since turned a
/// physical monitor on or off or changed its mode, scale or rotation (their newer choice wins).
pub fn plan(
    saved: &SavedLayout,
    state: &DisplayState,
    virtual_connector: &str,
) -> Option<Vec<ApplyLogical>> {
    let virtual_mode = state.monitor(virtual_connector)?.current_mode()?.id.clone();
    // The same physical monitors are on now as when the layout was saved.
    let enabled = |connectors: &mut dyn Iterator<Item = &String>| {
        let mut v: Vec<String> = connectors.cloned().collect();
        v.sort();
        v
    };
    let saved_on = enabled(
        &mut saved
            .logical
            .iter()
            .flat_map(|l| &l.monitors)
            .filter_map(|m| m.connector.as_ref()),
    );
    let now_on = enabled(
        &mut state
            .logical
            .iter()
            .flat_map(|l| &l.monitors)
            .filter(|c| *c != virtual_connector),
    );
    if saved_on != now_on {
        return None;
    }
    let mut out = Vec::new();
    for l in &saved.logical {
        let mut monitors = Vec::new();
        for m in &l.monitors {
            let (connector, mode_id) = match &m.connector {
                None => (virtual_connector.to_string(), virtual_mode.clone()),
                Some(c) => {
                    let now = state.logical_of(c)?;
                    if state.monitor(c)?.current_mode()?.id != m.mode_id
                        || now.scale != l.scale
                        || now.transform != l.transform
                    {
                        return None;
                    }
                    (c.clone(), m.mode_id.clone())
                }
            };
            monitors.push((connector, mode_id, HashMap::new()));
        }
        out.push((l.x, l.y, l.scale, l.transform, l.primary, monitors));
    }
    (capture(state, virtual_connector).as_ref() != Some(saved)).then_some(out)
}

fn to_json(layout: &SavedLayout) -> Json {
    let logical: Vec<Json> = layout
        .logical
        .iter()
        .map(|l| {
            let monitors: Vec<Json> = l
                .monitors
                .iter()
                .map(|m| json!({ "connector": m.connector, "mode": m.mode_id }))
                .collect();
            json!({
                "x": l.x, "y": l.y, "scale": l.scale, "transform": l.transform,
                "primary": l.primary, "monitors": monitors,
            })
        })
        .collect();
    json!({ "logical": logical })
}

fn from_json(v: &Json) -> Option<SavedLayout> {
    let mut logical = Vec::new();
    for l in v.get("logical")?.as_array()? {
        let mut monitors = Vec::new();
        for m in l.get("monitors")?.as_array()? {
            monitors.push(SavedMonitor {
                connector: m.get("connector")?.as_str().map(String::from),
                mode_id: m.get("mode")?.as_str()?.to_string(),
            });
        }
        logical.push(SavedLogical {
            x: i32::try_from(l.get("x")?.as_i64()?).ok()?,
            y: i32::try_from(l.get("y")?.as_i64()?).ok()?,
            scale: l.get("scale")?.as_f64()?,
            transform: u32::try_from(l.get("transform")?.as_u64()?).ok()?,
            primary: l.get("primary")?.as_bool()?,
            monitors,
        });
    }
    Some(SavedLayout { logical })
}

/// `$XDG_STATE_HOME/ezdisplay/gnome-layouts.json`.
fn default_path() -> Option<PathBuf> {
    token_path_from(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        FILE,
    )
}

fn read_all(path: &Path) -> serde_json::Map<String, Json> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Json>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The layout saved in `path` for `key`.
pub fn load(path: &Path, key: &str) -> Option<SavedLayout> {
    from_json(read_all(path).get(key)?)
}

/// Saves `layout` for `key` in `path`, keeping the layouts of other keys.
pub fn store(path: &Path, key: &str, layout: &SavedLayout) -> std::io::Result<()> {
    let mut all = read_all(path);
    all.insert(key.to_string(), to_json(layout));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written aside and renamed: a failed write must not truncate the layouts already saved.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{:#}\n", Json::Object(all)))?;
    std::fs::rename(&tmp, path)
}

/// Saves the current layout around `virtual_connector`. Called while the monitor still exists.
pub fn remember(conn: &Connection, virtual_connector: &str) {
    let Some(path) = default_path() else { return };
    let Ok(state) = gnome_dc::get_state(conn) else {
        return;
    };
    let (Some(key), Some(layout)) = (
        key(&state, virtual_connector),
        capture(&state, virtual_connector),
    ) else {
        return;
    };
    match store(&path, &key, &layout) {
        Ok(()) => tracing::debug!("saved the monitor layout to {path:?}"),
        Err(e) => tracing::warn!("cannot save the monitor layout to {path:?}: {e}"),
    }
}

/// `DisplayConfig.ApplyMonitorsConfig` for the state with `serial`, without saving it to disk.
pub fn apply(conn: &Connection, serial: u32, logical: Vec<ApplyLogical>) -> zbus::Result<()> {
    Proxy::new(conn, gnome_dc::SERVICE, gnome_dc::PATH, gnome_dc::SERVICE)?.call(
        "ApplyMonitorsConfig",
        &(
            serial,
            METHOD_TEMPORARY,
            logical,
            HashMap::<&str, Value>::new(),
        ),
    )
}

/// Applies the layout saved for the situation in `state`, if any. True when it was applied.
pub fn restore(conn: &Connection, state: &DisplayState, virtual_connector: &str) -> bool {
    let Some(saved) = default_path()
        .zip(key(state, virtual_connector))
        .and_then(|(path, key)| load(&path, &key))
    else {
        return false;
    };
    let Some(logical) = plan(&saved, state, virtual_connector) else {
        return false;
    };
    let res = apply(conn, state.serial, logical);
    match res {
        Ok(()) => {
            tracing::info!("restored the saved layout of {virtual_connector}");
            true
        }
        Err(e) => {
            tracing::warn!("cannot restore the saved layout of {virtual_connector}: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gnome_dc::RawState;

    const PAIR: &str = include_str!("../testdata/gnome-displayconfig-headless-plus-virtual.json");

    fn pair() -> DisplayState {
        let raw: RawState<serde_json::Map<String, Json>> = serde_json::from_str(PAIR).unwrap();
        DisplayState::from_raw(raw)
    }

    /// `pair()` after the user moved the virtual monitor to the left at scale 2.
    fn arranged() -> DisplayState {
        let mut s = pair();
        let w = s.monitor("Meta-1").unwrap().current_mode().unwrap().width as i32;
        for l in &mut s.logical {
            if l.monitors == ["Meta-1"] {
                (l.x, l.y, l.scale) = (0, 0, 2.0);
            } else {
                l.x = w;
            }
        }
        s
    }

    #[test]
    fn the_key_ignores_the_connector_and_serial_of_the_virtual_monitor() {
        let a = pair();
        let mut b = a.clone();
        b.monitors[1].connector = "Meta-7".into();
        b.monitors[1].serial = "0x000009".into();
        b.logical[1].monitors = vec!["Meta-7".into()];
        assert_eq!(key(&a, "Meta-1"), key(&b, "Meta-7"));
        assert!(key(&a, "Meta-1").unwrap().starts_with("1280x720|physical|"));

        let mut other = a.clone();
        other.monitors[0].serial = "another".into();
        assert_ne!(key(&a, "Meta-1"), key(&other, "Meta-1"));
        // Two virtual monitors: which layout belongs to which is unknown.
        let mut two = a.clone();
        two.monitors[0].product = gnome_dc::VIRTUAL_PRODUCT.into();
        assert_eq!(key(&two, "Meta-1"), None);
        assert_eq!(key(&a, "Meta-9"), None);
        // Not a virtual monitor.
        assert_eq!(key(&a, "Meta-0"), None);
    }

    #[test]
    fn a_saved_layout_is_planned_onto_the_new_virtual_connector() {
        let saved = capture(&arranged(), "Meta-1").unwrap();
        let mut now = pair();
        now.monitors[1].connector = "Meta-2".into();
        now.logical[1].monitors = vec!["Meta-2".into()];

        let plan = plan(&saved, &now, "Meta-2").expect("a plan");
        let virt = plan.iter().find(|l| l.5[0].0 == "Meta-2").unwrap();
        assert_eq!((virt.0, virt.1, virt.2), (0, 0, 2.0));
        assert_eq!(virt.5[0].1, "1280x720@60.000");
        let phys = plan.iter().find(|l| l.5[0].0 != "Meta-2").unwrap();
        assert_eq!(phys.0, 1280);
    }

    #[test]
    fn nothing_is_planned_when_the_layout_is_current_or_no_longer_fits() {
        let arranged = arranged();
        let saved = capture(&arranged, "Meta-1").unwrap();
        assert_eq!(plan(&saved, &arranged, "Meta-1"), None);

        // The user changed the scale of the physical monitor since.
        let mut rescaled = pair();
        rescaled.logical[0].scale = 1.5;
        assert_eq!(plan(&saved, &rescaled, "Meta-1"), None);

        // The user turned the physical monitor off since: it is not switched back on.
        let mut off = pair();
        off.logical.remove(0);
        assert_eq!(plan(&saved, &off, "Meta-1"), None);
        // Or it was off when the layout was saved and is on now: it is not switched off.
        let saved_off = capture(&off, "Meta-1").unwrap();
        assert_eq!(plan(&saved_off, &pair(), "Meta-1"), None);

        // The user changed the mode of the physical monitor since.
        let mut other_mode = pair();
        other_mode.monitors[0].modes[0].id = "1280x720@60.000".into();
        assert_eq!(plan(&saved, &other_mode, "Meta-1"), None);
    }

    #[test]
    fn layouts_round_trip_through_the_file_by_key() {
        let dir = std::env::temp_dir().join(format!("ezd-gnome-layout-{}", std::process::id()));
        let path = dir.join("sub").join(FILE);
        let a = capture(&arranged(), "Meta-1").unwrap();
        let b = capture(&pair(), "Meta-1").unwrap();
        assert_eq!(load(&path, "a"), None);
        store(&path, "a", &a).unwrap();
        store(&path, "b", &b).unwrap();
        assert_eq!(load(&path, "a"), Some(a));
        assert_eq!(load(&path, "b"), Some(b));
        assert_eq!(load(&path, "c"), None);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
