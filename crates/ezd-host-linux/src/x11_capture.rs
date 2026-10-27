//! X11 capture: a rectangle of the root window grabbed with MIT-SHM `GetImage`, paced by XDamage,
//! plus the cursor from XFixes.
//!
//! The capture thread owns its own connection. Frames are only grabbed when the rectangle was
//! damaged (the same idle behaviour as the Wayland capture, which gets no frames from a still
//! screen), at most once per refresh interval, and the first frame is grabbed at once. `GetImage`
//! does not contain the cursor: the pointer position (polled once per interval, X sends no event
//! for it) and the cursor image (XFixes `CursorNotify`) go to the [`CursorSink`], the tablet draws
//! its own local cursor.

use crate::cursor::{PixelOrder, build_shape};
use crate::frame::{Frame, NegotiatedFormat, Pixels, ShmBuf, SpaColor, spa_fmt};
use crate::pw::{Frames, Shared};
use crate::x11_output::{Rect, connect, xe};
use ezd_host_core::{CursorSink, now_us};
use ezd_proto::CursorState;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ReportLevel};
use x11rb::protocol::shm;
use x11rb::protocol::xfixes::{self, CursorNotifyMask};
use x11rb::protocol::xproto::{self, ImageFormat, VisualClass};
use x11rb::rust_connection::RustConnection;

/// How often the loop looks at the stop flag while nothing happens.
const POLL_MS: i32 = 100;
/// Shortest interval between grabs (and pointer polls), whatever refresh rate was asked for.
const MIN_INTERVAL: Duration = Duration::from_millis(4);

/// SPA format of the root window's pixels, `Err` for layouts the encoder cannot read.
pub fn spa_format_for(
    depth: u8,
    bits_per_pixel: u8,
    masks: (u32, u32, u32),
    msb_first: bool,
) -> Result<u32, String> {
    if !matches!(depth, 24 | 32) || bits_per_pixel != 32 {
        return Err(format!(
            "the X screen has depth {depth} at {bits_per_pixel} bits per pixel; 24 or 32 deep \
             32-bit pixels are required"
        ));
    }
    match (masks, msb_first) {
        ((0xff_0000, 0xff00, 0xff), false) => Ok(spa_fmt::BGRX),
        ((0xff_0000, 0xff00, 0xff), true) => Ok(spa_fmt::XRGB),
        ((0xff, 0xff00, 0xff_0000), false) => Ok(spa_fmt::RGBX),
        ((0xff, 0xff00, 0xff_0000), true) => Ok(spa_fmt::XBGR),
        (m, _) => Err(format!(
            "the X screen's colour masks {:#x}/{:#x}/{:#x} are not 8-bit RGB",
            m.0, m.1, m.2
        )),
    }
}

/// Bytes per row of an image of `width` pixels in a format with `scanline_pad` bits per row unit.
pub fn stride_for(width: u32, bits_per_pixel: u32, scanline_pad: u32) -> usize {
    let pad = scanline_pad.max(8);
    (((width * bits_per_pixel).div_ceil(pad) * pad) / 8) as usize
}

/// The pixel layout of the root window.
#[derive(Debug, Clone, Copy)]
struct RootFormat {
    spa: u32,
    bits_per_pixel: u8,
    scanline_pad: u8,
}

fn root_format(conn: &RustConnection, screen_num: usize) -> Result<RootFormat, String> {
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|d| d.visuals.iter())
        .find(|v| v.visual_id == screen.root_visual)
        .ok_or("the root visual is not listed")?;
    if visual.class != VisualClass::TRUE_COLOR && visual.class != VisualClass::DIRECT_COLOR {
        return Err(format!(
            "the root visual is {:?}; a TrueColor screen is required",
            visual.class
        ));
    }
    let format = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == screen.root_depth)
        .ok_or("no pixmap format for the root depth")?;
    let spa = spa_format_for(
        screen.root_depth,
        format.bits_per_pixel,
        (visual.red_mask, visual.green_mask, visual.blue_mask),
        setup.image_byte_order == xproto::ImageOrder::MSB_FIRST,
    )?;
    Ok(RootFormat {
        spa,
        bits_per_pixel: format.bits_per_pixel,
        scanline_pad: format.scanline_pad,
    })
}

/// A shared memory segment the server writes images into.
struct ShmSegment {
    seg: shm::Seg,
    ptr: *mut u8,
    len: usize,
}

