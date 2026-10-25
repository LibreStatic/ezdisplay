//! GPU encoder front-end: takes the captured BGRA texture and produces H.264.
//!
//! * NVENC and AMF: zero-copy. The BGRA texture is copied (GPU to GPU) into a
//!   D3D11 frame of the encoder's pool (see [`super::ffenc`]).
//!   If the encoder rejects those frames, [`probe`] falls back to single textures and then to
//!   the NV12 path below.
//! * QSV and libx264: the texture is converted to NV12 by the D3D11 video
//!   processor, read back, and given to the regular `ezd_ffmpeg::Encoder`
//!   (QSV uploads the NV12 itself; FFmpeg's QSV encoder wants QSV surfaces or system memory).

use super::ffenc::ZeroCopyEncoder;
use super::util::err_str;
use crate::annexb;
use ezd_ffmpeg::{Encoder, EncoderConfig, RawPixelFormat};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC};
use windows::core::Interface;

/// How captured frames reach an encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPath {
    /// D3D11 frames from a texture-array pool.
    ZeroCopyArray,
    /// D3D11 frames that are each their own texture.
    ZeroCopySingle,
    /// NV12 from the video processor, read back into system memory.
    SystemMemory,
}

impl InputPath {
    fn describe(self) -> &'static str {
        match self {
            InputPath::ZeroCopyArray => "zero-copy BGRA",
            InputPath::ZeroCopySingle => "zero-copy BGRA, one texture per frame",
            InputPath::SystemMemory => "NV12 via video processor",
        }
    }
}

/// The input path [`probe`] found working for each encoder name, used by [`GpuEncoder::open`].
/// Per process: sessions open the encoders probed on their own adapter.
fn probed_paths() -> &'static Mutex<HashMap<String, InputPath>> {
    static PATHS: OnceLock<Mutex<HashMap<String, InputPath>>> = OnceLock::new();
    PATHS.get_or_init(Default::default)
}

/// One encoded access unit.
pub struct Output {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Annex-B SPS/PPS, on the first output only.
    pub codec_config: Option<Vec<u8>>,
}

enum Inner {
    Zero(ZeroCopyEncoder),
    Nv12(Box<Nv12Path>),
}

/// Encoder bound to one D3D11 device.
pub struct GpuEncoder {
    inner: Inner,
    ctx: ID3D11DeviceContext,
    name: String,
    params: Vec<u8>,
    sent_config: bool,
}

impl GpuEncoder {
    /// True if `name` encodes straight from BGRA D3D11 frames.
    pub fn is_zero_copy(name: &str) -> bool {
        matches!(name, "h264_nvenc" | "h264_amf")
    }

    /// The input paths to try for `name`, best first.
    fn input_paths(name: &str) -> &'static [InputPath] {
        if Self::is_zero_copy(name) {
            &[
                InputPath::ZeroCopyArray,
                InputPath::ZeroCopySingle,
                InputPath::SystemMemory,
            ]
        } else {
            &[InputPath::SystemMemory]
        }
    }

    /// Opens `name` on `device` for `width x height` frames at `refresh_mhz`, on the input path
    /// [`probe`] found working (the best one if `name` was not probed).
    pub fn open(
        name: &str,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        bitrate_kbps: u32,
    ) -> Result<GpuEncoder, String> {
        let path = probed_paths()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .copied()
            .unwrap_or(Self::input_paths(name)[0]);
        Self::open_on(name, device, width, height, refresh_mhz, bitrate_kbps, path)
    }

    fn open_on(
        name: &str,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        bitrate_kbps: u32,
        path: InputPath,
    ) -> Result<GpuEncoder, String> {
        // SAFETY: plain COM call.
        let ctx = unsafe { device.GetImmediateContext() }
            .map_err(|e| format!("GetImmediateContext: {}", err_str(&e)))?;
        let (inner, extradata) = if path != InputPath::SystemMemory {
            // `into_raw` hands FFmpeg its own COM reference, which it releases when its device context dies.
            let raw: *mut c_void = device.clone().into_raw();
            let array_pool = path == InputPath::ZeroCopyArray;
            // SAFETY: `raw` is a valid ID3D11Device* and ownership of one reference moves to the call.
            let enc = unsafe {
                ZeroCopyEncoder::open(
                    name,
                    raw,
                    width,
                    height,
                    refresh_mhz,
                    bitrate_kbps,
                    array_pool,
                )
            }?;
            let extra = enc.extradata().to_vec();
            (Inner::Zero(enc), extra)
        } else {
            let mut cfg = EncoderConfig::new(name, width, height, refresh_mhz, bitrate_kbps);
            cfg.pix_fmt = RawPixelFormat::Nv12;
            let enc = Encoder::open(cfg).map_err(|e| e.to_string())?;
            let extra = enc.extradata().to_vec();
            let path = Nv12Path::new(device, &ctx, width, height, refresh_mhz, enc)?;
            (Inner::Nv12(Box::new(path)), extra)
        };
        let params = annexb::param_sets_from_extradata(&extradata).unwrap_or_default();
        Ok(GpuEncoder {
            inner,
            ctx,
            name: name.to_string(),
            params,
            sent_config: false,
        })
    }

    /// Encodes the BGRA texture `src`. `Ok(None)` if the encoder produced nothing yet.
    pub fn encode(
        &mut self,
        src: &ID3D11Texture2D,
        force_key: bool,
    ) -> Result<Option<Output>, String> {
        // The very first frame is always an IDR.
        let force = force_key || !self.sent_config;
        let packet = match &mut self.inner {
            Inner::Zero(enc) => {
                let (tex, index) = enc.acquire()?;
                // SAFETY: `tex` is the ID3D11Texture2D* of a live pool frame; borrowed only for this call.
                let dst = unsafe { ID3D11Texture2D::from_raw_borrowed(&tex) }
                    .ok_or("encoder returned a null texture")?;
                // SAFETY: both textures belong to the same device; `index` is the array slice of the frame.
                unsafe {
                    self.ctx
                        .CopySubresourceRegion(dst, index, 0, 0, 0, src, 0, None);
                    // Make the copy visible to the encoder's own submission queue.
                    self.ctx.Flush();
                }
                enc.submit(force)?;
                enc.receive()?.map(|p| (p.data, p.keyframe))
            }
            Inner::Nv12(path) => path.encode(src, force)?,
        };
        let Some((mut data, keyframe)) = packet else {
            return Ok(None);
        };
        if keyframe {
            data = annexb::with_param_sets(data, &self.params);
        }
        let mut codec_config = None;
        if !self.sent_config {
            let params = if self.params.is_empty() {
                annexb::extract_h264_param_sets(&data).unwrap_or_default()
            } else {
                self.params.clone()
            };
            if params.is_empty() {
                return Err(format!(
                    "{} produced no SPS/PPS (neither in extradata nor in the first frame)",
                    self.name
                ));
            }
            self.params = params.clone();
            codec_config = Some(params);
            self.sent_config = true;
        }
        Ok(Some(Output {
            data,
            keyframe,
            codec_config,
        }))
    }
}

