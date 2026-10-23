//! DXGI Desktop Duplication capture on its own thread.
//!
//! The thread keeps exactly one GPU texture (`latest`) up to date: every
//! presented frame is `CopyResource`d into it and the duplication frame is
//! released immediately. The consumer copies `latest` into the encoder's
//! surface; commands on the immediate context are serialised (the device is
//! multithread protected), so a consumer copy always sees a complete frame.
//!
//! Pointer position and shape go to the [`CursorSink`] from this thread as soon
//! as they arrive; they never wait for video.

use super::util::err_str;
use crate::cursor_shape;
use ezd_host_core::{CursorSink, now_us};
use ezd_proto::{CursorShape, CursorState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_DESC, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, IDXGIOutput, IDXGIOutput1, IDXGIOutput5,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

/// Result of waiting for a frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameTicket {
    /// Counter of captured frames; increases with every presented frame.
    pub seq: u64,
    /// `now_us` when the frame was acquired.
    pub capture_us: u64,
}

#[derive(Default)]
struct State {
    seq: u64,
    capture_us: u64,
    has_frame: bool,
    error: Option<String>,
    frames: u64,
    access_lost: u64,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    stop: AtomicBool,
}

/// Running capture.
pub struct Capture {
    /// The newest frame, BGRA, `width x height`.
    pub latest: ID3D11Texture2D,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// A zero-initialised BGRA texture usable as render target and shader resource.
pub fn create_bgra_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, String> {
    let zeros = vec![0u8; width as usize * height as usize * 4];
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: zeros.as_ptr().cast(),
        SysMemPitch: width * 4,
        SysMemSlicePitch: 0,
    };
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    // SAFETY: `desc` and `init` are valid; `zeros` outlives the call.
    unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)) }
        .map_err(|e| format!("CreateTexture2D: {}", err_str(&e)))?;
    tex.ok_or_else(|| "CreateTexture2D returned no texture".to_string())
}

/// Which duplication API succeeded (for diagnostics).
pub struct Duplication {
    pub dup: IDXGIOutputDuplication,
    pub desc: DXGI_OUTDUPL_DESC,
    pub via: &'static str,
}

/// `IDXGIOutput5::DuplicateOutput1` with B8G8R8A8, falling back to `DuplicateOutput`.
pub fn duplicate(device: &ID3D11Device, output: &IDXGIOutput) -> Result<Duplication, String> {
    let mut why = String::new();
    let mut made: Option<(IDXGIOutputDuplication, &'static str)> = None;
    if let Ok(o5) = output.cast::<IDXGIOutput5>() {
        // SAFETY: plain COM call with a valid device and a one-element format list.
        match unsafe { o5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]) } {
            Ok(d) => made = Some((d, "DuplicateOutput1")),
            Err(e) => why = format!("DuplicateOutput1: {}", err_str(&e)),
        }
    } else {
        why = "IDXGIOutput5 not available".into();
    }
    if made.is_none() {
        let o1 = output
            .cast::<IDXGIOutput1>()
            .map_err(|e| format!("IDXGIOutput1: {}", err_str(&e)))?;
        // SAFETY: plain COM call with a valid device.
        match unsafe { o1.DuplicateOutput(device) } {
            Ok(d) => made = Some((d, "DuplicateOutput")),
            Err(e) => return Err(format!("{why}; DuplicateOutput: {}", err_str(&e))),
        }
    }
    let (dup, via) = made.expect("set above");
    // SAFETY: plain COM call returning a value.
    let desc = unsafe { dup.GetDesc() };
    Ok(Duplication { dup, desc, via })
}

struct CursorTracker {
    sink: CursorSink,
    hot: (i32, i32),
    shape_id: u32,
    pos: (i32, i32),
    visible: bool,
    have_pos: bool,
}

