//! X11: a virtual monitor on an X server, and the helpers the capture and input halves share.
//!
//! An X server cannot create outputs, so the virtual monitor is a region of the screen that no
//! real output shows:
//!
//! 1. The screen (the root window) grows to the right by the tablet's width (`set_screen_size`).
//! 2. A RandR 1.5 monitor called `EZDISPLAY` covers the new area. Desktops that follow RandR
//!    monitors (panels, maximizing, fullscreen) treat it as one more display.
//! 3. The server clamps the pointer to the CRTC areas (`RRConstrainCursorHarder`), so the new area
//!    would be unreachable. The clamp is skipped while any CRTC has panning enabled, so one CRTC
//!    gets panning over its own rectangle: the CRTC does not move, only the constraint goes away.
//!
//! `x11_capture.rs` grabs the rectangle from the root window and `x11_input.rs` maps the tablet's
//! devices onto it. Dropping the guard removes the monitor, restores the panning and shrinks the
//! screen. Desktop daemons (kscreen, gnome-settings-daemon, xfsettingsd) reset the layout when
//! they see it change, so a watcher thread applies it again, rate limited, while the guard lives.

use crate::portal::SourceKind;
use crate::provider::{FrameSource, OpenedOutput, OutputRequest, VirtualOutputProvider};
use ezd_host_core::{DisplayMode, OutputInfo, SourceError};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::randr::{self, NotifyMask};
use x11rb::protocol::xproto::{self, AtomEnum};
use x11rb::rust_connection::RustConnection;

/// Name of the RandR monitor of the virtual output.
pub const MONITOR_NAME: &str = "EZDISPLAY";

/// How long the watcher waits after an event before it looks at the layout, so a daemon that
/// changes it in several steps is seen once.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Some changes (a deleted monitor) may send no RandR event: the layout is also checked this often.
const RECHECK: Duration = Duration::from_secs(2);
/// The watcher applies the layout at most this many times per [`REAPPLY_WINDOW`].
const REAPPLY_LIMIT: usize = 3;
const REAPPLY_WINDOW: Duration = Duration::from_secs(10);
/// A screen whose pixel density is unknown is assumed to be this.
const FALLBACK_DPI: f64 = 96.0;

/// Formats an X11 error with what was being asked.
pub fn xe<E: Display>(what: &'static str) -> impl Fn(E) -> String {
    move |e| format!("X11 {what}: {e}")
}

/// Connects to the X server of `$DISPLAY`; returns the connection and its screen number.
pub fn connect() -> Result<(RustConnection, usize), String> {
    RustConnection::connect(None).map_err(|e| {
        let display = std::env::var("DISPLAY").unwrap_or_default();
        format!("cannot connect to the X server (DISPLAY={display:?}): {e}")
    })
}

/// Interns `name`.
pub fn atom(conn: &RustConnection, name: &str) -> Result<xproto::Atom, String> {
    Ok(xproto::intern_atom(conn, false, name.as_bytes())
        .map_err(xe("InternAtom"))?
        .reply()
        .map_err(xe("InternAtom"))?
        .atom)
}

/// A rectangle in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.w as i32)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h as i32)
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// Where the virtual monitor goes and how large the screen becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The monitor's rectangle.
    pub monitor: Rect,
    /// The screen size that holds it and everything that was there.
    pub screen: (u32, u32),
}

/// Places a `size` monitor to the right of everything in `areas` (enabled CRTCs and monitors),
/// at the top. The screen grows to hold it and never shrinks below `current`.
pub fn placement(areas: &[Rect], current: (u32, u32), size: (u32, u32)) -> Layout {
    let x = areas.iter().map(Rect::right).max().unwrap_or(0).max(0);
    Layout {
        monitor: Rect {
            x,
            y: 0,
            w: size.0,
            h: size.1,
        },
        screen: (x as u32 + size.0, current.1.max(size.1)),
    }
}

