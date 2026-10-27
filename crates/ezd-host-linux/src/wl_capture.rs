//! Direct Wayland capture of one output, for the wlroots compositors (sway, Hyprland): no portal and
//! no PipeWire, the frames come from the compositor's own capture protocols over `$WAYLAND_DISPLAY`.
//!
//! Two protocols are supported, the first one advertised wins ([`choose_protocol`]):
//!
//! * `ext-image-copy-capture-v1` with `ext-output-image-capture-source-v1`: frames are captured
//!   without the cursor, and a pointer cursor session reports the cursor's position, hotspot and
//!   image, which go to the [`CursorSink`] (the tablet draws its own local cursor).
//! * `wlr-screencopy-unstable-v1` as the fallback: the cursor is painted into the frames
//!   (`overlay_cursor`), so no cursor data is sent and the tablet shows none of its own.
//!
//! Buffers are `wl_shm` (a memfd) only; DMA-BUF capture is not implemented. The capture thread
//! asks for the next frame as soon as one is delivered: both protocols answer when the output
//! changed, so the rate follows the compositor's damage up to its refresh rate. Frames are copied
//! out of the shared buffer into the one-slot mailbox of `pw.rs` (the newest frame wins), so the
//! encoder sees the same frame type as on the PipeWire path.

use crate::cursor::{PixelOrder, build_shape};
use crate::frame::{
    Frame, NegotiatedFormat, Pixels, ShmBuf, SpaColor, capture_us_from_pts, pixel_order, spa_fmt,
};
use crate::pw::{Frames, Shared};
use ezd_host_core::{CursorSink, now_us};
use ezd_proto::{CursorShape, CursorState};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

const EXT_COPY: &str = "ext_image_copy_capture_manager_v1";
const EXT_SOURCE: &str = "ext_output_image_capture_source_manager_v1";
const WLR_COPY: &str = "zwlr_screencopy_manager_v1";
/// Consecutive failed captures before the stream is given up.
const MAX_FAILURES: u32 = 20;
/// How long the output may take to show up in the registry after the compositor created it.
const OUTPUT_WAIT: Duration = Duration::from_secs(5);
/// How often the loop looks at the stop flag while the connection is idle.
const POLL_MS: i32 = 50;

/// The capture protocol in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `ext-image-copy-capture-v1`: no cursor in the frames, cursor session.
    ExtImageCopy,
    /// `wlr-screencopy-unstable-v1`: the cursor is painted into the frames.
    WlrScreencopy,
}

impl Protocol {
    /// Short name for logs.
    pub fn name(self) -> &'static str {
        match self {
            Protocol::ExtImageCopy => "ext-image-copy-capture-v1",
            Protocol::WlrScreencopy => "wlr-screencopy-unstable-v1",
        }
    }

    /// True if the compositor sends the cursor separately (the tablet can draw it locally).
    pub fn separate_cursor(self) -> bool {
        self == Protocol::ExtImageCopy
    }
}

/// Which capture globals the compositor advertises.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Advertised {
    pub ext_copy: bool,
    pub ext_output_source: bool,
    pub wlr_screencopy: bool,
}

impl Advertised {
    /// Reads the interface names of a registry.
    pub fn from_interfaces<'a>(names: impl IntoIterator<Item = &'a str>) -> Advertised {
        let mut a = Advertised::default();
        for n in names {
            match n {
                EXT_COPY => a.ext_copy = true,
                EXT_SOURCE => a.ext_output_source = true,
                WLR_COPY => a.wlr_screencopy = true,
                _ => {}
            }
        }
        a
    }
}

/// Picks the capture protocol: `forced` (`EZD_WL_CAPTURE`: `ext` or `wlr`) when given, else the
/// ext protocol (it needs both of its globals), else wlr-screencopy.
pub fn choose_protocol(adv: &Advertised, forced: Option<&str>) -> Result<Protocol, String> {
    let ext = adv.ext_copy && adv.ext_output_source;
    match forced.map(|f| f.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("auto") => {}
        Some("ext") => {
            return if ext {
                Ok(Protocol::ExtImageCopy)
            } else {
                Err("EZD_WL_CAPTURE=ext, but the compositor does not advertise \
                     ext-image-copy-capture-v1 with ext-output-image-capture-source-v1"
                    .into())
            };
        }
        Some("wlr") => {
            return if adv.wlr_screencopy {
                Ok(Protocol::WlrScreencopy)
            } else {
                Err("EZD_WL_CAPTURE=wlr, but the compositor does not advertise \
                     wlr-screencopy-unstable-v1"
                    .into())
            };
        }
        Some(other) => {
            return Err(format!(
                "EZD_WL_CAPTURE={other} is not valid (use ext, wlr or auto)"
            ));
        }
    }
    if ext {
        Ok(Protocol::ExtImageCopy)
    } else if adv.wlr_screencopy {
        Ok(Protocol::WlrScreencopy)
    } else {
        Err("the compositor offers no screen capture protocol \
             (ext-image-copy-capture-v1 or wlr-screencopy-unstable-v1)"
            .into())
    }
}

/// `wl_shm` formats the encoder takes: `(wl_shm code, SPA format)`. The codes are the DRM fourccs
/// except for the two legacy ones, and describe a little-endian word, so XRGB8888 is `B G R x` in
/// memory.
const SHM_FORMATS: [(u32, u32); 4] = [
    (1, spa_fmt::BGRX),           // XRGB8888
    (0, spa_fmt::BGRA),           // ARGB8888
    (0x3432_4258, spa_fmt::RGBX), // XBGR8888
    (0x3432_4241, spa_fmt::RGBA), // ABGR8888
];