impl ShmSegment {
    /// Creates a memfd of `len` bytes and attaches it to the server (MIT-SHM 1.2).
    fn attach(conn: &RustConnection, len: usize) -> Result<ShmSegment, String> {
        let version = shm::query_version(conn)
            .map_err(xe("ShmQueryVersion"))?
            .reply()
            .map_err(xe("ShmQueryVersion"))?;
        if (version.major_version, version.minor_version) < (1, 2) {
            return Err(format!(
                "MIT-SHM {}.{} cannot take file descriptors (1.2 is needed)",
                version.major_version, version.minor_version
            ));
        }
        // SAFETY: plain syscalls; the descriptor is owned right away and closed on every path.
        let fd = unsafe {
            let raw = libc::memfd_create(c"ezd-x11-capture".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
            }
            OwnedFd::from_raw_fd(raw)
        };
        // SAFETY: `fd` is a valid memfd.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(format!("ftruncate: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: maps `len` bytes of the memfd that was just sized to `len`.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        let seg = conn.generate_id().map_err(xe("generate_id"))?;
        let attached = shm::attach_fd(conn, seg, fd, false)
            .map_err(xe("ShmAttachFd"))
            .and_then(|c| c.check().map_err(xe("ShmAttachFd")));
        if let Err(e) = attached {
            // SAFETY: undoes the mapping above.
            unsafe { libc::munmap(ptr, len) };
            return Err(e);
        }
        Ok(ShmSegment {
            seg,
            ptr: ptr.cast(),
            len,
        })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping lives as long as `self`; the server only writes during a GetImage
        // whose reply has been received before this is read.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn detach(&self, conn: &RustConnection) {
        let _ = shm::detach(conn, self.seg);
        let _ = conn.flush();
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        // SAFETY: undoes the mapping made in `attach`.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// Takes images of the rectangle.
struct Grabber {
    root: xproto::Window,
    rect: (i16, i16, u16, u16),
    stride: usize,
    shm: Option<ShmSegment>,
}

impl Grabber {
    fn grab(&self, conn: &RustConnection) -> Result<Vec<u8>, String> {
        let (x, y, w, h) = self.rect;
        match &self.shm {
            Some(seg) => {
                shm::get_image(
                    conn,
                    self.root,
                    x,
                    y,
                    w,
                    h,
                    !0,
                    ImageFormat::Z_PIXMAP.into(),
                    seg.seg,
                    0,
                )
                .map_err(xe("ShmGetImage"))?
                .reply()
                .map_err(xe("ShmGetImage"))?;
                Ok(seg.bytes().to_vec())
            }
            None => {
                let r = xproto::get_image(conn, ImageFormat::Z_PIXMAP, self.root, x, y, w, h, !0)
                    .map_err(xe("GetImage"))?
                    .reply()
                    .map_err(xe("GetImage"))?;
                Ok(r.data)
            }
        }
    }
}

/// The cursor as the capture reports it to the sink.
struct CursorTrack {
    sink: CursorSink,
    shape_id: u32,
    last: Option<(i32, i32, bool, u32)>,
    updates: Arc<AtomicU64>,
}

impl CursorTrack {
    /// Reads the current cursor image and sends it when the sink does not have it yet.
    fn shape_changed(&mut self, conn: &RustConnection) {
        let Some(image) = xfixes::get_cursor_image(conn)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        // ARGB32 words, premultiplied; little-endian bytes are B G R A.
        let bytes: Vec<u8> = image
            .cursor_image
            .iter()
            .flat_map(|p| p.to_le_bytes())
            .collect();
        let (w, h) = (u32::from(image.width), u32::from(image.height));
        match build_shape(
            &bytes,
            w,
            h,
            w as usize * 4,
            PixelOrder::Bgra,
            i32::from(image.xhot),
            i32::from(image.yhot),
        ) {
            Some(shape) => {
                self.shape_id = shape.shape_id;
                let _ = self.sink.set_shape(shape);
            }
            None => self.shape_id = 0,
        }
    }

    /// Publishes the pointer position relative to `rect` when something changed.
    fn publish(&mut self, conn: &RustConnection, root: xproto::Window, rect: &Rect) {
        let Some(p) = xproto::query_pointer(conn, root)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        let (x, y) = (i32::from(p.root_x) - rect.x, i32::from(p.root_y) - rect.y);
        let inside = x >= 0 && y >= 0 && (x as u32) < rect.w && (y as u32) < rect.h;
        let cur = (x, y, inside && self.shape_id != 0, self.shape_id);
        if self.last == Some(cur) {
            return;
        }
        self.last = Some(cur);
        self.updates.fetch_add(1, Ordering::Relaxed);
        let _ = self.sink.update(CursorState {
            x: cur.0,
            y: cur.1,
            visible: cur.2,
            shape_id: cur.3,
            ts_us: now_us(),
        });
    }
}

/// Handle to the capture thread.
pub struct X11Capture {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    cursor_updates: Arc<AtomicU64>,
    shm: bool,
}

impl X11Capture {
    /// Connects to the X server and starts grabbing `rect` (`x, y, width, height`) at up to
    /// `refresh_mhz`. Returns once the capture is set up; the first frame follows.
    pub fn start(
        rect: (i32, i32, u32, u32),
        refresh_mhz: u32,
        cursor: CursorSink,
    ) -> Result<X11Capture, String> {
        // `EZD_X11_NO_SHM=1` forces plain `GetImage` (remote displays, debugging).
        let shm = std::env::var_os("EZD_X11_NO_SHM").is_none();
        Self::start_with(rect, refresh_mhz, cursor, shm)
    }

    /// [`start`](Self::start) with the shared-memory path chosen by the caller.
    pub fn start_with(
        rect: (i32, i32, u32, u32),
        refresh_mhz: u32,
        cursor: CursorSink,
        allow_shm: bool,
    ) -> Result<X11Capture, String> {
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let cursor_updates = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<bool, String>>();
        let rect = Rect {
            x: rect.0,
            y: rect.1,
            w: rect.2,
            h: rect.3,
        };
        let thread = {
            let (shared, stop, updates) = (shared.clone(), stop.clone(), cursor_updates.clone());
            std::thread::Builder::new()
                .name("ezd-x11-capture".into())
                .spawn(move || {
                    let params = Params {
                        rect,
                        refresh_mhz,
                        allow_shm,
                    };
                    let r = run(params, cursor, &shared, &stop, updates, &ready_tx);
                    if let Err(e) = r {
                        let _ = ready_tx.send(Err(e.clone()));
                        shared.lock().error.get_or_insert(e);
                    }
                    let mut s = shared.lock();
                    s.closed = true;
                    s.latest = None;
                    drop(s);
                    shared.notify();
                })
                .map_err(|e| format!("cannot spawn the X11 capture thread: {e}"))?
        };
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(shm)) => Ok(X11Capture {
                shared,
                stop,
                thread: Some(thread),
                cursor_updates,
                shm,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err("the X11 capture thread did not become ready".into())
            }
        }
    }

    /// Whether frames come through shared memory (else plain `GetImage`).
    pub fn uses_shm(&self) -> bool {
        self.shm
    }

    /// Waits until a frame size of exactly `want` is known (or `timeout`); returns the last
    /// known format either way.
    pub fn wait_format(&self, want: (u32, u32), timeout: Duration) -> Option<NegotiatedFormat> {
        self.shared.wait_format(want, timeout)
    }

    /// Waits for any format.
    pub fn wait_any_format(&self, timeout: Duration) -> Option<NegotiatedFormat> {
        self.shared.wait_any_format(timeout)
    }

    /// Takes the newest frame, waiting up to `timeout`. `Err` when the capture died.
    pub fn next(&self, timeout: Duration) -> Result<Option<Frame>, String> {
        self.shared.next(timeout)
    }

    /// Handle that takes frames from another thread.
    pub fn frames(&self) -> Frames {
        Frames(self.shared.clone())
    }

    /// Frames grabbed and frames replaced before the encoder took them.
    pub fn counters(&self) -> (u64, u64) {
        let s = self.shared.lock();
        (s.produced, s.coalesced)
    }

    /// How many cursor state changes were published to the sink.
    pub fn cursor_updates(&self) -> u64 {
        self.cursor_updates.load(Ordering::Relaxed)
    }

    /// Stops the thread.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
            tracing::info!(
                cursor_updates = self.cursor_updates(),
                "X11 capture stopped"
            );
        }
    }
}

