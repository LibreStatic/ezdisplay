//! Mutter's `org.gnome.Mutter.DisplayConfig`: the monitor list and logical layout.
//!
//! Used to find the virtual monitor a ScreenCast session created (diff of two states), to report
//! its mode and position, to map the pointer onto it, and to read the EDID triple (vendor,
//! product, serial) that the touch and pen gsettings keys want.

use crate::kscreen::KOutput;
use serde_json::Value as Json;
use std::collections::HashMap;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

pub const SERVICE: &str = "org.gnome.Mutter.DisplayConfig";
const PATH: &str = "/org/gnome/Mutter/DisplayConfig";

/// Property bags (`a{sv}`) of the reply, readable for boolean flags from D-Bus and from JSON.
pub trait Props {
    /// The boolean property `key`, false when missing.
    fn flag(&self, key: &str) -> bool;
    /// The unsigned integer property `key`.
    fn uint(&self, key: &str) -> Option<u32>;
}

impl Props for HashMap<String, OwnedValue> {
    fn flag(&self, key: &str) -> bool {
        self.get(key)
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false)
    }

    fn uint(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(|v| u32::try_from(v).ok())
    }
}

impl Props for serde_json::Map<String, Json> {
    fn flag(&self, key: &str) -> bool {
        self.get(key).and_then(Json::as_bool).unwrap_or(false)
    }

    fn uint(&self, key: &str) -> Option<u32> {
        self.get(key)
            .and_then(Json::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    }
}

type Edid = (String, String, String, String);
type RawMode<P> = (String, i32, i32, f64, f64, Vec<f64>, P);
type RawMonitor<P> = (Edid, Vec<RawMode<P>>, P);
type RawLogical<P> = (i32, i32, f64, u32, bool, Vec<Edid>, P);
/// The reply of `GetCurrentState`: `(u a((ssss)a(siiddada{sv})a{sv}) a(iiduba(ssss)a{sv}) a{sv})`.
pub type RawState<P> = (u32, Vec<RawMonitor<P>>, Vec<RawLogical<P>>, P);

/// A display mode of a monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    pub current: bool,
}

/// A connected monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub connector: String,
    pub vendor: String,
    pub product: String,
    pub serial: String,
    pub modes: Vec<Mode>,
}

impl Monitor {
    /// The current mode, if the monitor is on.
    pub fn current_mode(&self) -> Option<&Mode> {
        self.modes.iter().find(|m| m.current)
    }

    /// The `[vendor, product, serial]` triple the peripherals' `output` keys use.
    pub fn edid(&self) -> [String; 3] {
        [
            self.vendor.clone(),
            self.product.clone(),
            self.serial.clone(),
        ]
    }
}

/// A rectangle of the desktop layout holding one monitor (or several mirrored ones).
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalMonitor {
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: u32,
    pub primary: bool,
    /// Connectors of the monitors it shows.
    pub monitors: Vec<String>,
}

/// `GetCurrentState`, reduced to what the host needs.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayState {
    pub serial: u32,
    pub monitors: Vec<Monitor>,
    pub logical: Vec<LogicalMonitor>,
    /// `layout-mode` 1: layout coordinates are physical pixels. In mode 0 they are logical
    /// (pixels divided by the scale).
    pub physical_layout: bool,
}

impl DisplayState {
    /// Converts the raw reply.
    pub fn from_raw<P: Props>(raw: RawState<P>) -> DisplayState {
        let (serial, monitors, logical, props) = raw;
        DisplayState {
            serial,
            monitors: monitors
                .into_iter()
                .map(|((connector, vendor, product, serial), modes, _)| Monitor {
                    connector,
                    vendor,
                    product,
                    serial,
                    modes: modes
                        .into_iter()
                        .map(|(id, width, height, refresh_hz, _, _, p)| Mode {
                            id,
                            width: width.max(0) as u32,
                            height: height.max(0) as u32,
                            refresh_hz,
                            current: p.flag("is-current"),
                        })
                        .collect(),
                })
                .collect(),
            logical: logical
                .into_iter()
                .map(
                    |(x, y, scale, transform, primary, monitors, _)| LogicalMonitor {
                        x,
                        y,
                        scale,
                        transform,
                        primary,
                        monitors: monitors.into_iter().map(|m| m.0).collect(),
                    },
                )
                .collect(),
            physical_layout: layout_mode_is_physical(&props),
        }
    }

    /// The monitor with `connector`.
    pub fn monitor(&self, connector: &str) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.connector == connector)
    }

    /// The logical monitor showing `connector`.
    pub fn logical_of(&self, connector: &str) -> Option<&LogicalMonitor> {
        self.logical
            .iter()
            .find(|l| l.monitors.iter().any(|m| m == connector))
    }

    /// The monitor present here but not in `before`.
    pub fn new_monitor(&self, before: &DisplayState) -> Option<&Monitor> {
        self.monitors
            .iter()
            .find(|m| before.monitor(&m.connector).is_none())
    }

    /// The monitors in the shape the pointer mapper reads (`kscreen` outputs). The scale is 1
    /// in a physical layout, where the layout size of a monitor is its pixel size.
    pub fn outputs(&self) -> Vec<KOutput> {
        let mut out = Vec::new();
        for l in &self.logical {
            for connector in &l.monitors {
                let Some(m) = self.monitor(connector) else {
                    continue;
                };
                let Some(mode) = m.current_mode() else {
                    continue;
                };
                let (w, h) = if l.transform % 2 == 1 {
                    (mode.height, mode.width)
                } else {
                    (mode.width, mode.height)
                };
                out.push(KOutput {
                    name: m.connector.clone(),
                    id: 0,
                    enabled: true,
                    connected: true,
                    size: (w, h),
                    pos: (l.x, l.y),
                    scale: if self.physical_layout || l.scale <= 0.0 {
                        1.0
                    } else {
                        l.scale
                    },
                    current_mode_id: mode.id.clone(),
                    modes: Vec::new(),
                });
            }
        }
        out
    }
}