/// The `wl_shm` format to use out of the ones the compositor offers, with its SPA id. The
/// capture prefers formats without alpha; the cursor image needs one with alpha.
pub fn choose_shm_format(offered: &[u32], want_alpha: bool) -> Option<(u32, u32)> {
    let order: [usize; 4] = if want_alpha {
        [1, 3, 0, 2]
    } else {
        [0, 2, 1, 3]
    };
    order
        .iter()
        .map(|&i| SHM_FORMATS[i])
        .find(|(code, _)| offered.contains(code))
}

fn wl_format(code: u32) -> Option<wl_shm::Format> {
    wl_shm::Format::try_from(code).ok()
}

/// Copies `rows` rows of `stride` bytes in reverse order (a `y_invert` screencopy frame).
pub fn flip_rows(data: &[u8], stride: usize, rows: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for y in (0..rows).rev() {
        out.extend_from_slice(&data[y * stride..(y + 1) * stride]);
    }
    out
}

/// A `presentation_time` / `ready` timestamp as CLOCK_MONOTONIC nanoseconds.
pub fn timestamp_ns(hi: u32, lo: u32, nsec: u32) -> i64 {
    let secs = (u64::from(hi) << 32) | u64::from(lo);
    (secs as i64)
        .saturating_mul(1_000_000_000)
        .saturating_add(i64::from(nsec))
}

fn mono_now_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    // `time_t` and `c_long` are `i64` on 64-bit Linux but narrower elsewhere.
    #[allow(clippy::useless_conversion)]
    {
        i64::from(ts.tv_sec) * 1_000_000_000 + i64::from(ts.tv_nsec)
    }
}

fn capture_time(pts: Option<(u32, u32, u32)>) -> u64 {
    let now = now_us();
    match pts {
        Some((hi, lo, ns)) => capture_us_from_pts(timestamp_ns(hi, lo, ns), mono_now_ns(), now),
        None => now,
    }
}

/// A `wl_shm` buffer over a memfd mapping.
struct ShmBuffer {
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    ptr: *mut u8,
    len: usize,
    width: u32,
    height: u32,
    stride: u32,
    wl_code: u32,
    spa: u32,
}

impl ShmBuffer {
    fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<State>,
        (width, height): (u32, u32),
        stride: u32,
        (wl_code, spa): (u32, u32),
    ) -> Result<ShmBuffer, String> {
        let format = wl_format(wl_code).ok_or("unknown wl_shm format")?;
        let len = stride as usize * height as usize;
        if len == 0 || len > i32::MAX as usize {
            return Err(format!("bad buffer size {width}x{height}"));
        }
        // SAFETY: plain libc calls; the fd is owned right after creation and the mapping is
        // unmapped in `drop`.
        let (fd, ptr) = unsafe {
            let raw = libc::memfd_create(c"ezd-wl-capture".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
            }
            let fd = OwnedFd::from_raw_fd(raw);
            if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) != 0 {
                return Err(format!("ftruncate: {}", std::io::Error::last_os_error()));
            }
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if ptr == libc::MAP_FAILED {
                return Err(format!("mmap: {}", std::io::Error::last_os_error()));
            }
            (fd, ptr as *mut u8)
        };
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format,
            qh,
            (),
        );
        Ok(ShmBuffer {
            pool,
            buffer,
            ptr,
            len,
            width,
            height,
            stride,
            wl_code,
            spa,
        })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping lives as long as `self`; the compositor only writes between the
        // capture request and `ready`/`failed`, and callers read after `ready`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn matches(&self, size: (u32, u32), stride: u32, code: u32) -> bool {
        (self.width, self.height) == size && self.stride == stride && self.wl_code == code
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        // SAFETY: `ptr` / `len` come from the successful mmap in `new`.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Main,
    Cursor,
}

/// One ext-image-copy-capture session (the output's, or the cursor's).
struct ExtSession {
    proxy: ExtImageCopyCaptureSessionV1,
    pending_size: Option<(u32, u32)>,
    pending_formats: Vec<u32>,
    size: Option<(u32, u32)>,
    formats: Vec<u32>,
    /// Counts `done` events (constraint batches).
    constraints_seq: u64,
    buffer: Option<ShmBuffer>,
    frame: Option<ExtImageCopyCaptureFrameV1>,
    /// `constraints_seq` when the frame was requested.
    frame_seq: u64,
    pts: Option<(u32, u32, u32)>,
    /// A frame failed on stale constraints: wait for the next batch before asking again.
    waiting: bool,
    failures: u32,
}

impl ExtSession {
    fn new(proxy: ExtImageCopyCaptureSessionV1) -> ExtSession {
        ExtSession {
            proxy,
            pending_size: None,
            pending_formats: Vec::new(),
            size: None,
            formats: Vec::new(),
            constraints_seq: 0,
            buffer: None,
            frame: None,
            frame_seq: 0,
            pts: None,
            waiting: false,
            failures: 0,
        }
    }