/// Physical size for `px` pixels at the density of the current screen (`cur_px` pixels over
/// `cur_mm` millimetres), or at 96 DPI when the screen reports none.
pub fn scaled_mm(px: (u32, u32), cur_px: (u32, u32), cur_mm: (u32, u32)) -> (u32, u32) {
    let axis = |px: u32, cur_px: u32, cur_mm: u32| -> u32 {
        let mm = if cur_px > 0 && cur_mm > 0 {
            f64::from(px) * f64::from(cur_mm) / f64::from(cur_px)
        } else {
            f64::from(px) * 25.4 / FALLBACK_DPI
        };
        (mm.round() as u32).max(1)
    };
    (
        axis(px.0, cur_px.0, cur_mm.0),
        axis(px.1, cur_px.1, cur_mm.1),
    )
}

/// The screen size to go back to: `original`, unless an enabled CRTC now reaches past it (the
/// user added a display meanwhile), then the smallest size that holds them all.
pub fn restore_size(original: (u32, u32), crtcs: &[Rect]) -> (u32, u32) {
    let w = crtcs
        .iter()
        .map(|r| r.right().max(0) as u32)
        .max()
        .unwrap_or(0);
    let h = crtcs
        .iter()
        .map(|r| r.bottom().max(0) as u32)
        .max()
        .unwrap_or(0);
    if w > original.0 || h > original.1 {
        (w, h)
    } else {
        original
    }
}

/// An enabled CRTC.
#[derive(Debug, Clone, Copy)]
pub struct Crtc {
    pub id: randr::Crtc,
    pub rect: Rect,
}

/// What the server shows now.
pub struct Snapshot {
    /// Screen size in pixels (`GetGeometry` of the root: the setup is stale after a change).
    pub screen: (u32, u32),
    /// Enabled CRTCs.
    pub crtcs: Vec<Crtc>,
    /// Active RandR monitors.
    pub monitors: Vec<MonitorRect>,
    /// CRTC of the primary output, if it has one.
    pub primary_crtc: Option<randr::Crtc>,
}

/// An active RandR monitor.
#[derive(Debug, Clone)]
pub struct MonitorRect {
    pub name: String,
    pub primary: bool,
    pub rect: Rect,
    pub mm: (u32, u32),
}

/// Reads the RandR state of `root`.
pub fn snapshot(conn: &RustConnection, root: xproto::Window) -> Result<Snapshot, String> {
    let geo = xproto::get_geometry(conn, root)
        .map_err(xe("GetGeometry"))?
        .reply()
        .map_err(xe("GetGeometry"))?;
    let res = randr::get_screen_resources_current(conn, root)
        .map_err(xe("RRGetScreenResourcesCurrent"))?
        .reply()
        .map_err(xe("RRGetScreenResourcesCurrent"))?;
    let cookies: Vec<_> = res
        .crtcs
        .iter()
        .map(|c| randr::get_crtc_info(conn, *c, res.config_timestamp).map(|k| (*c, k)))
        .collect::<Result<_, _>>()
        .map_err(xe("RRGetCrtcInfo"))?;
    let mut crtcs = Vec::new();
    for (id, cookie) in cookies {
        let info = cookie.reply().map_err(xe("RRGetCrtcInfo"))?;
        if info.mode != 0 && info.width > 0 && info.height > 0 {
            crtcs.push(Crtc {
                id,
                rect: Rect {
                    x: i32::from(info.x),
                    y: i32::from(info.y),
                    w: u32::from(info.width),
                    h: u32::from(info.height),
                },
            });
        }
    }
    let monitors = monitors(conn, root)?;
    let primary = randr::get_output_primary(conn, root)
        .map_err(xe("RRGetOutputPrimary"))?
        .reply()
        .map_err(xe("RRGetOutputPrimary"))?
        .output;
    let primary_crtc = if primary == 0 {
        None
    } else {
        let crtc = randr::get_output_info(conn, primary, res.config_timestamp)
            .map_err(xe("RRGetOutputInfo"))?
            .reply()
            .map_err(xe("RRGetOutputInfo"))?
            .crtc;
        (crtc != 0).then_some(crtc)
    };
    Ok(Snapshot {
        screen: (u32::from(geo.width), u32::from(geo.height)),
        crtcs,
        monitors,
        primary_crtc,
    })
}

