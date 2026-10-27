//! X11 input: the touch and pen devices are tied to the virtual monitor with the XInput2
//! `Coordinate Transformation Matrix` property, and the absolute pointer is placed over the whole
//! X screen by the sink's own mapper.
//!
//! The uinput devices show up as XInput2 slave devices a moment after they are created; their
//! `Device Node` property (set by the libinput and evdev drivers) names the `/dev/input/eventN`
//! node, which is how a device the sink created is found. The matrix maps the device's whole
//! range onto the monitor's rectangle inside the screen; the guard puts the identity back.

use crate::kscreen::KOutput;
use crate::uinput_sink::{DesktopInput, MappedDevices};
use crate::x11_output::{Rect, atom, connect, monitors, snapshot, xe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::randr::{self, NotifyMask};
use x11rb::protocol::xinput::{self, XIChangePropertyAux, XIGetPropertyItems};
use x11rb::protocol::xproto::{self, PropMode};
use x11rb::rust_connection::RustConnection;

/// How long a freshly created device may take to appear in the X server.
const DEVICE_WAIT: Duration = Duration::from_secs(3);
const IDENTITY: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
/// Name of the pseudo output that stands for the whole screen (see [`X11Desktop::outputs`]).
const SCREEN_OUTPUT: &str = "X11-screen";

/// The `Coordinate Transformation Matrix` (row-major 3x3) that maps a device's full range onto
/// `monitor` inside a `screen`-sized X screen.
pub fn transform_matrix(monitor: Rect, screen: (u32, u32)) -> [f32; 9] {
    let (sw, sh) = (screen.0.max(1) as f32, screen.1.max(1) as f32);
    [
        monitor.w as f32 / sw,
        0.0,
        monitor.x as f32 / sw,
        0.0,
        monitor.h as f32 / sh,
        monitor.y as f32 / sh,
        0.0,
        0.0,
        1.0,
    ]
}

fn find_devices(conn: &RustConnection, node: &str, device_node: xproto::Atom) -> Vec<u16> {
    let Some(all) = xinput::xi_query_device(conn, 0u16)
        .ok()
        .and_then(|c| c.reply().ok())
    else {
        return Vec::new();
    };
    all.infos
        .iter()
        .filter(|d| {
            xinput::xi_get_property(conn, d.deviceid, false, device_node, 0u32, 0, 64)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| match r.items {
                    XIGetPropertyItems::Data8(v) => {
                        String::from_utf8_lossy(&v).trim_end_matches('\0') == node
                    }
                    _ => false,
                })
        })
        .map(|d| d.deviceid)
        .collect()
}

/// Sets the matrix property of one XInput2 device.
pub fn set_matrix(
    conn: &RustConnection,
    device: u16,
    matrix: [f32; 9],
    property: xproto::Atom,
    float: xproto::Atom,
) -> Result<(), String> {
    let items = XIChangePropertyAux::Data32(matrix.iter().map(|v| v.to_bits()).collect());
    xinput::xi_change_property(conn, device, PropMode::REPLACE, property, float, 9, &items)
        .map_err(xe("XIChangeProperty"))?
        .check()
        .map_err(xe("XIChangeProperty"))
}

/// Reads the matrix property of one XInput2 device.
#[cfg(test)]
pub fn get_matrix(
    conn: &RustConnection,
    device: u16,
    property: xproto::Atom,
) -> Result<Option<[f32; 9]>, String> {
    let r = xinput::xi_get_property(conn, device, false, property, 0u32, 0, 9)
        .map_err(xe("XIGetProperty"))?
        .reply()
        .map_err(xe("XIGetProperty"))?;
    Ok(match r.items {
        XIGetPropertyItems::Data32(v) if v.len() == 9 => {
            let mut m = [0f32; 9];
            for (d, s) in m.iter_mut().zip(v) {
                *d = f32::from_bits(s);
            }
            Some(m)
        }
        _ => None,
    })
}

/// The atoms the matrix needs.
pub fn matrix_atoms(conn: &RustConnection) -> Result<(xproto::Atom, xproto::Atom), String> {
    Ok((
        atom(conn, "Coordinate Transformation Matrix")?,
        atom(conn, "FLOAT")?,
    ))
}