    /// Asks for the next frame when none is in flight and there is a buffer.
    fn capture(&mut self, qh: &QueueHandle<State>, role: Role) {
        if self.frame.is_some() || self.waiting {
            return;
        }
        let Some(buf) = &self.buffer else { return };
        let frame = self.proxy.create_frame(qh, role);
        frame.attach_buffer(&buf.buffer);
        frame.damage_buffer(0, 0, buf.width as i32, buf.height as i32);
        frame.capture();
        self.frame = Some(frame);
        self.frame_seq = self.constraints_seq;
        self.pts = None;
    }
}

/// The cursor as the pointer cursor session reports it.
#[derive(Default)]
struct CursorTrack {
    entered: bool,
    pos: (i32, i32),
    hotspot: (i32, i32),
    /// Last image: pixels (tightly as captured), size, stride, byte order.
    image: Option<(Vec<u8>, u32, u32, usize, PixelOrder)>,
    shape_id: u32,
    last: Option<(i32, i32, bool, u32)>,
}

struct OutputEntry {
    global: u32,
    output: wl_output::WlOutput,
    name: Option<String>,
}

struct WlrState {
    manager: ZwlrScreencopyManagerV1,
    output: wl_output::WlOutput,
    frame: Option<ZwlrScreencopyFrameV1>,
    /// `buffer` event of the frame in flight: format code, size, stride.
    info: Option<(u32, u32, u32, u32)>,
    y_invert: bool,
    buffer: Option<ShmBuffer>,
    failures: u32,
}

/// Everything the dispatch callbacks need; lives on the capture thread.
struct State {
    shared: Arc<Shared>,
    cursor: CursorSink,
    target: String,
    protocol: Protocol,
    outputs: Vec<OutputEntry>,
    target_removed: bool,
    shm: Option<wl_shm::WlShm>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    ext_manager: Option<ExtImageCopyCaptureManagerV1>,
    ext_source: Option<ExtImageCaptureSourceV1>,
    main: Option<ExtSession>,
    cursor_session: Option<ExtImageCopyCaptureCursorSessionV1>,
    cursor_capture: Option<ExtSession>,
    track: CursorTrack,
    wlr: Option<WlrState>,
    cursor_updates: Arc<AtomicU64>,
    seq: u64,
    error: Option<String>,
}

impl State {
    fn fail(&mut self, msg: impl Into<String>) {
        self.error.get_or_insert(msg.into());
    }

    fn session(&mut self, role: Role) -> Option<&mut ExtSession> {
        match role {
            Role::Main => self.main.as_mut(),
            Role::Cursor => self.cursor_capture.as_mut(),
        }
    }

    fn bind_output(
        &mut self,
        registry: &wl_registry::WlRegistry,
        global: u32,
        version: u32,
        qh: &QueueHandle<State>,
    ) {
        if self.outputs.iter().any(|o| o.global == global) {
            return;
        }
        // Version 4 adds the output's name, which is how the provider's output is found.
        let output = registry.bind::<wl_output::WlOutput, _, _>(global, version.min(4), qh, global);
        self.outputs.push(OutputEntry {
            global,
            output,
            name: None,
        });
    }

    fn target_output(&self) -> Option<&wl_output::WlOutput> {
        self.outputs
            .iter()
            .find(|o| o.name.as_deref() == Some(self.target.as_str()))
            .map(|o| &o.output)
    }

    fn publish(&mut self, mut frame: Frame) {
        self.seq += 1;
        frame.seq = self.seq;
        let mut s = self.shared.lock();
        s.produced += 1;
        if s.latest.replace(frame).is_some() {
            s.coalesced += 1;
        }
        drop(s);
        self.shared.notify();
    }

    fn set_format(&self, w: u32, h: u32, spa: u32) {
        let fmt = NegotiatedFormat {
            width: w,
            height: h,
            spa_format: spa,
            modifier: None,
            color: SpaColor::default(),
        };
        let mut s = self.shared.lock();
        if s.fmt != Some(fmt) {
            s.fmt = Some(fmt);
            s.fmt_version += 1;
        }
        drop(s);
        self.shared.notify();
    }

    // ----- ext-image-copy-capture -----

    /// A batch of constraints arrived: (re)allocate the buffer and ask for a frame.
    fn constraints_done(&mut self, role: Role, qh: &QueueHandle<State>) {
        let Some(shm) = self.shm.clone() else { return };
        let Some(s) = self.session(role) else { return };
        s.constraints_seq += 1;
        s.waiting = false;
        s.size = s.pending_size.or(s.size);
        s.formats = std::mem::take(&mut s.pending_formats);
        let size = s.size.filter(|&(w, h)| w > 0 && h > 0);
        let (Some(size), Some(fmt)) = (size, choose_shm_format(&s.formats, role == Role::Cursor))
        else {
            if role == Role::Cursor {
                // The cursor has no image right now (an empty size, no formats): nothing to
                // capture until the next batch.
                s.buffer = None;
                return;
            }
            let msg = format!(
                "no usable buffer constraints: {:?}, {:?}",
                s.size, s.formats
            );
            self.fail(msg);
            return;
        };
        let reuse = s
            .buffer
            .as_ref()
            .is_some_and(|b| b.matches(size, size.0 * 4, fmt.0));
        if !reuse {
            match ShmBuffer::new(&shm, qh, size, size.0 * 4, fmt) {
                Ok(b) => s.buffer = Some(b),
                Err(e) => {
                    self.fail(e);
                    return;
                }
            }
        }
        s.capture(qh, role);
        if role == Role::Main {
            self.set_format(size.0, size.1, fmt.1);
        }
    }

