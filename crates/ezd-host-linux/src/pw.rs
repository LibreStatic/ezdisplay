//! PipeWire capture thread: format negotiation (DMA-BUF first, shared memory as fallback), buffer
//! handling, cursor metadata.
//!
//! The PipeWire main loop runs on its own thread. The newest frame is parked in a one-slot mailbox
//! ([`PwCapture::next`] takes it); older frames are requeued immediately. A DMA-BUF frame keeps its
//! PipeWire buffer checked out until the encoder is done with it (dropping the [`Frame`]).
//! Cursor metadata is forwarded to the [`CursorSink`] from this thread, independent of video.

use crate::cursor_meta::{CursorTracker, parse_cursor_meta};
use crate::frame::{
    DRM_MOD_LINEAR, DmaBuf, Frame, NegotiatedFormat, OFFERED_FORMATS, Pixels, Plane, Release,
    ShmBuf, capture_us_from_pts, choose_modifier,
};
use crate::pod::{self, Val, VideoFormatProps};
use ezd_host_core::{CursorSink, now_us};
use ezd_proto::CursorState;
use pipewire as pw;
use pw::properties::properties;
use pw::spa::pod::Pod;
use pw::spa::sys as spa_sys;
use std::cell::RefCell;
use std::collections::HashSet;
use std::os::fd::{FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Largest cursor bitmap edge we ask the compositor to be able to attach.
const CURSOR_MAX_EDGE: usize = 256;

/// What to connect to.
#[derive(Debug, Clone)]
pub enum Target {
    /// A node id (from the portal).
    Node(u32),
    /// `target.object` (node name or serial), for test harnesses.
    #[allow(dead_code)]
    Name(String),
}

/// Stream parameters.
#[derive(Debug, Clone)]
pub struct PwParams {
    /// Wanted width.
    pub width: u32,
    /// Wanted height.
    pub height: u32,
    /// Wanted maximum frame rate.
    pub fps: u32,
    /// Modifiers we can import; `None` disables DMA-BUF.
    pub dmabuf_modifiers: Option<Vec<u64>>,
}

enum Cmd {
    Release { ptr: usize, generation: u64 },
    ForceShm,
    Quit,
}

#[derive(Default)]
struct State {
    latest: Option<Frame>,
    fmt: Option<NegotiatedFormat>,
    fmt_version: u64,
    error: Option<String>,
    closed: bool,
    produced: u64,
    coalesced: u64,
}

struct Shared {
    m: Mutex<State>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.m.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Handle to the capture thread.
pub struct PwCapture {
    shared: Arc<Shared>,
    tx: pw::channel::Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PwCapture {
    /// Starts the PipeWire thread and connects the stream.
    pub fn start(
        fd: Option<OwnedFd>,
        target: Target,
        params: PwParams,
        cursor: CursorSink,
    ) -> Result<PwCapture, String> {
        let shared = Arc::new(Shared {
            m: Mutex::new(State::default()),
            cv: Condvar::new(),
        });
        let (tx, rx) = pw::channel::channel::<Cmd>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread = {
            let (shared, tx) = (shared.clone(), tx.clone());
            std::thread::Builder::new()
                .name("ezd-pipewire".into())
                .spawn(move || {
                    let r = run(
                        fd,
                        target,
                        params,
                        cursor,
                        shared.clone(),
                        tx,
                        rx,
                        &ready_tx,
                    );
                    if let Err(e) = r {
                        let _ = ready_tx.send(Err(e.clone()));
                        shared.lock().error.get_or_insert(e);
                    }
                    let mut s = shared.lock();
                    s.closed = true;
                    s.latest = None;
                    drop(s);
                    shared.cv.notify_all();
                })
                .map_err(|e| format!("cannot spawn pipewire thread: {e}"))?
        };
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(PwCapture {
                shared,
                tx,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = tx.send(Cmd::Quit);
                Err("pipewire thread did not become ready".into())
            }
        }
    }

    /// Currently negotiated format.
    #[allow(dead_code)]
    pub fn format(&self) -> Option<NegotiatedFormat> {
        self.shared.lock().fmt
    }

    /// Waits until a format with exactly `want` size is negotiated (or `timeout`); returns the
    /// last known format either way.
    pub fn wait_format(&self, want: (u32, u32), timeout: Duration) -> Option<NegotiatedFormat> {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.lock();
        loop {
            if let Some(f) = s.fmt
                && (f.width, f.height) == want
            {
                return Some(f);
            }
            if s.closed || s.error.is_some() {
                return s.fmt;
            }
            let now = Instant::now();
            if now >= deadline {
                return s.fmt;
            }
            s = self
                .shared
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Waits for any format to be negotiated.
    pub fn wait_any_format(&self, timeout: Duration) -> Option<NegotiatedFormat> {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.lock();
        loop {
            if s.fmt.is_some() || s.closed || s.error.is_some() {
                return s.fmt;
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            s = self
                .shared
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Takes the newest frame, waiting up to `timeout`. `Err` when the stream died.
    pub fn next(&self, timeout: Duration) -> Result<Option<Frame>, String> {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.lock();
        loop {
            if let Some(f) = s.latest.take() {
                return Ok(Some(f));
            }
            if let Some(e) = &s.error {
                return Err(e.clone());
            }
            if s.closed {
                return Err("PipeWire stream closed (portal session ended?)".into());
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            s = self
                .shared
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Frames delivered by the compositor and frames replaced before the encoder took them.
    pub fn counters(&self) -> (u64, u64) {
        let s = self.shared.lock();
        (s.produced, s.coalesced)
    }

    /// Re-negotiates with shared memory only (used when DMA-BUF import failed).
    pub fn disable_dmabuf(&self) {
        let _ = self.tx.send(Cmd::ForceShm);
    }

    /// Stops the thread.
    pub fn stop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for PwCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Local {
    shared: Arc<Shared>,
    cursor: CursorSink,
    tracker: CursorTracker,
    tx: pw::channel::Sender<Cmd>,
    params: PwParams,
    fmt: Option<NegotiatedFormat>,
    generation: u64,
    live: HashSet<usize>,
    force_shm: bool,
    seq: u64,
}

fn mono_now_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

fn cursor_meta_size() -> u32 {
    (crate::cursor_meta::META_CURSOR_SIZE
        + crate::cursor_meta::META_BITMAP_SIZE
        + CURSOR_MAX_EDGE * CURSOR_MAX_EDGE * 4) as u32
}

fn enum_formats(params: &PwParams, with_dmabuf: bool) -> Vec<Vec<u8>> {
    let size = Val::RangeRect((params.width, params.height), (1, 1), (16384, 16384));
    let framerate = Val::RangeFrac((0, 1), (0, 1), (240, 1));
    let mk = |format: u32, modifiers: Option<Vec<u64>>| VideoFormatProps {
        format,
        modifiers,
        size: size.clone(),
        framerate: framerate.clone(),
        max_fps: params.fps.max(1),
    };
    let mut out = Vec::new();
    if let (true, Some(mods)) = (with_dmabuf, &params.dmabuf_modifiers) {
        for f in OFFERED_FORMATS {
            out.push(pod::enum_format(
                pod::PARAM_ENUM_FORMAT,
                &mk(f, Some(mods.clone())),
            ));
        }
    }
    for f in OFFERED_FORMATS {
        out.push(pod::enum_format(pod::PARAM_ENUM_FORMAT, &mk(f, None)));
    }
    out
}

fn update_params(stream: &pw::stream::Stream, blobs: &[Vec<u8>]) {
    let pods: Vec<&Pod> = blobs.iter().filter_map(|b| Pod::from_bytes(b)).collect();
    let mut refs: Vec<&Pod> = pods;
    if let Err(e) = stream.update_params(&mut refs) {
        tracing::warn!("pw_stream_update_params failed: {e}");
    }
}

fn on_format(stream: &pw::stream::Stream, local: &Rc<RefCell<Local>>, param: Option<&Pod>) {
    let Some(param) = param else {
        let mut l = local.borrow_mut();
        l.fmt = None;
        l.generation += 1;
        let mut s = l.shared.lock();
        s.fmt = None;
        return;
    };
    let Some(pf) = pod::parse_format(param.as_bytes()) else {
        return;
    };
    let (Some(format), Some((w, h))) = (pf.format, pf.size) else {
        return;
    };
    tracing::debug!(?pf, "pipewire format");
    if let Some(mods) = &pf.modifiers
        && (pf.modifier_dont_fixate || mods.len() > 1)
    {
        let supported = local
            .borrow()
            .params
            .dmabuf_modifiers
            .clone()
            .unwrap_or_default();
        match choose_modifier(mods, &supported) {
            Some(m) => {
                tracing::debug!(
                    modifier = format_args!("{m:#x}"),
                    "fixating DMA-BUF modifier"
                );
                if let Some(f) = pod::fixated_format(&pf, m) {
                    update_params(stream, &[f]);
                }
            }
            None => {
                tracing::warn!(
                    "no importable DMA-BUF modifier among {mods:x?}; falling back to shared memory"
                );
                local.borrow_mut().force_shm = true;
                let blobs = enum_formats(&local.borrow().params, false);
                update_params(stream, &blobs);
            }
        }
        return;
    }
    let dmabuf = pf.modifiers.is_some();
    let nf = NegotiatedFormat {
        width: w,
        height: h,
        spa_format: format,
        modifier: pf.modifiers.as_ref().and_then(|m| m.first().copied()),
    };
    {
        let mut l = local.borrow_mut();
        l.fmt = Some(nf);
        l.generation += 1;
        let mut s = l.shared.lock();
        s.fmt = Some(nf);
        s.fmt_version += 1;
    }
    local.borrow().shared.cv.notify_all();
    tracing::info!(
        width = w,
        height = h,
        format,
        dmabuf,
        modifier = ?nf.modifier.map(|m| format!("{m:#x}")),
        "pipewire stream format"
    );
    let data_types = if dmabuf {
        1 << pod::DATA_DMA_BUF
    } else {
        (1 << pod::DATA_MEM_FD) | (1 << pod::DATA_MEM_PTR)
    };
    update_params(
        stream,
        &[
            pod::buffers_param(data_types),
            pod::meta_param(pod::META_CURSOR, cursor_meta_size()),
            pod::meta_param(
                pod::META_HEADER,
                std::mem::size_of::<spa_sys::spa_meta_header>() as u32,
            ),
        ],
    );
}

/// Finds a meta block of `ty` in the buffer.
///
/// # Safety
/// `sb` must point at a live `spa_buffer`.
unsafe fn find_meta<'a>(sb: *const spa_sys::spa_buffer, ty: u32) -> Option<&'a [u8]> {
    // SAFETY: caller guarantees `sb` is live; metas/n_metas describe a valid array.
    unsafe {
        let sb = &*sb;
        if sb.metas.is_null() {
            return None;
        }
        for i in 0..sb.n_metas as usize {
            let m = &*sb.metas.add(i);
            if m.type_ == ty && !m.data.is_null() {
                return Some(std::slice::from_raw_parts(
                    m.data as *const u8,
                    m.size as usize,
                ));
            }
        }
    }
    None
}

fn dup_fd(fd: i64) -> Option<OwnedFd> {
    // SAFETY: plain fcntl; the new fd is owned by the returned OwnedFd.
    let n = unsafe { libc::fcntl(fd as i32, libc::F_DUPFD_CLOEXEC, 0) };
    // SAFETY: n is a fresh valid fd when >= 0.
    (n >= 0).then(|| unsafe { OwnedFd::from_raw_fd(n) })
}

/// Handles the buffers dequeued in one `process` call.
fn on_process(stream: &pw::stream::Stream, local: &Rc<RefCell<Local>>) {
    let mut newest: *mut PwBuffer = std::ptr::null_mut();
    // Every new shape must reach the client (later states may refer to it); only the last state
    // matters.
    let mut cursor_shapes = Vec::new();
    let mut cursor_state = None;
    let recv_ns = mono_now_ns();
    let recv_us = now_us();
    // Drain the queue: only the newest buffer with content is kept.
    loop {
        // SAFETY: dequeue/queue pairs on the stream owned by this thread.
        let b = unsafe { stream.dequeue_raw_buffer() };
        if b.is_null() {
            break;
        }
        // SAFETY: b is a live buffer owned by us until requeued.
        let (has_video, cursor_events) = unsafe { inspect(b, local) };
        if let Some((shape, state)) = cursor_events {
            cursor_shapes.extend(shape);
            if state.is_some() {
                cursor_state = state;
            }
        }
        if has_video {
            if !newest.is_null() {
                // SAFETY: `newest` was dequeued from this stream and not queued yet.
                unsafe { stream.queue_raw_buffer(newest) };
                local.borrow().shared.lock().coalesced += 1;
            }
            newest = b;
        } else {
            // SAFETY: as above.
            unsafe { stream.queue_raw_buffer(b) };
        }
    }
    // Cursor first, never gated on video.
    if !cursor_shapes.is_empty() || cursor_state.is_some() {
        let l = local.borrow();
        for s in cursor_shapes {
            let _ = l.cursor.set_shape(s);
        }
        if let Some(st) = cursor_state {
            let _ = l.cursor.update(CursorState {
                x: st.x,
                y: st.y,
                visible: st.visible,
                shape_id: st.shape_id,
                ts_us: now_us(),
            });
        }
    }
    if newest.is_null() {
        return;
    }
    // SAFETY: newest is a live dequeued buffer.
    let frame = unsafe { make_frame(stream, local, newest, recv_ns, recv_us) };
    match frame {
        Some(f) => {
            let l = local.borrow();
            let mut s = l.shared.lock();
            s.produced += 1;
            let old = s.latest.replace(f);
            if old.is_some() {
                s.coalesced += 1;
            }
            drop(s);
            l.shared.cv.notify_all();
            drop(l);
            drop(old);
        }
        None => {
            // The frame could not be built (DMA-BUF dup failed, short shared-memory buffer).
            // SAFETY: still checked out.
            unsafe { stream.queue_raw_buffer(newest) };
        }
    }
}

type PwBuffer = pw::sys::pw_buffer;
type CursorEv = (
    Option<ezd_proto::CursorShape>,
    Option<crate::cursor_meta::StateUpdate>,
);

/// Reads cursor metadata and decides if the buffer carries video.
///
/// # Safety
/// `b` must be a live dequeued buffer.
unsafe fn inspect(b: *mut PwBuffer, local: &Rc<RefCell<Local>>) -> (bool, Option<CursorEv>) {
    // SAFETY: caller contract; all derefs stay inside the spa_buffer description.
    unsafe {
        let sb = (*b).buffer;
        if sb.is_null() || (*sb).n_datas == 0 {
            return (false, None);
        }
        let d0 = &*(*sb).datas;
        let chunk = &*d0.chunk;
        let corrupted = chunk.flags & spa_sys::SPA_CHUNK_FLAG_CORRUPTED as i32 != 0;
        let is_dma = d0.type_ == spa_sys::SPA_DATA_DmaBuf;
        let has_video = !corrupted && (is_dma || chunk.size != 0);
        let cursor_ev = find_meta(sb, spa_sys::SPA_META_Cursor)
            .and_then(parse_cursor_meta)
            .map(|m| local.borrow_mut().tracker.update(&m))
            .filter(|ev| ev.shape.is_some() || ev.state.is_some())
            .map(|ev| (ev.shape, ev.state));
        (has_video, cursor_ev)
    }
}

/// Builds a [`Frame`] from a dequeued buffer. Shared-memory buffers are copied and requeued here;
/// DMA-BUF buffers stay checked out until the frame is dropped.
///
/// # Safety
/// `b` must be a live dequeued buffer of `stream`.
unsafe fn make_frame(
    stream: &pw::stream::Stream,
    local: &Rc<RefCell<Local>>,
    b: *mut PwBuffer,
    recv_ns: i64,
    recv_us: u64,
) -> Option<Frame> {
    let (fmt, generation, tx) = {
        let l = local.borrow();
        (l.fmt?, l.generation, l.tx.clone())
    };
    let seq = {
        let mut l = local.borrow_mut();
        l.seq += 1;
        l.seq
    };
    // SAFETY: caller contract.
    unsafe {
        let sb = (*b).buffer;
        let pts = find_meta(sb, spa_sys::SPA_META_Header)
            .filter(|m| m.len() >= std::mem::size_of::<spa_sys::spa_meta_header>())
            .map(|m| {
                (m.as_ptr() as *const spa_sys::spa_meta_header)
                    .read_unaligned()
                    .pts
            })
            .unwrap_or(0);
        let capture_us = capture_us_from_pts(pts, recv_ns, recv_us);
        let n = (*sb).n_datas as usize;
        let datas = std::slice::from_raw_parts((*sb).datas, n);
        let (w, h) = (fmt.width, fmt.height);
        if datas[0].type_ == spa_sys::SPA_DATA_DmaBuf {
            let mut planes = Vec::new();
            for d in datas {
                let c = &*d.chunk;
                planes.push(Plane {
                    fd: dup_fd(d.fd)?,
                    size: d.maxsize as u64,
                    offset: c.offset,
                    pitch: if c.stride > 0 { c.stride as u32 } else { w * 4 },
                });
            }
            let ptr = b as usize;
            let release = Release(Some(Box::new(move || {
                let _ = tx.send(Cmd::Release { ptr, generation });
            })));
            return Some(Frame {
                width: w,
                height: h,
                capture_us,
                seq,
                pixels: Pixels::Dma(DmaBuf {
                    spa_format: fmt.spa_format,
                    modifier: fmt.modifier.unwrap_or(DRM_MOD_LINEAR),
                    planes,
                    release,
                }),
            });
        }
        // Shared memory: copy out and give the buffer back right away.
        let d = &datas[0];
        let c = &*d.chunk;
        let stride = if c.stride > 0 {
            c.stride as usize
        } else {
            w as usize * 4
        };
        let need = stride * (h as usize - 1) + w as usize * 4;
        if d.data.is_null() || (c.offset as usize + need) > d.maxsize as usize {
            tracing::warn!(
                size = c.size,
                maxsize = d.maxsize,
                need,
                "shared-memory buffer too small or unmapped"
            );
            return None;
        }
        let src = std::slice::from_raw_parts((d.data as *const u8).add(c.offset as usize), need);
        let data = src.to_vec();
        stream.queue_raw_buffer(b);
        Some(Frame {
            width: w,
            height: h,
            capture_us,
            seq,
            pixels: Pixels::Shm(ShmBuf {
                spa_format: fmt.spa_format,
                stride,
                data,
            }),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    fd: Option<OwnedFd>,
    target: Target,
    params: PwParams,
    cursor: CursorSink,
    shared: Arc<Shared>,
    tx: pw::channel::Sender<Cmd>,
    rx: pw::channel::Receiver<Cmd>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let e = |what: &str, err: pw::Error| format!("pipewire {what}: {err}");
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|x| e("main loop", x))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|x| e("context", x))?;
    let core = match fd {
        Some(fd) => context.connect_fd_rc(fd, None),
        None => context.connect_rc(None),
    }
    .map_err(|x| e("connect", x))?;

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    let node_id = match &target {
        Target::Node(id) => Some(*id),
        Target::Name(n) => {
            props.insert("target.object", n.as_str());
            None
        }
    };
    let stream =
        pw::stream::StreamRc::new(core.clone(), "ezdisplay", props).map_err(|x| e("stream", x))?;

    let local = Rc::new(RefCell::new(Local {
        shared: shared.clone(),
        cursor,
        tracker: CursorTracker::default(),
        tx: tx.clone(),
        params: params.clone(),
        fmt: None,
        generation: 0,
        live: HashSet::new(),
        force_shm: false,
        seq: 0,
    }));

    let ml = mainloop.clone();
    let sh = shared.clone();
    let listener = stream
        .add_local_listener_with_user_data(local.clone())
        .state_changed(move |_, _, old, new| {
            tracing::debug!("pipewire stream state {old:?} -> {new:?}");
            match new {
                pw::stream::StreamState::Error(msg) => {
                    sh.lock().error = Some(format!("pipewire stream error: {msg}"));
                    sh.cv.notify_all();
                    ml.quit();
                }
                pw::stream::StreamState::Unconnected
                    if matches!(
                        old,
                        pw::stream::StreamState::Streaming | pw::stream::StreamState::Paused
                    ) =>
                {
                    ml.quit();
                }
                _ => {}
            }
        })
        .param_changed(|stream, local, id, param| {
            if id == pw::spa::param::ParamType::Format.as_raw() {
                on_format(stream, local, param);
            }
        })
        .add_buffer(|_, local, b| {
            local.borrow_mut().live.insert(b as usize);
        })
        .remove_buffer(|_, local, b| {
            let mut l = local.borrow_mut();
            l.live.remove(&(b as usize));
            l.generation += 1;
        })
        .process(|stream, local| on_process(stream, local))
        .register()
        .map_err(|x| e("listener", x))?;

    // Commands from other threads.
    let weak = stream.downgrade();
    let ml = mainloop.clone();
    let lc = local.clone();
    let _rx = rx.attach(mainloop.loop_(), move |cmd| match cmd {
        Cmd::Quit => ml.quit(),
        Cmd::Release { ptr, generation } => {
            let ok = {
                let l = lc.borrow();
                l.generation == generation && l.live.contains(&ptr)
            };
            if ok && let Some(s) = weak.upgrade() {
                // SAFETY: the buffer is live (tracked by add/remove_buffer, same generation) and was
                // dequeued by `on_process`.
                unsafe { s.queue_raw_buffer(ptr as *mut PwBuffer) };
            }
        }
        Cmd::ForceShm => {
            lc.borrow_mut().force_shm = true;
            let blobs = enum_formats(&lc.borrow().params, false);
            if let Some(s) = weak.upgrade() {
                update_params(&s, &blobs);
            }
        }
    });

    let blobs = enum_formats(&params, true);
    let mut pods: Vec<&Pod> = blobs.iter().filter_map(|b| Pod::from_bytes(b)).collect();
    stream
        .connect(
            pw::spa::utils::Direction::Input,
            node_id,
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut pods,
        )
        .map_err(|x| e("stream connect", x))?;
    let _ = ready.send(Ok(()));
    mainloop.run();

    drop(listener);
    let _ = stream.disconnect();
    Ok(())
}

/// Exposed for tests: the number of modifiers offered by default for Intel.
pub fn default_intel_modifiers() -> Vec<u64> {
    vec![
        DRM_MOD_LINEAR,
        // I915_FORMAT_MOD_X_TILED, I915_FORMAT_MOD_Y_TILED
        0x0100_0000_0000_0001,
        0x0100_0000_0000_0002,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_formats_cover_dmabuf_then_shm() {
        let p = PwParams {
            width: 1920,
            height: 1080,
            fps: 60,
            dmabuf_modifiers: Some(default_intel_modifiers()),
        };
        let all = enum_formats(&p, true);
        assert_eq!(all.len(), OFFERED_FORMATS.len() * 2);
        let first = pod::parse_format(&all[0]).unwrap();
        assert!(first.modifiers.is_some() && first.modifier_dont_fixate);
        let last = pod::parse_format(all.last().unwrap()).unwrap();
        assert!(last.modifiers.is_none());
        assert_eq!(enum_formats(&p, false).len(), OFFERED_FORMATS.len());
        let shm_only = PwParams {
            dmabuf_modifiers: None,
            ..p
        };
        assert_eq!(enum_formats(&shm_only, true).len(), OFFERED_FORMATS.len());
    }

    #[test]
    fn cursor_meta_size_fits_256_bitmap() {
        assert!(cursor_meta_size() as usize >= 256 * 256 * 4 + 48);
    }
}