/// Announces XInput 2.0 to the server (required before any XInput2 request).
pub fn init_xinput(conn: &RustConnection) -> Result<(), String> {
    xinput::xi_query_version(conn, 2, 0)
        .map_err(|_| "the X server has no XInput extension".to_string())?
        .reply()
        .map_err(|_| "the X server has no XInput 2 extension".to_string())?;
    Ok(())
}

/// How long the watcher waits after an event before it looks at the devices and the layout.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// The matrix for the monitor called `output`, `None` when the monitor is not there.
pub fn matrix_for(
    monitors: &[crate::x11_output::MonitorRect],
    output: &str,
    screen: (u32, u32),
) -> Option<[f32; 9]> {
    monitors
        .iter()
        .find(|m| m.name == output)
        .map(|m| transform_matrix(m.rect, screen))
}

/// The devices in `found` that have not been given the matrix yet.
pub fn unmapped(found: &[u16], touched: &[u16]) -> Vec<u16> {
    found
        .iter()
        .copied()
        .filter(|d| !touched.contains(d))
        .collect()
}

/// What the mapping watcher keeps up to date.
struct MapState {
    output: String,
    /// `/dev/input/eventN` of the devices to map.
    nodes: Vec<String>,
    property: xproto::Atom,
    float: xproto::Atom,
    device_node: xproto::Atom,
    matrix: Mutex<[f32; 9]>,
    /// Every X device that got the matrix (so it can be restored).
    touched: Mutex<Vec<u16>>,
    stop: AtomicBool,
}

impl MapState {
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Gives the matrix to devices with one of our nodes that do not have it yet (the stylus of
    /// a tablet appears on the tool's first proximity, long after the parent device).
    fn map_new_devices(&self, conn: &RustConnection) {
        let matrix = *Self::lock(&self.matrix);
        let mut touched = Self::lock(&self.touched);
        for node in &self.nodes {
            let found = find_devices(conn, node, self.device_node);
            for id in unmapped(&found, &touched) {
                match set_matrix(conn, id, matrix, self.property, self.float) {
                    Ok(()) => {
                        tracing::info!(device = id, %node, "X11 transformation matrix set on a new device");
                        touched.push(id);
                    }
                    Err(e) => tracing::debug!("X11 matrix on device {id}: {e}"),
                }
            }
        }
    }

    /// Recomputes the matrix from the current layout and sets it again when it changed.
    fn remap(&self, conn: &RustConnection, root: xproto::Window) {
        let Ok(snap) = snapshot(conn, root) else {
            return;
        };
        let Some(matrix) = matrix_for(&snap.monitors, &self.output, snap.screen) else {
            return;
        };
        {
            let mut current = Self::lock(&self.matrix);
            if *current == matrix {
                return;
            }
            *current = matrix;
        }
        let touched = Self::lock(&self.touched).clone();
        for id in &touched {
            let _ = set_matrix(conn, *id, matrix, self.property, self.float);
        }
        tracing::info!(devices = ?touched, screen = ?snap.screen, "X11 transformation matrix re-applied");
    }
}

/// Watches device hierarchy and screen changes on its own connection.
fn watch(state: &MapState) {
    let started = connect().and_then(|(conn, n)| {
        init_xinput(&conn)?;
        let root = conn.setup().roots[n].root;
        xinput::xi_select_events(
            &conn,
            root,
            &[xinput::EventMask {
                deviceid: 0,
                mask: vec![xinput::XIEventMask::HIERARCHY],
            }],
        )
        .map_err(xe("XISelectEvents"))?
        .check()
        .map_err(xe("XISelectEvents"))?;
        randr::select_input(&conn, root, NotifyMask::SCREEN_CHANGE)
            .map_err(xe("RRSelectInput"))?
            .check()
            .map_err(xe("RRSelectInput"))?;
        conn.flush().map_err(xe("flush"))?;
        Ok((conn, root))
    });
    let (conn, root) = match started {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("X11 input watcher cannot start: {e}");
            return;
        }
    };
    // Whatever changed between the mapping and the subscription.
    state.remap(&conn, root);
    state.map_new_devices(&conn);
    let (mut devices_since, mut screen_since): (Option<Instant>, Option<Instant>) = (None, None);
    while !state.stop.load(Ordering::Relaxed) {
        loop {
            match conn.poll_for_event() {
                Ok(Some(Event::XinputHierarchy(_))) => devices_since = Some(Instant::now()),
                Ok(Some(Event::RandrScreenChangeNotify(_))) => screen_since = Some(Instant::now()),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("X11 input watcher stopped: {e}");
                    return;
                }
            }
        }
        if screen_since.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            screen_since = None;
            state.remap(&conn, root);
        }
        if devices_since.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            devices_since = None;
            state.map_new_devices(&conn);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Keeps the matrix current while the mapping lives, and puts the identity back when dropped.