    fn frame_ready(&mut self, role: Role, qh: &QueueHandle<State>) {
        let Some(s) = self.session(role) else { return };
        if let Some(f) = s.frame.take() {
            f.destroy();
        }
        s.failures = 0;
        let pts = s.pts.take();
        let Some(buf) = &s.buffer else { return };
        match role {
            Role::Main => {
                let frame = Frame {
                    width: buf.width,
                    height: buf.height,
                    capture_us: capture_time(pts),
                    seq: 0,
                    pixels: Pixels::Shm(ShmBuf {
                        spa_format: buf.spa,
                        stride: buf.stride as usize,
                        data: buf.bytes().to_vec(),
                    }),
                };
                s.capture(qh, role);
                self.publish(frame);
            }
            Role::Cursor => {
                let image = pixel_order(buf.spa).map(|order| {
                    (
                        buf.bytes().to_vec(),
                        buf.width,
                        buf.height,
                        buf.stride as usize,
                        order,
                    )
                });
                s.capture(qh, role);
                if image.is_some() {
                    self.track.image = image;
                }
                self.cursor_image_changed();
            }
        }
    }

    fn frame_failed(&mut self, role: Role, reason: WEnum<FailureReason>, qh: &QueueHandle<State>) {
        let Some(s) = self.session(role) else { return };
        let frame_seq = s.frame_seq;
        if let Some(f) = s.frame.take() {
            f.destroy();
        }
        match reason {
            WEnum::Value(FailureReason::Stopped) => {
                if role == Role::Main {
                    self.fail("the compositor stopped the capture session (output removed?)");
                }
            }
            WEnum::Value(FailureReason::BufferConstraints) => {
                if s.constraints_seq > frame_seq {
                    s.capture(qh, role);
                } else {
                    s.waiting = true;
                }
            }
            other => {
                s.failures += 1;
                if s.failures > MAX_FAILURES {
                    let msg = format!("capture failed {} times in a row ({other:?})", s.failures);
                    if role == Role::Main {
                        self.fail(msg);
                    }
                } else {
                    s.capture(qh, role);
                }
            }
        }
    }

    /// Builds the wire shape from the last cursor image and the current hotspot.
    fn cursor_shape(&self) -> Option<CursorShape> {
        let (data, w, h, stride, order) = self.track.image.as_ref()?;
        build_shape(
            data,
            *w,
            *h,
            *stride,
            *order,
            self.track.hotspot.0,
            self.track.hotspot.1,
        )
    }

    fn cursor_image_changed(&mut self) {
        match self.cursor_shape() {
            Some(shape) => {
                self.track.shape_id = shape.shape_id;
                let _ = self.cursor.set_shape(shape);
            }
            None => self.track.shape_id = 0,
        }
        self.publish_cursor();
    }

    fn publish_cursor(&mut self) {
        let t = &self.track;
        let cur = (t.pos.0, t.pos.1, t.entered && t.shape_id != 0, t.shape_id);
        if t.last == Some(cur) {
            return;
        }
        self.track.last = Some(cur);
        self.cursor_updates.fetch_add(1, Ordering::Relaxed);
        let _ = self.cursor.update(CursorState {
            x: cur.0,
            y: cur.1,
            visible: cur.2,
            shape_id: cur.3,
            ts_us: now_us(),
        });
    }

    /// Creates the cursor session once the seat has a pointer (and the output source exists).
    fn try_cursor_session(&mut self, qh: &QueueHandle<State>) {
        if self.protocol != Protocol::ExtImageCopy || self.cursor_session.is_some() {
            return;
        }
        let (Some(seat), Some(manager), Some(source)) =
            (&self.seat, &self.ext_manager, &self.ext_source)
        else {
            return;
        };
        let Some(pointer) = self.pointer.clone().or_else(|| {
            let p = seat.get_pointer(qh, ());
            Some(p)
        }) else {
            return;
        };
        let session = manager.create_pointer_cursor_session(source, &pointer, qh, ());
        let capture = session.get_capture_session(qh, Role::Cursor);
        tracing::info!("cursor session created");
        self.pointer = Some(pointer);
        self.cursor_session = Some(session);
        self.cursor_capture = Some(ExtSession::new(capture));
    }

    // ----- wlr-screencopy -----

    fn wlr_capture(&mut self, qh: &QueueHandle<State>) {
        if let Some(w) = self.wlr.as_mut() {
            // overlay_cursor = 1: the cursor is part of the frame.
            w.frame = Some(w.manager.capture_output(1, &w.output, qh, ()));
            w.info = None;
            w.y_invert = false;
        }
    }

    fn wlr_buffer_ready(&mut self, qh: &QueueHandle<State>) {
        let Some(shm) = self.shm.clone() else { return };
        let Some(w) = self.wlr.as_mut() else { return };
        let (Some((code, width, height, stride)), Some(frame)) = (w.info, w.frame.as_ref()) else {
            return;
        };
        let Some(fmt) = SHM_FORMATS.iter().find(|(c, _)| *c == code).copied() else {
            let msg = format!("screencopy offers only unsupported buffer format {code:#x}");
            self.fail(msg);
            return;
        };
        if !w
            .buffer
            .as_ref()
            .is_some_and(|b| b.matches((width, height), stride, code))
        {
            match ShmBuffer::new(&shm, qh, (width, height), stride, fmt) {
                Ok(b) => w.buffer = Some(b),
                Err(e) => {
                    self.fail(e);
                    return;
                }
            }
        }
        if let Some(b) = &w.buffer {
            if frame.version() >= 2 {
                frame.copy_with_damage(&b.buffer);
            } else {
                frame.copy(&b.buffer);
            }
            let (bw, bh, spa) = (b.width, b.height, b.spa);
            self.set_format(bw, bh, spa);
        }
    }