/// `layout-mode`: 0 = logical, 1 = physical. Mutter 3.x replies without it, meaning logical.
fn layout_mode_is_physical<P: Props>(props: &P) -> bool {
    props.uint("layout-mode") == Some(1)
}

/// A connection to the session bus and a proxy for `DisplayConfig`.
fn proxy(conn: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(conn, SERVICE, PATH, SERVICE)
}

/// Reads the current state.
pub fn get_state(conn: &Connection) -> Result<DisplayState, String> {
    let p = proxy(conn).map_err(|e| format!("DisplayConfig proxy: {e}"))?;
    let raw: RawState<HashMap<String, OwnedValue>> = p
        .call("GetCurrentState", &())
        .map_err(|e| format!("DisplayConfig.GetCurrentState: {e}"))?;
    Ok(DisplayState::from_raw(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Mapper;

    /// Replies captured from `mutter --headless` (51.1): a lone virtual monitor created by
    /// `RecordVirtual`, and the same next to a `--virtual-monitor 1920x1080` one.
    const SINGLE: &str = include_str!("../testdata/gnome-displayconfig-virtual.json");
    const PAIR: &str = include_str!("../testdata/gnome-displayconfig-headless-plus-virtual.json");

    fn parse(json: &str) -> DisplayState {
        let raw: RawState<serde_json::Map<String, Json>> = serde_json::from_str(json).unwrap();
        DisplayState::from_raw(raw)
    }

    #[test]
    fn parses_a_lone_virtual_monitor() {
        let s = parse(SINGLE);
        assert!(s.physical_layout);
        assert_eq!(s.monitors.len(), 1);
        let m = &s.monitors[0];
        assert_eq!(m.connector, "Meta-0");
        assert_eq!(
            m.edid(),
            ["MetaVendor", "Virtual remote monitor", "0x000001"].map(String::from)
        );
        let mode = m.current_mode().unwrap();
        assert_eq!((mode.width, mode.height), (1280, 720));
        assert!((mode.refresh_hz - 60.0).abs() < 1e-9);
        assert_eq!(mode.id, "1280x720@60.000");
        let l = s.logical_of("Meta-0").unwrap();
        assert_eq!((l.x, l.y, l.primary), (0, 0, true));
        assert!(s.logical_of("Meta-9").is_none());
    }

    #[test]
    fn finds_the_new_monitor_by_diffing_states() {
        let before = parse(PAIR);
        let one = DisplayState {
            monitors: before.monitors[..1].to_vec(),
            logical: before.logical[..1].to_vec(),
            ..before.clone()
        };
        let new = before.new_monitor(&one).unwrap();
        assert_eq!(new.connector, "Meta-1");
        assert_eq!(new.product, "Virtual remote monitor");
        assert!(before.new_monitor(&before).is_none());
    }

    #[test]
    fn pointer_mapper_covers_the_virtual_monitor_to_the_right() {
        let s = parse(PAIR);
        let m = Mapper::from_outputs(&s.outputs(), "Meta-1");
        assert!(m.is_known() && !m.origin_offset);
        // 1920 + 1280 pixels wide, the virtual monitor on the right.
        let ws: f64 = 3200.0;
        assert_eq!(m.map(0, 0), ((1920.0 / ws * 65535.0).round() as u16, 0));
        assert_eq!(m.map(65535, 0).0, 65535);
        assert_eq!(
            m.map(65535, 65535).1,
            (720.0_f64 / 1080.0 * 65535.0).round() as u16
        );
    }

    #[test]
    fn logical_layout_divides_by_the_scale_and_physical_does_not() {
        let mut s = parse(SINGLE);
        s.logical[0].scale = 2.0;
        // layout-mode 1 (physical): the layout size is the pixel size.
        assert_eq!(s.outputs()[0].scale, 1.0);
        s.physical_layout = false;
        let o = &s.outputs()[0];
        assert_eq!((o.size, o.scale), ((1280, 720), 2.0));
    }

    #[test]
    fn rotated_monitors_swap_their_size() {
        let mut s = parse(SINGLE);
        s.logical[0].transform = 1;
        assert_eq!(s.outputs()[0].size, (720, 1280));
    }

    #[test]
    fn a_reply_without_layout_mode_is_logical() {
        let mut v: Json = serde_json::from_str(SINGLE).unwrap();
        v[3] = serde_json::json!({});
        let s = parse(&v.to_string());
        assert!(!s.physical_layout);
    }
}
