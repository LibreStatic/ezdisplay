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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
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
    /// Bits of the `f32` SDR white level (cd/m2) last read from Windows.
    sdr_white_bits: AtomicU32,
}

/// How often the capture thread re-reads the HDR state of the output.
const HDR_RECHECK: Duration = Duration::from_secs(1);

/// Running capture.
pub struct Capture {
    /// The newest frame, `width x height`, in [`format`](Self::format).
    pub latest: ID3D11Texture2D,
    /// Format of `latest`: BGRA8 for SDR, FP16 scRGB when the output is HDR (unless DXGI only
    /// offered BGRA8).
    pub format: DXGI_FORMAT,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// A zero-initialised BGRA texture usable as render target and shader resource.
pub fn create_bgra_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, String> {
    create_zeroed_texture(device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, 4)
}

/// A zero-initialised texture of `format` (`bytes_per_pixel` each) usable as render target and
/// shader resource.
pub fn create_zeroed_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    bytes_per_pixel: u32,
) -> Result<ID3D11Texture2D, String> {
    let zeros = vec![0u8; width as usize * height as usize * bytes_per_pixel as usize];
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: zeros.as_ptr().cast(),
        SysMemPitch: width * bytes_per_pixel,
        SysMemSlicePitch: 0,
    };
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
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

/// The output to capture and what the session was set up for.
#[derive(Clone, Copy)]
pub struct CaptureOutput<'a> {
    pub output: &'a IDXGIOutput,
    /// GDI device name of the output, to re-read its HDR state.
    pub device_name: &'a str,
    /// HDR state of the output at the start of the session.
    pub hdr: bool,
    /// SDR white level in cd/m2 at the start.
    pub sdr_white_nits: f32,
}

/// Which duplication API succeeded (for diagnostics).
pub struct Duplication {
    pub dup: IDXGIOutputDuplication,
    pub desc: DXGI_OUTDUPL_DESC,
    pub via: &'static str,
}

/// Formats offered to `DuplicateOutput1`, best first. An HDR desktop is delivered as FP16 scRGB
/// when asked for; with BGRA8 only, DXGI would clip it to 8 bits without tone mapping.
fn duplication_formats(hdr: bool) -> &'static [DXGI_FORMAT] {
    if hdr {
        &[DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM]
    } else {
        &[DXGI_FORMAT_B8G8R8A8_UNORM]
    }
}