    fn wlr_ready(&mut self, ts: (u32, u32, u32), qh: &QueueHandle<State>) {
        let Some(w) = self.wlr.as_mut() else { return };
        if let Some(f) = w.frame.take() {
            f.destroy();
        }
        w.failures = 0;
        let (y_invert, Some(buf)) = (w.y_invert, w.buffer.as_ref()) else {
            return;
        };
        let raw = buf.bytes();
        let data = if y_invert {
            flip_rows(raw, buf.stride as usize, buf.height as usize)
        } else {
            raw.to_vec()
        };
        let frame = Frame {
            width: buf.width,
            height: buf.height,
            capture_us: capture_time(Some(ts)),
            seq: 0,
            pixels: Pixels::Shm(ShmBuf {
                spa_format: buf.spa,
                stride: buf.stride as usize,
                data,
            }),
        };
        self.wlr_capture(qh);
        self.publish(frame);
    }

    fn wlr_failed(&mut self, qh: &QueueHandle<State>) {
        let Some(w) = self.wlr.as_mut() else { return };
        if let Some(f) = w.frame.take() {
            f.destroy();
        }
        w.failures += 1;
        if w.failures > MAX_FAILURES {
            self.fail("screencopy failed repeatedly (output removed?)");
        } else {
            self.wlr_capture(qh);
        }
    }
}

// ----- dispatch -----

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => state.bind_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.outputs.iter().position(|o| o.global == name) {
                    let gone = state.outputs.remove(i);
                    if gone.name.as_deref() == Some(state.target.as_str()) {
                        state.target_removed = true;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(o) = state.outputs.iter_mut().find(|o| o.global == *global)
        {
            o.name = Some(name);
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Pointer)
        {
            state.try_cursor_session(qh);
        }
    }
}