pub struct MapGuard {
    conn: RustConnection,
    state: Arc<MapState>,
    watcher: Option<JoinHandle<()>>,
}

impl MapGuard {
    fn start(conn: RustConnection, state: MapState) -> MapGuard {
        let state = Arc::new(state);
        let s = state.clone();
        let watcher = std::thread::Builder::new()
            .name("ezd-x11-input".into())
            .spawn(move || watch(&s))
            .map_err(|e| tracing::warn!("X11: cannot start the input watcher: {e}"))
            .ok();
        MapGuard {
            conn,
            state,
            watcher,
        }
    }

    /// A guard over devices that are already known by id (no `Device Node` lookup).
    #[cfg(test)]
    pub fn for_devices(output: &str, devices: Vec<u16>) -> Result<MapGuard, String> {
        let (conn, _) = connect()?;
        init_xinput(&conn)?;
        let (property, float) = matrix_atoms(&conn)?;
        let device_node = atom(&conn, "Device Node")?;
        let state = MapState {
            output: output.into(),
            nodes: Vec::new(),
            property,
            float,
            device_node,
            matrix: Mutex::new(IDENTITY),
            touched: Mutex::new(devices),
            stop: AtomicBool::new(false),
        };
        Ok(MapGuard::start(conn, state))
    }
}

impl Drop for MapGuard {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.watcher.take() {
            let _ = t.join();
        }
        for d in MapState::lock(&self.state.touched).iter() {
            // The device is gone when the sink dropped it first; nothing to restore then.
            let _ = set_matrix(
                &self.conn,
                *d,
                IDENTITY,
                self.state.property,
                self.state.float,
            );
        }
    }
}

/// Per-device results and the guard that keeps and undoes the mapping.
type Mapping = (Vec<Result<(), String>>, Option<MapGuard>);

/// X11: transformation matrices for the touch and pen devices.
#[derive(Default)]
pub struct X11Desktop;

impl X11Desktop {
    fn map(&self, devices: &[(&str, &str)], output: &str) -> Result<Mapping, String> {
        let (conn, screen_num) = connect()?;
        init_xinput(&conn)?;
        let root = conn.setup().roots[screen_num].root;
        let snap = snapshot(&conn, root)?;
        let matrix = matrix_for(&snap.monitors, output, snap.screen)
            .ok_or_else(|| format!("the X server has no monitor called {output}"))?;
        let (property, float) = matrix_atoms(&conn)?;
        let device_node = atom(&conn, "Device Node")?;

        let wanted: Vec<String> = devices
            .iter()
            .map(|(_, event)| format!("/dev/input/{event}"))
            .collect();
        let mut found: Vec<Vec<u16>> = vec![Vec::new(); devices.len()];
        let deadline = Instant::now() + DEVICE_WAIT;
        loop {
            for (ids, node) in found.iter_mut().zip(&wanted) {
                if ids.is_empty() {
                    *ids = find_devices(&conn, node, device_node);
                }
            }
            if found.iter().all(|ids| !ids.is_empty()) || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let mut mapped = Vec::new();
        let results = devices
            .iter()
            .zip(&found)
            .map(|((label, event), ids)| {
                if ids.is_empty() {
                    return Err(format!(
                        "the X server did not add /dev/input/{event} as an input device (is it \
                         ignored by the server's input configuration?)"
                    ));
                }
                // A pen can be several X devices (stylus, eraser, pad): all get the matrix.
                for id in ids {
                    set_matrix(&conn, *id, matrix, property, float)?;
                    mapped.push(*id);
                }
                tracing::info!(device = *label, devices = ?ids, %output, "X11 transformation matrix set");
                Ok(())
            })
            .collect();
        // Devices the driver adds later (a tablet's stylus) are picked up by the watcher.
        let guard = (!wanted.is_empty()).then(|| {
            MapGuard::start(
                conn,
                MapState {
                    output: output.into(),
                    nodes: wanted,
                    property,
                    float,
                    device_node,
                    matrix: Mutex::new(matrix),
                    touched: Mutex::new(mapped),
                    stop: AtomicBool::new(false),
                },
            )
        });
        Ok((results, guard))
    }
}

impl DesktopInput for X11Desktop {
    fn map_devices(&self, devices: &[(&str, &str)], output: &str) -> MappedDevices {
        match self.map(devices, output) {
            Ok((results, guard)) => MappedDevices {
                results,
                guard: guard.map(|g| Box::new(g) as Box<dyn Send>),
            },
            Err(e) => MappedDevices {
                results: devices.iter().map(|_| Err(e.clone())).collect(),
                guard: None,
            },
        }
    }