/// The active RandR 1.5 monitors.
pub fn monitors(conn: &RustConnection, root: xproto::Window) -> Result<Vec<MonitorRect>, String> {
    let reply = randr::get_monitors(conn, root, true)
        .map_err(xe("RRGetMonitors"))?
        .reply()
        .map_err(xe("RRGetMonitors"))?;
    let mut out = Vec::new();
    for m in reply.monitors {
        let name = xproto::get_atom_name(conn, m.name)
            .map_err(xe("GetAtomName"))?
            .reply()
            .map_err(xe("GetAtomName"))?
            .name;
        out.push(MonitorRect {
            name: String::from_utf8_lossy(&name).into_owned(),
            primary: m.primary,
            rect: Rect {
                x: i32::from(m.x),
                y: i32::from(m.y),
                w: u32::from(m.width),
                h: u32::from(m.height),
            },
            mm: (m.width_in_millimeters, m.height_in_millimeters),
        });
    }
    Ok(out)
}

/// RandR version of the server, `Err` below 1.5 (monitors).
pub fn require_randr_15(conn: &RustConnection) -> Result<(u32, u32), String> {
    let v = randr::query_version(conn, 1, 5)
        .map_err(|_| "the X server has no RandR extension".to_string())?
        .reply()
        .map_err(|_| "the X server has no RandR extension".to_string())?;
    let version = (v.major_version, v.minor_version);
    if version < (1, 5) {
        return Err(format!(
            "the X server speaks RandR {}.{}; 1.5 (monitors) is required",
            version.0, version.1
        ));
    }
    Ok(version)
}

/// What the layout needs to be applied again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    layout: Layout,
    /// Screen size in millimetres.
    screen_mm: (u32, u32),
    /// Monitor size in millimetres.
    monitor_mm: (u32, u32),
}

/// The CRTC that carries the panning, and the panning it had before.
#[derive(Debug, Clone, Copy)]
struct SavedPanning {
    crtc: randr::Crtc,
    previous: randr::GetPanningReply,
}

fn panning_enabled(conn: &RustConnection, crtc: randr::Crtc) -> Result<bool, String> {
    Ok(randr::get_panning(conn, crtc)
        .map_err(xe("RRGetPanning"))?
        .reply()
        .map_err(xe("RRGetPanning"))?
        .width
        > 0)
}

/// The CRTC's panning area, `None` when panning is off.
fn panning_area(conn: &RustConnection, crtc: randr::Crtc) -> Result<Option<Rect>, String> {
    let r = randr::get_panning(conn, crtc)
        .map_err(xe("RRGetPanning"))?
        .reply()
        .map_err(xe("RRGetPanning"))?;
    Ok((r.width > 0).then(|| Rect {
        x: i32::from(r.left),
        y: i32::from(r.top),
        w: u32::from(r.width),
        h: u32::from(r.height),
    }))
}

fn set_panning_to(
    conn: &RustConnection,
    crtc: randr::Crtc,
    area: Rect,
    track: (u16, u16, u16, u16),
    border: (i16, i16, i16, i16),
) -> Result<(), String> {
    let r = randr::set_panning(
        conn,
        crtc,
        x11rb::CURRENT_TIME,
        area.x.max(0) as u16,
        area.y.max(0) as u16,
        area.w as u16,
        area.h as u16,
        track.0,
        track.1,
        track.2,
        track.3,
        border.0,
        border.1,
        border.2,
        border.3,
    )
    .map_err(xe("RRSetPanning"))?
    .reply()
    .map_err(xe("RRSetPanning"))?;
    if r.status != randr::SetConfig::SUCCESS {
        return Err(format!("X11 RRSetPanning: status {:?}", r.status));
    }
    Ok(())
}