delegate_noop!(State: ignore wl_pointer::WlPointer);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ExtImageCopyCaptureManagerV1);
delegate_noop!(State: ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(State: ExtImageCaptureSourceV1);
delegate_noop!(State: ZwlrScreencopyManagerV1);

impl Dispatch<ExtImageCopyCaptureSessionV1, Role> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        role: &Role,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        match event {
            Event::BufferSize { width, height } => {
                if let Some(s) = state.session(*role) {
                    s.pending_size = Some((width, height));
                }
            }
            Event::ShmFormat {
                format: WEnum::Value(f),
            } => {
                if let Some(s) = state.session(*role) {
                    s.pending_formats.push(u32::from(f));
                }
            }
            Event::ShmFormat {
                format: WEnum::Unknown(f),
            } => {
                if let Some(s) = state.session(*role) {
                    s.pending_formats.push(f);
                }
            }
            Event::Done => state.constraints_done(*role, qh),
            Event::Stopped => match role {
                Role::Main => state.fail("the compositor stopped the capture session"),
                Role::Cursor => tracing::debug!("the cursor capture session stopped"),
            },
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, Role> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        role: &Role,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        match event {
            Event::PresentationTime {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                if let Some(s) = state.session(*role) {
                    s.pts = Some((tv_sec_hi, tv_sec_lo, tv_nsec));
                }
            }
            Event::Ready => state.frame_ready(*role, qh),
            Event::Failed { reason } => state.frame_failed(*role, reason, qh),
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureCursorSessionV1,
        event: ext_image_copy_capture_cursor_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_cursor_session_v1::Event;
        match event {
            Event::Enter => {
                state.track.entered = true;
                state.publish_cursor();
            }
            Event::Leave => {
                state.track.entered = false;
                state.publish_cursor();
            }
            Event::Position { x, y } => {
                state.track.pos = (x, y);
                state.publish_cursor();
            }
            Event::Hotspot { x, y } if state.track.hotspot != (x, y) => {
                state.track.hotspot = (x, y);
                state.cursor_image_changed();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let code = match format {
                    WEnum::Value(f) => u32::from(f),
                    WEnum::Unknown(f) => f,
                };
                if let Some(w) = state.wlr.as_mut() {
                    // Several `buffer` events can come; keep the first one we can use.
                    let usable = SHM_FORMATS.iter().any(|(c, _)| *c == code);
                    if usable && w.info.is_none() {
                        w.info = Some((code, width, height, stride));
                    }
                    // Before version 3 there is no `buffer_done`: answer right away.
                    if usable && w.frame.as_ref().is_some_and(|f| f.version() < 3) {
                        state.wlr_buffer_ready(qh);
                    }
                }
            }
            Event::BufferDone => state.wlr_buffer_ready(qh),
            Event::Flags { flags } => {
                if let Some(w) = state.wlr.as_mut() {
                    w.y_invert = matches!(flags, WEnum::Value(f) if f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
                }
            }
            Event::Ready {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => state.wlr_ready((tv_sec_hi, tv_sec_lo, tv_nsec), qh),
            Event::Failed => state.wlr_failed(qh),
            _ => {}
        }
    }
}

/// Lists the globals of the compositor behind `$WAYLAND_DISPLAY` as `(interface, version)`.
pub fn advertised_globals() -> Result<Vec<(String, u32)>, String> {
    struct Probe;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
        fn event(
            _: &mut Self,
            _: &wl_registry::WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    let conn =
        Connection::connect_to_env().map_err(|e| format!("cannot connect to Wayland: {e}"))?;
    let (globals, _queue) =
        registry_queue_init::<Probe>(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
    Ok(globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|g| (g.interface, g.version))
        .collect())
}

/// Capture-related part of `ezd-host diag`: the protocols the compositor offers and the choice.
pub fn diag() -> serde_json::Value {
    match advertised_globals() {
        Ok(globals) => {
            let adv = Advertised::from_interfaces(globals.iter().map(|(n, _)| n.as_str()));
            let forced = std::env::var("EZD_WL_CAPTURE").ok();
            let chosen = choose_protocol(&adv, forced.as_deref());
            let version = |name: &str| globals.iter().find(|(n, _)| n == name).map(|g| g.1);
            serde_json::json!({
                "ext_image_copy_capture": version(EXT_COPY),
                "ext_output_image_capture_source": version(EXT_SOURCE),
                "wlr_screencopy": version(WLR_COPY),
                "wl_shm": version("wl_shm"),
                "zwp_linux_dmabuf": version("zwp_linux_dmabuf_v1"),
                "chosen": chosen.as_ref().ok().map(|p| p.name()),
                "separate_cursor": chosen.as_ref().is_ok_and(|p| p.separate_cursor()),
                "error": chosen.err(),
            })
        }
        Err(e) => serde_json::json!({ "error": e }),
    }
}

/// Handle to the capture thread.
pub struct WlCapture {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    protocol: Protocol,
    cursor_updates: Arc<AtomicU64>,
}

impl WlCapture {
    /// Connects to the compositor and starts capturing the output called `output`. Returns once
    /// the capture objects exist; the first frame follows.
    pub fn start(output: &str, cursor: CursorSink) -> Result<WlCapture, String> {
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let cursor_updates = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Protocol, String>>();
        let thread = {
            let (shared, stop, updates) = (shared.clone(), stop.clone(), cursor_updates.clone());
            let target = output.to_string();
            std::thread::Builder::new()
                .name("ezd-wl-capture".into())
                .spawn(move || {
                    let r = run(&target, cursor, &shared, &stop, updates, &ready_tx);
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
                .map_err(|e| format!("cannot spawn the Wayland capture thread: {e}"))?
        };
        match ready_rx.recv_timeout(OUTPUT_WAIT + Duration::from_secs(5)) {
            Ok(Ok(protocol)) => Ok(WlCapture {
                shared,
                stop,
                thread: Some(thread),
                protocol,
                cursor_updates,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err("the Wayland capture thread did not become ready".into())
            }
        }
    }

    /// The protocol in use.
    pub fn protocol(&self) -> Protocol {
        self.protocol
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

    /// Frames delivered by the compositor and frames replaced before the encoder took them.
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
                protocol = self.protocol.name(),
                cursor_updates = self.cursor_updates(),
                "Wayland capture stopped"
            );
        }
    }
}

impl Drop for WlCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Waits for the connection to become readable (up to `POLL_MS`).
fn wait_readable(fd: std::os::fd::BorrowedFd<'_>) -> bool {
    let mut p = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    let n = unsafe { libc::poll(&mut p, 1, POLL_MS) };
    n > 0 && p.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0
}

/// One pass of the event loop: dispatch, flush, wait for the socket, read.
fn pump(queue: &mut wayland_client::EventQueue<State>, state: &mut State) -> Result<(), String> {
    queue
        .dispatch_pending(state)
        .map_err(|e| format!("Wayland dispatch: {e}"))?;
    queue.flush().map_err(|e| format!("Wayland flush: {e}"))?;
    if let Some(guard) = queue.prepare_read() {
        if wait_readable(guard.connection_fd()) {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("Wayland read: {e}")),
            }
        }
        queue
            .dispatch_pending(state)
            .map_err(|e| format!("Wayland dispatch: {e}"))?;
    }
    Ok(())
}

fn run(
    target: &str,
    cursor: CursorSink,
    shared: &Arc<Shared>,
    stop: &AtomicBool,
    cursor_updates: Arc<AtomicU64>,
    ready_tx: &mpsc::Sender<Result<Protocol, String>>,
) -> Result<(), String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to the Wayland compositor: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
    let qh = queue.handle();
    let list = globals.contents().clone_list();
    let adv = Advertised::from_interfaces(list.iter().map(|g| g.interface.as_str()));
    let forced = std::env::var("EZD_WL_CAPTURE").ok();
    let protocol = choose_protocol(&adv, forced.as_deref())?;
    tracing::info!(
        protocol = protocol.name(),
        output = target,
        "Wayland capture"
    );

    let mut state = State {
        shared: shared.clone(),
        cursor,
        target: target.to_string(),
        protocol,
        outputs: Vec::new(),
        target_removed: false,
        shm: None,
        seat: None,
        pointer: None,
        ext_manager: None,
        ext_source: None,
        main: None,
        cursor_session: None,
        cursor_capture: None,
        track: CursorTrack::default(),
        wlr: None,
        cursor_updates,
        seq: 0,
        error: None,
    };
    let registry = globals.registry();
    for g in list.iter().filter(|g| g.interface == "wl_output") {
        state.bind_output(registry, g.name, g.version, &qh);
    }
    state.shm = Some(
        globals
            .bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ())
            .map_err(|e| format!("wl_shm: {e}"))?,
    );

    // The compositor created the output a moment ago: wait for its registry entry and name.
    let deadline = Instant::now() + OUTPUT_WAIT;
    while state.target_output().is_none() {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let names: Vec<_> = state
                .outputs
                .iter()
                .filter_map(|o| o.name.clone())
                .collect();
            let hint = if state.outputs.iter().any(|o| o.name.is_none()) {
                " (the compositor's wl_output is older than version 4 and has no names)"
            } else {
                ""
            };
            return Err(format!(
                "the compositor has no output called {target} (outputs: {}){hint}",
                names.join(", ")
            ));
        }
        pump(&mut queue, &mut state)?;
    }
    let output = state.target_output().cloned().ok_or("output vanished")?;

    match protocol {
        Protocol::ExtImageCopy => {
            let manager = globals
                .bind::<ExtImageCopyCaptureManagerV1, _, _>(&qh, 1..=1, ())
                .map_err(|e| format!("{EXT_COPY}: {e}"))?;
            let sources = globals
                .bind::<ExtOutputImageCaptureSourceManagerV1, _, _>(&qh, 1..=1, ())
                .map_err(|e| format!("{EXT_SOURCE}: {e}"))?;
            let source = sources.create_source(&output, &qh, ());
            // Options 0: no cursor in the frames; the cursor session reports it.
            let session = manager.create_session(&source, Options::empty(), &qh, Role::Main);
            state.main = Some(ExtSession::new(session));
            state.ext_manager = Some(manager);
            state.ext_source = Some(source);
            // The seat may already offer a pointer; later capability changes are handled in
            // `Dispatch<WlSeat>`.
            state.seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=5, ()).ok();
        }
        Protocol::WlrScreencopy => {
            let manager = globals
                .bind::<ZwlrScreencopyManagerV1, _, _>(&qh, 1..=3, ())
                .map_err(|e| format!("{WLR_COPY}: {e}"))?;
            state.wlr = Some(WlrState {
                manager,
                output,
                frame: None,
                info: None,
                y_invert: false,
                buffer: None,
                failures: 0,
            });
            state.wlr_capture(&qh);
            tracing::info!("screencopy paints the cursor into the frames; no cursor data is sent");
        }
    }
    let _ = ready_tx.send(Ok(protocol));

    while !stop.load(Ordering::Relaxed) {
        pump(&mut queue, &mut state)?;
        if let Some(e) = state.error.take() {
            return Err(e);
        }
        if state.target_removed {
            return Err(format!("the output {target} was removed"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adv(ext_copy: bool, ext_output_source: bool, wlr_screencopy: bool) -> Advertised {
        Advertised {
            ext_copy,
            ext_output_source,
            wlr_screencopy,
        }
    }

    #[test]
    fn advertised_from_registry_names() {
        let a = Advertised::from_interfaces([
            "wl_shm",
            "zwlr_screencopy_manager_v1",
            "ext_image_copy_capture_manager_v1",
            "ext_output_image_capture_source_manager_v1",
        ]);
        assert_eq!(a, adv(true, true, true));
        assert_eq!(
            Advertised::from_interfaces(["wl_shm"]),
            adv(false, false, false)
        );
    }

    #[test]
    fn ext_is_preferred_when_it_is_complete() {
        assert_eq!(
            choose_protocol(&adv(true, true, true), None),
            Ok(Protocol::ExtImageCopy)
        );
        // The copy protocol alone cannot name an output.
        assert_eq!(
            choose_protocol(&adv(true, false, true), None),
            Ok(Protocol::WlrScreencopy)
        );
        assert_eq!(
            choose_protocol(&adv(false, false, true), Some("auto")),
            Ok(Protocol::WlrScreencopy)
        );
        assert!(choose_protocol(&adv(false, false, false), None).is_err());
    }

    #[test]
    fn the_override_can_pick_either_protocol() {
        let both = adv(true, true, true);
        assert_eq!(
            choose_protocol(&both, Some("wlr")),
            Ok(Protocol::WlrScreencopy)
        );
        assert_eq!(
            choose_protocol(&both, Some(" EXT ")),
            Ok(Protocol::ExtImageCopy)
        );
        assert!(choose_protocol(&adv(false, false, true), Some("ext")).is_err());
        assert!(choose_protocol(&adv(true, true, false), Some("wlr")).is_err());
        assert!(
            choose_protocol(&both, Some("pipewire"))
                .unwrap_err()
                .contains("pipewire")
        );
    }

    #[test]
    fn only_the_ext_protocol_has_a_separate_cursor() {
        assert!(Protocol::ExtImageCopy.separate_cursor());
        assert!(!Protocol::WlrScreencopy.separate_cursor());
    }

    #[test]
    fn shm_format_preference() {
        // Capture: opaque first.
        assert_eq!(choose_shm_format(&[0, 1], false), Some((1, spa_fmt::BGRX)));
        assert_eq!(
            choose_shm_format(&[0, 0x3432_4241], false),
            Some((0, spa_fmt::BGRA))
        );
        assert_eq!(
            choose_shm_format(&[0x3432_4258], false),
            Some((0x3432_4258, spa_fmt::RGBX))
        );
        // Cursor: alpha first.
        assert_eq!(choose_shm_format(&[0, 1], true), Some((0, spa_fmt::BGRA)));
        assert_eq!(choose_shm_format(&[1], true), Some((1, spa_fmt::BGRX)));
        // 10-bit only: nothing the encoder path takes.
        assert_eq!(choose_shm_format(&[0x3033_5258], false), None);
        assert_eq!(choose_shm_format(&[], false), None);
    }

    #[test]
    fn shm_codes_match_the_binding_and_the_spa_table() {
        for (code, spa) in SHM_FORMATS {
            assert!(wl_format(code).is_some(), "{code:#x}");
            assert!(crate::frame::drm_fourcc(spa).is_some());
            assert!(pixel_order(spa).is_some());
        }
        assert_eq!(wl_format(1), Some(wl_shm::Format::Xrgb8888));
        assert_eq!(wl_format(0x3432_4258), Some(wl_shm::Format::Xbgr8888));
        // DRM fourcc of the SPA format equals the wl_shm code (except the two legacy codes).
        assert_eq!(crate::frame::drm_fourcc(spa_fmt::RGBX), Some(0x3432_4258));
    }

    #[test]
    fn rows_flip() {
        let data = [1u8, 1, 2, 2, 3, 3];
        assert_eq!(flip_rows(&data, 2, 3), [3, 3, 2, 2, 1, 1]);
        assert_eq!(flip_rows(&[], 4, 0), Vec::<u8>::new());
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp_ns(0, 5, 7), 5_000_000_007);
        assert_eq!(timestamp_ns(1, 0, 0), 4_294_967_296_000_000_000);
        // A missing or implausible stamp falls back to now.
        let before = now_us();
        assert!(capture_time(None) >= before);
        assert!(capture_time(Some((0, 0, 0))) >= before);
    }

    /// Creates an output in a headless sway, captures it and checks size, content and (with the
    /// ext protocol) the cursor session. Run it with `tools/wlroots-headless-test.sh`; set
    /// `EZD_WL_CAPTURE=wlr` to take the screencopy path.
    #[test]
    fn headless_sway_frames() {
        use crate::portal::SourceKind;
        use crate::provider::{FrameSource, OutputRequest, VirtualOutputProvider};
        use crate::sway::{SwayIpc, SwayProvider};
        if !crate::wlroots_test::enabled() {
            return;
        }
        let req = OutputRequest {
            kind: SourceKind::Virtual,
            output_name: None,
            width: 1280,
            height: 720,
            refresh_mhz: 60_000,
        };
        let opened = SwayProvider.open(&req).expect("open");
        let info = opened.info.clone().expect("output info");
        eprintln!("output: {info:?}");
        assert_eq!((info.mode.width, info.mode.height), (1280, 720));
        let FrameSource::Wayland { output } = &opened.frames else {
            panic!("not a Wayland frame source");
        };
        let _driver = crate::wlroots_test::Driver::start(output);
        let mut cap = WlCapture::start(output, CursorSink::null()).expect("capture");
        eprintln!("protocol: {}", cap.protocol().name());
        let fmt = cap
            .wait_format((1280, 720), Duration::from_secs(5))
            .expect("format");
        assert_eq!((fmt.width, fmt.height), (1280, 720));
        let deadline = Instant::now() + Duration::from_secs(20);
        let (mut frames, mut colours) = (0, std::collections::HashSet::new());
        while frames < 10 && Instant::now() < deadline {
            let Some(f) = cap.next(Duration::from_millis(500)).expect("frame") else {
                continue;
            };
            assert_eq!((f.width, f.height), (1280, 720));
            let Pixels::Shm(shm) = &f.pixels else {
                panic!("expected a shared-memory frame");
            };
            assert!(shm.data.len() >= shm.stride * 719 + 1280 * 4);
            let at = 360 * shm.stride + 640 * 4;
            let order = pixel_order(shm.spa_format).expect("byte order");
            let px = &shm.data[at..at + 4];
            // Red or blue, whatever the byte order.
            let (r, b) = match order {
                PixelOrder::Bgra | PixelOrder::Bgrx => (px[2], px[0]),
                _ => (px[0], px[2]),
            };
            colours.insert((r, b));
            frames += 1;
        }
        eprintln!("frames: {frames}, centre colours (r, b): {colours:?}");
        assert!(frames >= 3, "got {frames} frames");
        assert!(
            colours.iter().any(|&(r, b)| r > 200 && b < 50)
                || colours.iter().any(|&(r, b)| b > 200 && r < 50),
            "frames do not show the output background: {colours:?}"
        );
        if cap.protocol() == Protocol::ExtImageCopy {
            let deadline = Instant::now() + Duration::from_secs(10);
            while cap.cursor_updates() == 0 && Instant::now() < deadline {
                let _ = cap.next(Duration::from_millis(100));
            }
            eprintln!("cursor updates: {}", cap.cursor_updates());
            assert!(cap.cursor_updates() > 0, "no cursor session data arrived");
        } else {
            assert_eq!(cap.cursor_updates(), 0);
        }
        cap.stop();
        let name = info.name.clone();
        drop(opened);
        let after = SwayIpc::connect().unwrap().outputs().unwrap();
        assert!(
            !after.iter().any(|o| o.name == name),
            "{name} is still there: {after:?}"
        );
    }
}