impl Drop for X11Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Waits until the connection is readable or `timeout` passes.
fn wait_readable(conn: &RustConnection, timeout: Duration) {
    let mut fds = [libc::pollfd {
        fd: conn.stream().as_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    let ms = timeout.as_millis().min(POLL_MS as u128) as i32;
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(fds.as_mut_ptr(), 1, ms) };
}

/// Frame interval for `refresh_mhz`.
fn interval_for(refresh_mhz: u32) -> Duration {
    let mhz = if refresh_mhz == 0 {
        60_000
    } else {
        refresh_mhz
    };
    Duration::from_micros(1_000_000_000 / u64::from(mhz)).max(MIN_INTERVAL)
}

/// What the capture thread is asked to do.
struct Params {
    rect: Rect,
    refresh_mhz: u32,
    allow_shm: bool,
}

fn run(
    params: Params,
    cursor: CursorSink,
    shared: &Arc<Shared>,
    stop: &AtomicBool,
    cursor_updates: Arc<AtomicU64>,
    ready_tx: &mpsc::Sender<Result<bool, String>>,
) -> Result<(), String> {
    let Params {
        rect,
        refresh_mhz,
        allow_shm,
    } = params;
    let (conn, screen_num) = connect()?;
    let root = conn.setup().roots[screen_num].root;
    let geo = xproto::get_geometry(&conn, root)
        .map_err(xe("GetGeometry"))?
        .reply()
        .map_err(xe("GetGeometry"))?;
    if rect.x < 0
        || rect.y < 0
        || rect.right() > i32::from(geo.width)
        || rect.bottom() > i32::from(geo.height)
        || rect.w == 0
        || rect.h == 0
        || rect.w > u32::from(u16::MAX)
        || rect.h > u32::from(u16::MAX)
    {
        return Err(format!(
            "the capture area {}x{}+{}+{} is not inside the {}x{} X screen",
            rect.w, rect.h, rect.x, rect.y, geo.width, geo.height
        ));
    }
    let format = root_format(&conn, screen_num)?;
    let stride = stride_for(
        rect.w,
        u32::from(format.bits_per_pixel),
        u32::from(format.scanline_pad),
    );

    // Damage and the cursor need their versions announced first.
    damage::query_version(&conn, 1, 1)
        .map_err(|_| "the X server has no Damage extension".to_string())?
        .reply()
        .map_err(|_| "the X server has no Damage extension".to_string())?;
    xfixes::query_version(&conn, 5, 0)
        .map_err(|_| "the X server has no XFixes extension".to_string())?
        .reply()
        .map_err(|_| "the X server has no XFixes extension".to_string())?;
    let damage_id = conn.generate_id().map_err(xe("generate_id"))?;
    damage::create(&conn, damage_id, root, ReportLevel::NON_EMPTY)
        .map_err(xe("DamageCreate"))?
        .check()
        .map_err(xe("DamageCreate"))?;
    let region = conn.generate_id().map_err(xe("generate_id"))?;
    xfixes::create_region(&conn, region, &[])
        .map_err(xe("XFixesCreateRegion"))?
        .check()
        .map_err(xe("XFixesCreateRegion"))?;
    xfixes::select_cursor_input(&conn, root, CursorNotifyMask::DISPLAY_CURSOR)
        .map_err(xe("XFixesSelectCursorInput"))?
        .check()
        .map_err(xe("XFixesSelectCursorInput"))?;

    let segment = if allow_shm {
        ShmSegment::attach(&conn, stride * rect.h as usize)
            .map_err(|e| {
                tracing::warn!("X11: shared memory capture unavailable ({e}); using GetImage")
            })
            .ok()
    } else {
        None
    };
    let using_shm = segment.is_some();
    let grabber = Grabber {
        root,
        rect: (rect.x as i16, rect.y as i16, rect.w as u16, rect.h as u16),
        stride,
        shm: segment,
    };
    tracing::info!(
        width = rect.w,
        height = rect.h,
        x = rect.x,
        y = rect.y,
        shm = using_shm,
        "X11 capture"
    );

    {
        let fmt = NegotiatedFormat {
            width: rect.w,
            height: rect.h,
            spa_format: format.spa,
            modifier: None,
            color: SpaColor::default(),
        };
        let mut s = shared.lock();
        s.fmt = Some(fmt);
        s.fmt_version += 1;
        drop(s);
        shared.notify();
    }
    let _ = ready_tx.send(Ok(using_shm));

    let interval = interval_for(refresh_mhz);
    let mut track = CursorTrack {
        sink: cursor,
        shape_id: 0,
        last: None,
        updates: cursor_updates,
    };
    track.shape_changed(&conn);
    let mut seq = 0u64;
    // The first frame is grabbed at once; later ones when the rectangle is damaged.
    let mut dirty = true;
    let mut damage_pending = false;
    let mut cursor_shape_pending = false;
    let mut next_grab = Instant::now();
    let mut next_pointer = Instant::now();

    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        loop {
            match conn.poll_for_event() {
                Ok(Some(Event::DamageNotify(_))) => damage_pending = true,
                Ok(Some(Event::XfixesCursorNotify(_))) => cursor_shape_pending = true,
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => return Err(format!("the X connection failed: {e}")),
            }
        }
        if damage_pending {
            damage_pending = false;
            // Subtracting re-arms the damage and returns what was damaged since the last time.
            damage::subtract(&conn, damage_id, x11rb::NONE, region)
                .map_err(xe("DamageSubtract"))?;
            let parts = xfixes::fetch_region(&conn, region)
                .map_err(xe("XFixesFetchRegion"))?
                .reply()
                .map_err(xe("XFixesFetchRegion"))?;
            dirty |= parts.rectangles.iter().any(|r| {
                rect.intersects(&Rect {
                    x: i32::from(r.x),
                    y: i32::from(r.y),
                    w: u32::from(r.width),
                    h: u32::from(r.height),
                })
            });
        }
        if cursor_shape_pending {
            cursor_shape_pending = false;
            track.shape_changed(&conn);
            track.publish(&conn, root, &rect);
        }
        let now = Instant::now();
        if now >= next_pointer {
            track.publish(&conn, root, &rect);
            next_pointer = now + interval;
        }
        if dirty && now >= next_grab {
            dirty = false;
            let data = grabber.grab(&conn)?;
            next_grab = Instant::now() + interval;
            seq += 1;
            let frame = Frame {
                width: rect.w,
                height: rect.h,
                capture_us: now_us(),
                seq,
                pixels: Pixels::Shm(ShmBuf {
                    spa_format: format.spa,
                    stride: grabber.stride,
                    data,
                }),
            };
            let mut s = shared.lock();
            s.produced += 1;
            if s.latest.replace(frame).is_some() {
                s.coalesced += 1;
            }
            drop(s);
            shared.notify();
            continue;
        }
        let mut wake = next_pointer;
        if dirty {
            wake = wake.min(next_grab);
        }
        wait_readable(&conn, wake.saturating_duration_since(Instant::now()));
    };
    if let Some(seg) = &grabber.shm {
        seg.detach(&conn);
    }
    let _ = damage::destroy(&conn, damage_id);
    let _ = conn.flush();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_endian_bgr_screens_are_bgrx() {
        assert_eq!(
            spa_format_for(24, 32, (0xff0000, 0xff00, 0xff), false),
            Ok(spa_fmt::BGRX)
        );
        assert_eq!(
            spa_format_for(32, 32, (0xff0000, 0xff00, 0xff), false),
            Ok(spa_fmt::BGRX)
        );
        assert_eq!(
            spa_format_for(24, 32, (0xff0000, 0xff00, 0xff), true),
            Ok(spa_fmt::XRGB)
        );
        assert_eq!(
            spa_format_for(24, 32, (0xff, 0xff00, 0xff0000), false),
            Ok(spa_fmt::RGBX)
        );
        assert_eq!(
            spa_format_for(24, 32, (0xff, 0xff00, 0xff0000), true),
            Ok(spa_fmt::XBGR)
        );
    }

    #[test]
    fn other_layouts_are_refused() {
        assert!(spa_format_for(16, 16, (0xf800, 0x7e0, 0x1f), false).is_err());
        assert!(spa_format_for(24, 24, (0xff0000, 0xff00, 0xff), false).is_err());
        assert!(spa_format_for(30, 32, (0x3ff00000, 0xffc00, 0x3ff), false).is_err());
        assert!(spa_format_for(24, 32, (0xf80000, 0xfc00, 0xf8), false).is_err());
    }

    #[test]
    fn strides_round_up_to_the_scanline_pad() {
        assert_eq!(stride_for(1920, 32, 32), 7680);
        assert_eq!(stride_for(1, 32, 32), 4);
        assert_eq!(stride_for(3, 32, 64), 16);
        assert_eq!(stride_for(5, 8, 32), 8);
    }

    #[test]
    fn the_interval_follows_the_refresh_rate() {
        assert_eq!(interval_for(60_000), Duration::from_micros(16_666));
        assert_eq!(interval_for(0), interval_for(60_000));
        assert_eq!(interval_for(1_000_000), MIN_INTERVAL);
    }
}