/// BGRA -> NV12 (video processor) -> staging -> system memory -> `ezd_ffmpeg::Encoder`.
struct Nv12Path {
    ctx: ID3D11DeviceContext,
    vctx: ID3D11VideoContext,
    venum: ID3D11VideoProcessorEnumerator,
    vdev: ID3D11VideoDevice,
    vp: ID3D11VideoProcessor,
    out_tex: ID3D11Texture2D,
    out_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
    in_view: Option<(*mut c_void, ID3D11VideoProcessorInputView)>,
    enc: Encoder,
    width: u32,
    height: u32,
    raw: Vec<u8>,
}

// SAFETY: the COM objects are only used from the thread that owns the encoder (the device is
// multithread protected); the raw pointer in `in_view` is only an identity key, never dereferenced.
unsafe impl Send for Nv12Path {}

impl Nv12Path {
    fn new(
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        enc: Encoder,
    ) -> Result<Nv12Path, String> {
        let e = |what: &str, e: windows::core::Error| format!("{what}: {}", err_str(&e));
        let vdev: ID3D11VideoDevice = device.cast().map_err(|x| e("ID3D11VideoDevice", x))?;
        let vctx: ID3D11VideoContext = ctx.cast().map_err(|x| e("ID3D11VideoContext", x))?;
        let rate = DXGI_RATIONAL {
            Numerator: refresh_mhz,
            Denominator: 1000,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: width,
            InputHeight: height,
            OutputFrameRate: rate,
            OutputWidth: width,
            OutputHeight: height,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        // SAFETY: all descriptors are fully initialised and outlive the calls.
        unsafe {
            let venum = vdev
                .CreateVideoProcessorEnumerator(&content)
                .map_err(|x| e("CreateVideoProcessorEnumerator", x))?;
            let vp = vdev
                .CreateVideoProcessor(&venum, 0)
                .map_err(|x| e("CreateVideoProcessor", x))?;

            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut out_tex = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut out_tex))
                .map_err(|x| e("CreateTexture2D(NV12)", x))?;
            let out_tex = out_tex.ok_or("no NV12 texture")?;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .map_err(|x| e("CreateTexture2D(staging)", x))?;
            let staging = staging.ok_or("no staging texture")?;

            let ov = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut out_view = None;
            vdev.CreateVideoProcessorOutputView(&out_tex, &venum, &ov, Some(&mut out_view))
                .map_err(|x| e("CreateVideoProcessorOutputView", x))?;
            let out_view = out_view.ok_or("no output view")?;

            // Input: RGB, full range (0-255). Output: BT.709 YCbCr, studio range (16-235), which is what
            // the encoder is told in the VUI (colorspace bt709, range mpeg).
            // Bit layout: Usage:1, RGB_Range:1, YCbCr_Matrix:1, YCbCr_xvYCC:1, Nominal_Range:2.
            let in_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (1 << 2) | (2 << 4),
            };
            let out_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (1 << 2) | (1 << 4),
            };
            vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &in_cs);
            vctx.VideoProcessorSetOutputColorSpace(&vp, &out_cs);

            Ok(Nv12Path {
                ctx: ctx.clone(),
                vctx,
                venum,
                vdev,
                vp,
                out_tex,
                out_view,
                staging,
                in_view: None,
                enc,
                width,
                height,
                raw: vec![0; (width * height * 3 / 2) as usize],
            })
        }
    }

    fn encode(
        &mut self,
        src: &ID3D11Texture2D,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        let key = src.as_raw();
        if self.in_view.as_ref().is_none_or(|(k, _)| *k != key) {
            let iv = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut view = None;
            // SAFETY: valid resource, enumerator and descriptor.
            unsafe {
                self.vdev
                    .CreateVideoProcessorInputView(src, &self.venum, &iv, Some(&mut view))
            }
            .map_err(|e| format!("CreateVideoProcessorInputView: {}", err_str(&e)))?;
            self.in_view = Some((key, view.ok_or("no input view")?));
        }
        let view = self.in_view.as_ref().map(|(_, v)| v.clone());
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(view),
            ..Default::default()
        };
        // SAFETY: the stream array lives across the call; resources belong to our device. The
        // ManuallyDrop'd input view is released right after the blit.
        let blt = unsafe {
            let r = self.vctx.VideoProcessorBlt(
                &self.vp,
                &self.out_view,
                0,
                std::slice::from_ref(&stream),
            );
            let mut stream = stream;
            ManuallyDrop::drop(&mut stream.pInputSurface);
            r
        };
        blt.map_err(|e| format!("VideoProcessorBlt: {}", err_str(&e)))?;
        self.read_back()?;
        let p = self
            .enc
            .encode(&self.raw, force)
            .map_err(|e| e.to_string())?;
        Ok(p.map(|p| (p.data, p.keyframe)))
    }

    /// Copies the NV12 texture into `self.raw` (tightly packed Y plane then interleaved UV plane).
    fn read_back(&mut self) -> Result<(), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging is a CPU-readable texture; the mapping is released below before returning.
        unsafe {
            self.ctx.CopyResource(&self.staging, &self.out_tex);
            self.ctx
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .map_err(|e| format!("Map(staging): {}", err_str(&e)))?;
            let pitch = m.RowPitch as usize;
            let base = m.pData as *const u8;
            // NV12: the interleaved chroma plane follows the luma plane, `pitch * height` bytes in.
            for y in 0..h {
                std::ptr::copy_nonoverlapping(
                    base.add(y * pitch),
                    self.raw.as_mut_ptr().add(y * w),
                    w,
                );
            }
            let uv = base.add(pitch * h);
            for y in 0..h / 2 {
                std::ptr::copy_nonoverlapping(
                    uv.add(y * pitch),
                    self.raw.as_mut_ptr().add(w * h + y * w),
                    w,
                );
            }
            self.ctx.Unmap(&self.staging, 0);
        }
        Ok(())
    }
}