/// Makes sure some CRTC has panning over its own rectangle, so the server does not clamp the
/// pointer to the CRTC areas.
fn ensure_panning(
    conn: &RustConnection,
    snap: &Snapshot,
    saved: &mut Option<SavedPanning>,
) -> Result<(), String> {
    if let Some(p) = *saved
        && let Some(c) = snap.crtcs.iter().find(|c| c.id == p.crtc)
    {
        // Also when the CRTC moved: a stale panning area would pull it back to where it was.
        if panning_area(conn, p.crtc)? != Some(c.rect) {
            set_panning_to(conn, p.crtc, c.rect, (0, 0, 0, 0), (0, 0, 0, 0))?;
            tracing::info!("X11 panning applied again");
        }
        return Ok(());
    }
    for c in &snap.crtcs {
        if panning_enabled(conn, c.id)? {
            // Someone else's panning already lifts the pointer constraint.
            return Ok(());
        }
    }
    let Some(c) = snap
        .primary_crtc
        .and_then(|p| snap.crtcs.iter().find(|c| c.id == p))
        .or(snap.crtcs.first())
    else {
        tracing::warn!(
            "X11: no enabled CRTC to carry the panning; the pointer may not reach the monitor"
        );
        return Ok(());
    };
    let previous = randr::get_panning(conn, c.id)
        .map_err(xe("RRGetPanning"))?
        .reply()
        .map_err(xe("RRGetPanning"))?;
    set_panning_to(conn, c.id, c.rect, (0, 0, 0, 0), (0, 0, 0, 0))?;
    tracing::info!(
        crtc = c.id,
        "X11 panning enabled to lift the pointer constraint"
    );
    *saved = Some(SavedPanning {
        crtc: c.id,
        previous,
    });
    Ok(())
}

fn monitor_present(
    conn: &RustConnection,
    root: xproto::Window,
    plan: &Plan,
) -> Result<bool, String> {
    Ok(monitors(conn, root)?
        .iter()
        .any(|m| m.name == MONITOR_NAME && m.rect == plan.layout.monitor))
}

/// Brings the server to `plan`: screen size, panning, monitor. Idempotent.
fn apply(
    conn: &RustConnection,
    root: xproto::Window,
    plan: &Plan,
    saved: &mut Option<SavedPanning>,
) -> Result<(), String> {
    xproto::grab_server(conn).map_err(xe("GrabServer"))?;
    let r = apply_grabbed(conn, root, plan, saved);
    let _ = xproto::ungrab_server(conn);
    let _ = conn.flush();
    r
}

fn apply_grabbed(
    conn: &RustConnection,
    root: xproto::Window,
    plan: &Plan,
    saved: &mut Option<SavedPanning>,
) -> Result<(), String> {
    let snap = snapshot(conn, root)?;
    if snap.screen.0 < plan.layout.screen.0 || snap.screen.1 < plan.layout.screen.1 {
        let size = (
            snap.screen.0.max(plan.layout.screen.0),
            snap.screen.1.max(plan.layout.screen.1),
        );
        randr::set_screen_size(
            conn,
            root,
            size.0 as u16,
            size.1 as u16,
            plan.screen_mm.0,
            plan.screen_mm.1,
        )
        .map_err(xe("RRSetScreenSize"))?
        .check()
        .map_err(xe("RRSetScreenSize"))?;
    }
    let snap = snapshot(conn, root)?;
    ensure_panning(conn, &snap, saved)?;
    if !monitor_present(conn, root, plan)? {
        let name = atom(conn, MONITOR_NAME)?;
        let m = plan.layout.monitor;
        randr::set_monitor(
            conn,
            root,
            randr::MonitorInfo {
                name,
                primary: false,
                automatic: false,
                x: m.x as i16,
                y: m.y as i16,
                width: m.w as u16,
                height: m.h as u16,
                width_in_millimeters: plan.monitor_mm.0,
                height_in_millimeters: plan.monitor_mm.1,
                outputs: Vec::new(),
            },
        )
        .map_err(xe("RRSetMonitor"))?
        .check()
        .map_err(xe("RRSetMonitor"))?;
    }
    Ok(())
}

/// True if the server no longer matches `plan` (a desktop daemon reset the layout).
fn drifted(
    conn: &RustConnection,
    root: xproto::Window,
    plan: &Plan,
    saved: &Option<SavedPanning>,
) -> Result<bool, String> {
    let snap = snapshot(conn, root)?;
    if snap.screen.0 < plan.layout.screen.0 || snap.screen.1 < plan.layout.screen.1 {
        return Ok(true);
    }
    if !snap
        .monitors
        .iter()
        .any(|m| m.name == MONITOR_NAME && m.rect == plan.layout.monitor)
    {
        return Ok(true);
    }
    if let Some(p) = saved {
        let Some(c) = snap.crtcs.iter().find(|c| c.id == p.crtc) else {
            return Ok(true);
        };
        return Ok(panning_area(conn, p.crtc)? != Some(c.rect));
    }
    Ok(false)
}