/// `IDXGIOutput5::DuplicateOutput1` (FP16 first when `hdr`, else B8G8R8A8), falling back to
/// `DuplicateOutput`.
pub fn duplicate(
    device: &ID3D11Device,
    output: &IDXGIOutput,
    hdr: bool,
) -> Result<Duplication, String> {
    let mut why = String::new();
    let mut made: Option<(IDXGIOutputDuplication, &'static str)> = None;
    if let Ok(o5) = output.cast::<IDXGIOutput5>() {
        // SAFETY: plain COM call with a valid device and a short format list.
        match unsafe { o5.DuplicateOutput1(device, 0, duplication_formats(hdr)) } {
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
    ///
    /// When Windows HDR is switched on or off after the start the capture ends with an error
    /// ("display HDR mode changed") so the session ends and the client reconnects and renegotiates.
    pub fn start(
        device: &ID3D11Device,
        target: &CaptureOutput<'_>,
        width: u32,
        height: u32,
        cursor: CursorSink,
    ) -> Result<Capture, String> {
        let CaptureOutput {
            output,
            device_name,
            hdr,
            sdr_white_nits,
        } = *target;
        let first = duplicate(device, output, hdr)?;
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
        // What DXGI really delivers decides the format of `latest` (CopyResource needs equal formats).
        let format = first.desc.ModeDesc.Format;
        let latest = match format {
            DXGI_FORMAT_R16G16B16A16_FLOAT => {
                create_zeroed_texture(device, width, height, format, 8)?
            }
            DXGI_FORMAT_B8G8R8A8_UNORM => create_bgra_texture(device, width, height)?,
            other => {
                return Err(format!(
                    "desktop duplication delivers {other:?}, which is not handled"
                ));
            }
        };
        tracing::info!(
            "desktop duplication via {}, {format:?}, output HDR: {hdr}",
            first.via
        );

        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            sdr_white_bits: AtomicU32::new(sdr_white_nits.to_bits()),
        });
        let watch = HdrWatch {
            device_name: device_name.to_string(),
            session_hdr: hdr,
            format,
        };
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
                    t_shared, t_dev, t_out, first, t_latest, width, height, cursor, watch,
                )
            })
            .map_err(|e| format!("spawn capture thread: {e}"))?;
        Ok(Capture {
            latest,
            format,
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
            // A dead capture (HDR switched, device lost) ends the session even when an older
            // frame could still be sent: the client has to renegotiate.
            if let Some(e) = &st.error {
                return Err(e.clone());
            }
            if st.has_frame && (st.seq > last_seq || force) {
                return Ok(Some(FrameTicket {
                    seq: st.seq,
                    capture_us: st.capture_us,
                }));
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

    /// SDR white level in cd/m2 as of the last HDR re-check (about once a second).
    pub fn sdr_white_nits(&self) -> f32 {
        f32::from_bits(self.shared.sdr_white_bits.load(Ordering::Relaxed))
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

/// What the capture thread needs to notice that the output changed under the session.
struct HdrWatch {
    device_name: String,
    session_hdr: bool,
    format: DXGI_FORMAT,
}

impl HdrWatch {
    /// Re-reads the output's HDR state (through a fresh factory) and the SDR white level.
    /// `Err` when the HDR state is no longer the one the session was negotiated for.
    fn check(&self, shared: &Shared) -> Result<(), String> {
        let Some((color, white)) = super::dxgi::fresh_output_color(&self.device_name) else {
            return Ok(());
        };
        shared
            .sdr_white_bits
            .store(white.to_bits(), Ordering::Relaxed);
        if color.hdr != self.session_hdr {
            return Err("display HDR mode changed".to_string());
        }
        Ok(())
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
    watch: HdrWatch,
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
    let mut last_check = Instant::now();
    while !shared.stop.load(Ordering::SeqCst) {
        if last_check.elapsed() >= HDR_RECHECK {
            last_check = Instant::now();
            if let Err(e) = watch.check(&shared) {
                fail(&shared, e);
                return;
            }
        }
        let Some(d) = dup.as_ref() else {
            match duplicate(&device, &output, watch.session_hdr) {
                Ok(n)
                    if n.desc.ModeDesc.Width == width
                        && n.desc.ModeDesc.Height == height
                        && n.desc.ModeDesc.Format == watch.format =>
                {
                    tracing::info!("desktop duplication re-created via {}", n.via);
                    dup = Some(n.dup);
                    backoff = Duration::from_millis(50);
                }
                Ok(n) => {
                    tracing::warn!(
                        "output is now {}x{} {:?}, stream is {width}x{height} {:?}; waiting",
                        n.desc.ModeDesc.Width,
                        n.desc.ModeDesc.Height,
                        n.desc.ModeDesc.Format,
                        watch.format
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
        // Poll instead of blocking: AcquireNextFrame holds the device's multithread lock for
        // its whole timeout, which stalls the encoder sharing this device (an AMF encode went
        // from ~23 ms to 60-350 ms with a 50 ms timeout).
        // SAFETY: valid out-parameters; ReleaseFrame is called below on success.
        let r = unsafe { d.AcquireNextFrame(0, &mut info, &mut res) };
        if matches!(&r, Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT) {
            std::thread::sleep(Duration::from_millis(1));
        }
        match r {
            Ok(()) => {
                let acquired = now_us();
                let mut new_frame = false;
                if info.LastPresentTime != 0 {
                    if let Some(tex) = res.as_ref().and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                        let mut td = D3D11_TEXTURE2D_DESC::default();
                        // SAFETY: valid out-parameter.
                        unsafe { tex.GetDesc(&mut td) };
                        if td.Format != watch.format {
                            // Never copy across formats; end the session so it renegotiates.
                            let _ = unsafe { d.ReleaseFrame() };
                            fail(
                                &shared,
                                format!(
                                    "desktop duplication switched from {:?} to {:?}",
                                    watch.format, td.Format
                                ),
                            );
                            return;
                        }
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
                // A mode change often is the user toggling Windows HDR: decide now rather than
                // streaming the wrong format until the next periodic check.
                last_check = Instant::now();
                if let Err(e) = watch.check(&shared) {
                    fail(&shared, e);
                    return;
                }
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