    /// The monitors, plus one output covering the whole screen: the absolute pointer device is
    /// spread over the entire X screen, which can be larger than the union of the monitors.
    fn outputs(&self) -> Option<Vec<KOutput>> {
        let (conn, screen_num) = connect().ok()?;
        let root = conn.setup().roots[screen_num].root;
        let screen = snapshot(&conn, root).ok()?.screen;
        let mons = monitors(&conn, root).ok()?;
        let output = |name: String, id: i64, r: Rect| KOutput {
            name,
            id,
            enabled: true,
            connected: true,
            size: (r.w, r.h),
            pos: (r.x, r.y),
            scale: 1.0,
            current_mode_id: String::new(),
            modes: Vec::new(),
        };
        let mut out: Vec<KOutput> = mons
            .into_iter()
            .enumerate()
            .map(|(i, m)| output(m.name, i as i64 + 1, m.rect))
            .collect();
        out.push(output(
            SCREEN_OUTPUT.into(),
            0,
            Rect {
                x: 0,
                y: 0,
                w: screen.0,
                h: screen.1,
            },
        ));
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn the_matrix_places_the_device_on_the_monitor() {
        // 1920x1080 screen plus a 1920x1080 monitor to its right.
        let m = transform_matrix(r(1920, 0, 1920, 1080), (3840, 1080));
        assert_eq!(m, [0.5, 0.0, 0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        // A monitor lower in a taller screen.
        let m = transform_matrix(r(0, 1080, 1920, 1080), (1920, 2160));
        assert_eq!(m, [1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0]);
        // The monitor is the whole screen: identity.
        assert_eq!(transform_matrix(r(0, 0, 800, 600), (800, 600)), IDENTITY);
    }

    #[test]
    fn the_corners_land_on_the_monitor_corners() {
        let (mon, screen) = (r(1920, 0, 1280, 800), (3200u32, 1080u32));
        let m = transform_matrix(mon, screen);
        let map = |u: f32, v: f32| (m[0] * u + m[1] * v + m[2], m[3] * u + m[4] * v + m[5]);
        let (x0, y0) = map(0.0, 0.0);
        let (x1, y1) = map(1.0, 1.0);
        assert!((x0 * screen.0 as f32 - 1920.0).abs() < 1e-2);
        assert!(y0.abs() < 1e-6);
        assert!((x1 * screen.0 as f32 - 3200.0).abs() < 1e-2);
        assert!((y1 * screen.1 as f32 - 800.0).abs() < 1e-2);
    }

    #[test]
    fn only_new_devices_need_the_matrix() {
        assert_eq!(unmapped(&[4, 5, 6], &[4]), vec![5, 6]);
        assert!(unmapped(&[4], &[4, 9]).is_empty());
        assert!(unmapped(&[], &[4]).is_empty());
    }

    #[test]
    fn the_matrix_follows_the_screen_size() {
        use crate::x11_output::MonitorRect;
        let mon = |name: &str, rect| MonitorRect {
            name: name.into(),
            primary: false,
            rect,
            mm: (0, 0),
        };
        let mons = [
            mon("A", r(0, 0, 1920, 1080)),
            mon("EZDISPLAY", r(1920, 0, 1280, 720)),
        ];
        let before = matrix_for(&mons, "EZDISPLAY", (3200, 1080)).unwrap();
        let after = matrix_for(&mons, "EZDISPLAY", (3400, 1080)).unwrap();
        assert_ne!(before, after);
        assert_eq!(
            before,
            transform_matrix(r(1920, 0, 1280, 720), (3200, 1080))
        );
        assert_eq!(matrix_for(&mons, "missing", (3200, 1080)), None);
    }

    #[test]
    fn a_degenerate_screen_does_not_divide_by_zero() {
        let m = transform_matrix(r(0, 0, 10, 10), (0, 0));
        assert!(m.iter().all(|v| v.is_finite()));
    }
}