/// State shared by the guard and its watcher.
struct Shared {
    plan: Plan,
    saved: Mutex<Option<SavedPanning>>,
    stop: AtomicBool,
}

/// Keeps the virtual monitor: applies the layout again when a desktop daemon resets it, and
/// undoes everything when dropped.
pub struct MonitorGuard {
    conn: RustConnection,
    root: xproto::Window,
    original: (u32, u32),
    original_mm: (u32, u32),
    shared: Arc<Shared>,
    watcher: Option<JoinHandle<()>>,
}

impl Drop for MonitorGuard {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.watcher.take() {
            let _ = t.join();
        }
        if let Err(e) = self.restore() {
            tracing::warn!("X11: restoring the screen failed: {e}");
        }
    }
}

impl MonitorGuard {
    fn restore(&self) -> Result<(), String> {
        xproto::grab_server(&self.conn).map_err(xe("GrabServer"))?;
        let r = self.restore_grabbed();
        let _ = xproto::ungrab_server(&self.conn);
        let _ = self.conn.flush();
        r
    }

    fn restore_grabbed(&self) -> Result<(), String> {
        let conn = &self.conn;
        let root = self.root;
        let mut result = Ok(());
        let mut keep = |r: Result<(), String>| {
            if let Err(e) = r {
                tracing::warn!("X11 restore: {e}");
                if result.is_ok() {
                    result = Err(e);
                }
            }
        };
        keep(atom(conn, MONITOR_NAME).and_then(|a| {
            randr::delete_monitor(conn, root, a)
                .map_err(xe("RRDeleteMonitor"))?
                .check()
                .map_err(xe("RRDeleteMonitor"))
        }));
        let saved = self
            .shared
            .saved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(p) = saved {
            let q = p.previous;
            let area = Rect {
                x: i32::from(q.left),
                y: i32::from(q.top),
                w: u32::from(q.width),
                h: u32::from(q.height),
            };
            keep(set_panning_to(
                conn,
                p.crtc,
                area,
                (q.track_left, q.track_top, q.track_width, q.track_height),
                (q.border_left, q.border_top, q.border_right, q.border_bottom),
            ));
        }
        keep(snapshot(conn, root).and_then(|snap| {
            let rects: Vec<Rect> = snap.crtcs.iter().map(|c| c.rect).collect();
            let size = restore_size(self.original, &rects);
            if size == snap.screen {
                return Ok(());
            }
            let mm = if size == self.original {
                self.original_mm
            } else {
                scaled_mm(size, self.original, self.original_mm)
            };
            randr::set_screen_size(conn, root, size.0 as u16, size.1 as u16, mm.0, mm.1)
                .map_err(xe("RRSetScreenSize"))?
                .check()
                .map_err(xe("RRSetScreenSize"))?;
            tracing::info!(width = size.0, height = size.1, "X11 screen size restored");
            Ok(())
        }));
        result
    }
}