impl CursorTracker {
    fn on_frame(&mut self, dup: &IDXGIOutputDuplication, info: &DXGI_OUTDUPL_FRAME_INFO) {
        let mut changed = false;
        if info.LastMouseUpdateTime != 0 {
            self.pos = (
                info.PointerPosition.Position.x,
                info.PointerPosition.Position.y,
            );
            self.visible = info.PointerPosition.Visible.as_bool();
            self.have_pos = true;
            changed = true;
        }
        if info.PointerShapeBufferSize > 0 {
            match self.fetch_shape(dup, info.PointerShapeBufferSize) {
                Ok(()) => changed = true,
                Err(e) => tracing::warn!("pointer shape: {e}"),
            }
        }
        if changed && self.have_pos && self.shape_id != 0 {
            // DDA reports the position of the pointer image's top-left corner; the protocol
            // wants the hotspot.
            let _ = self.sink.update(CursorState {
                x: self.pos.0 + self.hot.0,
                y: self.pos.1 + self.hot.1,
                visible: self.visible,
                shape_id: self.shape_id,
                ts_us: now_us(),
            });
        }
    }

    fn fetch_shape(&mut self, dup: &IDXGIOutputDuplication, size: u32) -> Result<(), String> {
        let mut buf = vec![0u8; size as usize];
        let mut needed = 0u32;
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        // SAFETY: `buf` is `size` bytes; out-parameters are valid.
        unsafe { dup.GetFramePointerShape(size, buf.as_mut_ptr().cast(), &mut needed, &mut info) }
            .map_err(|e| format!("GetFramePointerShape: {}", err_str(&e)))?;
        let rgba = cursor_shape::to_rgba(info.Type, info.Width, info.Height, info.Pitch, &buf)?;
        let (hx, hy) = (info.HotSpot.x.max(0) as u32, info.HotSpot.y.max(0) as u32);
        let id = cursor_shape::shape_id(&rgba, hx, hy);
        if !self.sink.has_shape(id) {
            let _ = self.sink.set_shape(CursorShape {
                shape_id: id,
                width: rgba.width as u16,
                height: rgba.height as u16,
                hot_x: hx as u16,
                hot_y: hy as u16,
                rgba: rgba.data,
            });
        }
        self.shape_id = id;
        self.hot = (hx as i32, hy as i32);
        Ok(())
    }
}