/// Opens `name` on `device` and encodes a black frame; the result is a short description.
///
/// The input paths of `name` are tried best first: some encoders open on D3D11 frames and then
/// fail the first one (AMF on older GPUs, with error 18). The first path that encodes is used by
/// later [`GpuEncoder::open`] calls.
pub fn probe(device: &ID3D11Device, name: &str, width: u32, height: u32) -> Result<String, String> {
    let tex = super::capture::create_bgra_texture(device, width, height)?;
    let mut failed = Vec::new();
    for &path in GpuEncoder::input_paths(name) {
        match probe_on(device, &tex, name, width, height, path) {
            Ok(note) => {
                probed_paths()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(name.to_string(), path);
                return Ok(if failed.is_empty() {
                    note
                } else {
                    format!("{note} (failed: {})", failed.join("; "))
                });
            }
            Err(e) => {
                tracing::warn!("{name}: {} input failed: {e}", path.describe());
                failed.push(format!("{}: {e}", path.describe()));
            }
        }
    }
    Err(failed.join("; "))
}

fn probe_on(
    device: &ID3D11Device,
    tex: &ID3D11Texture2D,
    name: &str,
    width: u32,
    height: u32,
    path: InputPath,
) -> Result<String, String> {
    let mut enc = GpuEncoder::open_on(name, device, width, height, 60_000, 8_000, path)?;
    for _ in 0..4 {
        if let Some(o) = enc.encode(tex, true)? {
            return Ok(format!(
                "encoded {} bytes, keyframe={}, parameter sets {} bytes, {}",
                o.data.len(),
                o.keyframe,
                o.codec_config.as_ref().map_or(0, Vec::len),
                path.describe()
            ));
        }
    }
    Err("opened, but no packet after 4 frames".into())
}