/// The watcher: applies the layout again after a desktop daemon changed it.
fn watch(shared: &Shared, root: xproto::Window) {
    let (conn, _) = match connect() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("X11 layout watcher cannot start: {e}");
            return;
        }
    };
    let selected = randr::select_input(
        &conn,
        root,
        NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE,
    )
    .map_err(xe("RRSelectInput"))
    .and_then(|c| c.check().map_err(xe("RRSelectInput")))
    .and_then(|()| conn.flush().map_err(xe("flush")));
    if let Err(e) = selected {
        tracing::warn!("X11 layout watcher cannot start: {e}");
        return;
    }
    let mut recent: VecDeque<Instant> = VecDeque::new();
    let mut dirty_since: Option<Instant> = None;
    let mut warned_limit = false;
    let mut last_check = Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        loop {
            match conn.poll_for_event() {
                Ok(Some(Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_))) => {
                    dirty_since = Some(Instant::now());
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("X11 layout watcher stopped: {e}");
                    return;
                }
            }
        }
        if dirty_since.is_none() && last_check.elapsed() >= RECHECK {
            dirty_since = Some(Instant::now() - DEBOUNCE);
        }
        if dirty_since.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            last_check = Instant::now();
            let mut saved = shared.saved.lock().unwrap_or_else(|e| e.into_inner());
            match drifted(&conn, root, &shared.plan, &saved) {
                Ok(false) => {
                    dirty_since = None;
                    warned_limit = false;
                }
                Ok(true) => {
                    let now = Instant::now();
                    while recent.front().is_some_and(|t| now - *t > REAPPLY_WINDOW) {
                        recent.pop_front();
                    }
                    if recent.len() >= REAPPLY_LIMIT {
                        if !std::mem::replace(&mut warned_limit, true) {
                            tracing::warn!(
                                "X11: a desktop daemon keeps resetting the virtual monitor; \
                                 applying it again later"
                            );
                        }
                        // Look again after the window has moved on.
                        dirty_since = Some(now + REAPPLY_WINDOW / 2 - DEBOUNCE);
                    } else {
                        recent.push_back(now);
                        tracing::warn!("X11: the virtual monitor was reset; applying it again");
                        match apply(&conn, root, &shared.plan, &mut saved) {
                            Ok(()) => dirty_since = Some(Instant::now()),
                            Err(e) => {
                                tracing::warn!(
                                    "X11: applying the virtual monitor again failed: {e}"
                                );
                                dirty_since = Some(Instant::now());
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("X11 layout check failed: {e}");
                    dirty_since = Some(Instant::now());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Creates the virtual monitor on the X server of `$DISPLAY`.
pub struct X11Provider;

impl VirtualOutputProvider for X11Provider {
    fn name(&self) -> &'static str {
        "x11"
    }

    fn open(&mut self, req: &OutputRequest) -> Result<OpenedOutput, SourceError> {
        open(req).map_err(SourceError::msg)
    }
}

fn mode_of(req: &OutputRequest, rect: Rect) -> DisplayMode {
    DisplayMode {
        width: rect.w,
        height: rect.h,
        refresh_mhz: req.refresh_mhz,
    }
}

fn open(req: &OutputRequest) -> Result<OpenedOutput, String> {
    let (conn, screen_num) = connect()?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let original = (
        u32::from(screen.width_in_pixels),
        u32::from(screen.height_in_pixels),
    );
    let original_mm = (
        u32::from(screen.width_in_millimeters),
        u32::from(screen.height_in_millimeters),
    );
    require_randr_15(&conn)?;

    if req.kind == SourceKind::Monitor {
        let name = req.output_name.clone().ok_or("no output name to capture")?;
        let mons = monitors(&conn, root)?;
        let m = mons.iter().find(|m| m.name == name).ok_or_else(|| {
            format!(
                "the X server has no monitor called {name} (monitors: {})",
                mons.iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        return Ok(OpenedOutput {
            frames: FrameSource::X11 {
                rect: (m.rect.x, m.rect.y, m.rect.w, m.rect.h),
            },
            info: Some(OutputInfo {
                name,
                mode: mode_of(req, m.rect),
                origin: (m.rect.x, m.rect.y),
                adapter: None,
            }),
            guard: Box::new(()),
        });
    }

    let snap = snapshot(&conn, root)?;
    // A monitor left behind by an earlier run must not push the new one further right.
    let areas: Vec<Rect> = snap
        .crtcs
        .iter()
        .map(|c| c.rect)
        .chain(
            snap.monitors
                .iter()
                .filter(|m| m.name != MONITOR_NAME)
                .map(|m| m.rect),
        )
        .collect();
    let layout = placement(&areas, snap.screen, (req.width, req.height));
    let range = randr::get_screen_size_range(&conn, root)
        .map_err(xe("RRGetScreenSizeRange"))?
        .reply()
        .map_err(xe("RRGetScreenSizeRange"))?;
    if layout.screen.0 > u32::from(range.max_width) || layout.screen.1 > u32::from(range.max_height)
    {
        return Err(format!(
            "the X screen would have to be {}x{} to hold the {}x{} monitor, but the server allows \
             at most {}x{} (Xorg `Virtual` in the Screen section raises it)",
            layout.screen.0,
            layout.screen.1,
            req.width,
            req.height,
            range.max_width,
            range.max_height
        ));
    }
    if layout.monitor.x > i32::from(i16::MAX)
        || req.width > u32::from(u16::MAX)
        || req.height > u32::from(u16::MAX)
    {
        return Err("the virtual monitor does not fit the X11 coordinate range".into());
    }
    let plan = Plan {
        layout,
        screen_mm: scaled_mm(layout.screen, original, original_mm),
        monitor_mm: scaled_mm((req.width, req.height), original, original_mm),
    };
    let shared = Arc::new(Shared {
        plan,
        saved: Mutex::new(None),
        stop: AtomicBool::new(false),
    });
    // From here on the guard undoes whatever was applied, also on an early return.
    let mut guard = MonitorGuard {
        conn,
        root,
        original,
        original_mm,
        shared: shared.clone(),
        watcher: None,
    };
    {
        let mut saved = shared.saved.lock().unwrap_or_else(|e| e.into_inner());
        apply(&guard.conn, root, &plan, &mut saved)?;
    }
    let m = layout.monitor;
    tracing::info!(
        x = m.x,
        y = m.y,
        width = m.w,
        height = m.h,
        screen = ?layout.screen,
        "X11 virtual monitor {MONITOR_NAME} created"
    );
    let watcher_shared = shared.clone();
    guard.watcher = std::thread::Builder::new()
        .name("ezd-x11-layout".into())
        .spawn(move || watch(&watcher_shared, root))
        .map_err(|e| tracing::warn!("X11: cannot start the layout watcher: {e}"))
        .ok();
    Ok(OpenedOutput {
        frames: FrameSource::X11 {
            rect: (m.x, m.y, m.w, m.h),
        },
        info: Some(OutputInfo {
            name: MONITOR_NAME.into(),
            mode: mode_of(req, m),
            origin: (m.x, m.y),
            adapter: None,
        }),
        guard: Box::new(guard),
    })
}

/// Value of a window property as text (first item).
fn text_property(
    conn: &RustConnection,
    window: xproto::Window,
    property: xproto::Atom,
) -> Option<String> {
    let r = xproto::get_property(conn, false, window, property, AtomEnum::ANY, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    (r.format == 8 && !r.value.is_empty()).then(|| {
        String::from_utf8_lossy(&r.value)
            .trim_end_matches('\0')
            .to_string()
    })
}

/// The window manager's name (`_NET_SUPPORTING_WM_CHECK` -> `_NET_WM_NAME`).
fn window_manager(conn: &RustConnection, root: xproto::Window) -> Option<String> {
    let check = atom(conn, "_NET_SUPPORTING_WM_CHECK").ok()?;
    let r = xproto::get_property(conn, false, root, check, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let win = r.value32()?.next()?;
    text_property(conn, win, atom(conn, "_NET_WM_NAME").ok()?)
}

/// The X11 part of `ezd-host diag`.
pub fn diag() -> Value {
    let probe = || -> Result<Value, String> {
        let (conn, screen_num) = connect()?;
        let setup = conn.setup();
        let root = setup.roots[screen_num].root;
        let randr = require_randr_15(&conn);
        let versions = json!({
            "randr": randr.as_ref().ok().map(|v| format!("{}.{}", v.0, v.1)),
            "randr_error": randr.as_ref().err(),
            "shm": x11rb::protocol::shm::query_version(&conn)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|v| format!("{}.{}", v.major_version, v.minor_version)),
            "xfixes": x11rb::protocol::xfixes::query_version(&conn, 5, 0)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|v| format!("{}.{}", v.major_version, v.minor_version)),
            "damage": x11rb::protocol::damage::query_version(&conn, 1, 1)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|v| format!("{}.{}", v.major_version, v.minor_version)),
            "xinput": x11rb::protocol::xinput::xi_query_version(&conn, 2, 2)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|v| format!("{}.{}", v.major_version, v.minor_version)),
        });
        let mons = monitors(&conn, root).unwrap_or_default();
        let cm = atom(&conn, &format!("_NET_WM_CM_S{screen_num}"))
            .ok()
            .and_then(|a| xproto::get_selection_owner(&conn, a).ok())
            .and_then(|c| c.reply().ok())
            .map(|r| r.owner != 0);
        Ok(json!({
            "display": std::env::var("DISPLAY").ok(),
            "vendor": String::from_utf8_lossy(&setup.vendor),
            "release": setup.release_number,
            "extensions": versions,
            "screen": [setup.roots[screen_num].width_in_pixels, setup.roots[screen_num].height_in_pixels],
            "monitors": mons.iter().map(|m| json!({
                "name": m.name,
                "primary": m.primary,
                "size": [m.rect.w, m.rect.h],
                "position": [m.rect.x, m.rect.y],
                "mm": [m.mm.0, m.mm.1],
            })).collect::<Vec<_>>(),
            "window_manager": window_manager(&conn, root),
            "compositor_running": cm,
        }))
    };
    probe().unwrap_or_else(|e| json!({ "error": e }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn the_monitor_goes_right_of_everything() {
        let l = placement(&[r(0, 0, 1920, 1080)], (1920, 1080), (2560, 1600));
        assert_eq!(l.monitor, r(1920, 0, 2560, 1600));
        assert_eq!(l.screen, (4480, 1600));
        // The screen never shrinks below its current height.
        let l = placement(&[r(0, 0, 1920, 1080)], (1920, 2000), (1280, 800));
        assert_eq!(l.screen, (3200, 2000));
    }

    #[test]
    fn the_rightmost_area_decides() {
        let areas = [
            r(0, 0, 1920, 1080),
            r(1920, 200, 1280, 1024),
            r(-1000, 0, 1000, 800),
        ];
        let l = placement(&areas, (3200, 1224), (1000, 1000));
        assert_eq!(l.monitor.x, 3200);
        assert_eq!(l.screen, (4200, 1224));
        assert_eq!(placement(&[], (0, 0), (800, 600)).monitor.x, 0);
        // Only areas left of the origin: still starts at 0.
        assert_eq!(
            placement(&[r(-500, 0, 400, 300)], (100, 100), (10, 10))
                .monitor
                .x,
            0
        );
    }

    #[test]
    fn millimetres_keep_the_density() {
        // 1920x1080 on 508x286 mm: a 3840-wide screen is twice as wide.
        assert_eq!(
            scaled_mm((3840, 1080), (1920, 1080), (508, 286)),
            (1016, 286)
        );
        // The monitor takes the same density.
        assert_eq!(
            scaled_mm((1920, 1080), (1920, 1080), (508, 286)),
            (508, 286)
        );
        // No physical size known: 96 DPI.
        assert_eq!(scaled_mm((960, 96), (1920, 1080), (0, 0)), (254, 25));
        assert_eq!(scaled_mm((0, 0), (1920, 1080), (508, 286)), (1, 1));
    }

    #[test]
    fn the_original_size_comes_back() {
        let original = (1920, 1080);
        assert_eq!(restore_size(original, &[r(0, 0, 1920, 1080)]), original);
        assert_eq!(restore_size(original, &[]), original);
        // A display added meanwhile keeps the screen large enough for it.
        assert_eq!(
            restore_size(original, &[r(0, 0, 1920, 1080), r(1920, 0, 1280, 1024)]),
            (3200, 1080)
        );
        assert_eq!(
            restore_size(original, &[r(0, 0, 1920, 1080), r(0, 1080, 1280, 1024)]),
            (1920, 2104)
        );
    }

    #[test]
    fn rects_intersect_when_they_overlap() {
        let a = r(0, 0, 100, 100);
        assert!(a.intersects(&r(99, 99, 10, 10)));
        assert!(!a.intersects(&r(100, 0, 10, 10)));
        assert!(!a.intersects(&r(0, 100, 10, 10)));
        assert!(a.intersects(&r(-5, -5, 10, 10)));
    }
}