impl Capture {
    /// Creates the duplication (synchronously, so errors surface in `start`) and
    /// spawns the capture thread.
    pub fn start(
        device: &ID3D11Device,
        output: &IDXGIOutput,
        width: u32,
        height: u32,
        cursor: CursorSink,
    ) -> Result<Capture, String> {
        let first = duplicate(device, output)?;
        if first.desc.ModeDesc.Width != width || first.desc.ModeDesc.Height != height {
            return Err(format!(
                "the captured output is {}x{} but the stream needs {width}x{height}",
                first.desc.ModeDesc.Width, first.desc.ModeDesc.Height
            ));
        }
        if first.desc.DesktopImageInSystemMemory.as_bool() {
            tracing::warn!(
                "desktop duplication delivers the image in system memory (cross-adapter?)"
            );
        }
        tracing::info!("desktop duplication via {}", first.via);

        let latest = create_bgra_texture(device, width, height)?;

        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let (t_shared, t_latest, t_dev, t_out) = (
            shared.clone(),
            latest.clone(),
            device.clone(),
            output.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("ezd-capture".into())
            .spawn(move || {
                capture_loop(
                    t_shared, t_dev, t_out, first, t_latest, width, height, cursor,
                )
            })
            .map_err(|e| format!("spawn capture thread: {e}"))?;
        Ok(Capture {
            latest,
            shared,
            thread: Some(thread),
        })
    }

    /// Waits for a frame newer than `last_seq` (or any frame when `force`).
    /// `Ok(None)` on timeout.
    pub fn wait_frame(
        &self,
        last_seq: u64,
        force: bool,
        timeout: Duration,
    ) -> Result<Option<FrameTicket>, String> {
        let deadline = Instant::now() + timeout;
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if st.has_frame && (st.seq > last_seq || force) {
                return Ok(Some(FrameTicket {
                    seq: st.seq,
                    capture_us: st.capture_us,
                }));
            }
            if let Some(e) = &st.error {
                return Err(e.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            st = self
                .shared
                .cv
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// `(frames captured, access-lost events)`.
    pub fn stats(&self) -> (u64, u64) {
        let st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        (st.frames, st.access_lost)
    }

    /// Stops the thread.
    pub fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[allow(clippy::too_many_arguments)]
fn capture_loop(
    shared: Arc<Shared>,
    device: ID3D11Device,
    output: IDXGIOutput,
    first: Duplication,
    latest: ID3D11Texture2D,
    width: u32,
    height: u32,
    sink: CursorSink,
) {
    // SAFETY: plain COM call.
    let ctx: ID3D11DeviceContext = match unsafe { device.GetImmediateContext() } {
        Ok(c) => c,
        Err(e) => {
            fail(&shared, format!("GetImmediateContext: {}", err_str(&e)));
            return;
        }
    };
    let mut cursor = CursorTracker {
        sink,
        hot: (0, 0),
        shape_id: 0,
        pos: (0, 0),
        visible: false,
        have_pos: false,
    };
    let mut dup = Some(first.dup);
    let mut backoff = Duration::from_millis(50);
    let started = Instant::now();
    while !shared.stop.load(Ordering::SeqCst) {
        let Some(d) = dup.as_ref() else {
            match duplicate(&device, &output) {
                Ok(n) if n.desc.ModeDesc.Width == width && n.desc.ModeDesc.Height == height => {
                    tracing::info!("desktop duplication re-created via {}", n.via);
                    dup = Some(n.dup);
                    backoff = Duration::from_millis(50);
                }
                Ok(n) => {
                    tracing::warn!(
                        "output is now {}x{}, stream is {width}x{height}; waiting",
                        n.desc.ModeDesc.Width,
                        n.desc.ModeDesc.Height
                    );
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(1));
                }
                Err(e) => {
                    tracing::debug!("duplication not available yet: {e}");
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(1));
                }
            }
            continue;
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        // SAFETY: valid out-parameters; ReleaseFrame is called below on success.
        let r = unsafe { d.AcquireNextFrame(50, &mut info, &mut res) };
        match r {
            Ok(()) => {
                let acquired = now_us();
                let mut new_frame = false;
                if info.LastPresentTime != 0 {
                    if let Some(tex) = res.as_ref().and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                        // SAFETY: both textures belong to `device` and have identical size and format.
                        unsafe { ctx.CopyResource(&latest, &tex) };
                        new_frame = true;
                    }
                }
                cursor.on_frame(d, &info);
                drop(res);
                // SAFETY: a frame was acquired above.
                if let Err(e) = unsafe { d.ReleaseFrame() } {
                    tracing::debug!("ReleaseFrame: {}", err_str(&e));
                }
                if new_frame {
                    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.seq += 1;
                    st.frames += 1;
                    st.capture_us = acquired;
                    st.has_frame = true;
                    drop(st);
                    shared.cv.notify_all();
                }
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                // Nothing presented. An idle virtual display may never present at all, so
                // after a moment publish the (black) initial texture to get the stream going.
                let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                if !st.has_frame && started.elapsed() > Duration::from_millis(300) {
                    st.has_frame = true;
                    st.seq += 1;
                    st.capture_us = now_us();
                    drop(st);
                    shared.cv.notify_all();
                }
            }
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                tracing::info!(
                    "desktop duplication lost (mode change or secure desktop); re-creating"
                );
                shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .access_lost += 1;
                dup = None;
                std::thread::sleep(backoff);
            }
            Err(e) => {
                tracing::warn!("AcquireNextFrame: {}", err_str(&e));
                dup = None;
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(1));
            }
        }
    }
}

fn fail(shared: &Shared, msg: String) {
    tracing::error!("{msg}");
    shared.state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(msg);
    shared.cv.notify_all();
}
