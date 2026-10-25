//! GPU encoder front-end: takes the captured texture and produces H.264 (SDR) or HEVC Main10
//! (HDR10).
//!
//! SDR, the captured texture is BGRA:
//! * NVENC and AMF: zero-copy. The BGRA texture is copied (GPU to GPU) into a
//!   D3D11 frame of the encoder's pool (see [`super::ffenc`]).
//!   If the encoder rejects those frames, [`probe`] falls back to single textures and then to
//!   the NV12 path below.
//! * QSV and libx264: the texture is converted to NV12 by the D3D11 video
//!   processor, read back, and given to the regular `ezd_ffmpeg::Encoder`
//!   (QSV uploads the NV12 itself; FFmpeg's QSV encoder wants QSV surfaces or system memory).
//!
//! HDR10, the captured texture is FP16 scRGB and `hevc_*` encoders are used:
//! * NVENC and AMF: zero-copy. A shader ([`super::hdr::ScrgbToP010`]) renders PQ BT.2020 P010
//!   straight into the encoder's pool frame.
//! * QSV: the shader renders into a P010 texture that is read back and given to the regular
//!   `ezd_ffmpeg::Encoder` as P010, like the NV12 path above.

use super::ffenc::{StreamFormat, ZeroCopyEncoder};
use super::hdr::{ScrgbToP010, create_p010_texture};
use super::util::err_str;
use crate::annexb;
use ezd_ffmpeg::{ColorInfo, Encoder, EncoderConfig, HdrMeta, RawPixelFormat};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
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
    /// NV12 (P010 for HDR10) rendered on the GPU, read back into system memory.
    SystemMemory,
}

impl InputPath {
    fn describe(self, ten_bit: bool) -> &'static str {
        match (self, ten_bit) {
            (InputPath::ZeroCopyArray, false) => "zero-copy BGRA",
            (InputPath::ZeroCopySingle, false) => "zero-copy BGRA, one texture per frame",
            (InputPath::SystemMemory, false) => "NV12 via video processor",
            (InputPath::ZeroCopyArray, true) => "HDR10: zero-copy P010 (shader)",
            (InputPath::ZeroCopySingle, true) => {
                "HDR10: zero-copy P010 (shader), one texture per frame"
            }
            (InputPath::SystemMemory, true) => "HDR10: P010 (shader) via system memory",
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
    /// `hdr` is set for the 10-bit path: the source is FP16 scRGB and is converted into the frame.
    Zero {
        enc: ZeroCopyEncoder,
        hdr: Option<Box<ScrgbToP010>>,
    },
    Nv12(Box<Nv12Path>),
    P010(Box<P010Path>),
}

/// Encoder bound to one D3D11 device.
pub struct GpuEncoder {
    inner: Inner,
    ctx: ID3D11DeviceContext,
    name: String,
    params: Vec<u8>,
    sent_config: bool,
    /// A frame has been submitted (the very first one is forced to be an IDR).
    started: bool,
    hevc: bool,
    /// Time spent in the last [`push`](Self::push) or [`finish`](Self::finish): copy (or HDR
    /// render, or convert and read back) into the encoder's input, sending the frame, receiving
    /// the packet. Paths that do not split the work put it all in the middle entry. On the
    /// pipelined path the middle entry is the worker's encode time of the returned packet and the
    /// last one the time spent blocked waiting for it; the first is not updated by `finish`.
    pub stages: [Duration; 3],
}

impl GpuEncoder {
    /// True if `name` encodes straight from D3D11 frames (BGRA for H.264, P010 for HEVC).
    /// `hevc_qsv` is not: like `h264_qsv` it needs system memory (or QSV surfaces).
    pub fn is_zero_copy(name: &str) -> bool {
        matches!(name, "h264_nvenc" | "h264_amf" | "hevc_nvenc" | "hevc_amf")
    }

    /// The format an HDR10 stream (HEVC Main10, BT.2020 PQ) is encoded in.
    pub fn hdr10_format(hdr: Option<HdrMeta>) -> StreamFormat {
        StreamFormat {
            ten_bit: true,
            color: ColorInfo::BT2020_PQ_LIMITED,
            hdr,
        }
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
    /// [`probe`] found working (the best one if `name` was not probed). With a ten-bit
    /// `format`, [`encode`](Self::encode) takes FP16 scRGB textures instead of BGRA ones.
    pub fn open(
        name: &str,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        bitrate_kbps: u32,
        format: StreamFormat,
    ) -> Result<GpuEncoder, String> {
        let path = probed_paths()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .copied()
            .unwrap_or(Self::input_paths(name)[0]);
        tracing::info!("{name}: input path {}", path.describe(false));
        Self::open_on(
            name,
            device,
            width,
            height,
            refresh_mhz,
            bitrate_kbps,
            format,
            path,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_on(
        name: &str,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        bitrate_kbps: u32,
        format: StreamFormat,
        path: InputPath,
    ) -> Result<GpuEncoder, String> {
        let hevc = name.starts_with("hevc_");
        if format.ten_bit && !hevc {
            return Err(format!("{name} has no 10-bit HDR10 mode here"));
        }
        // SAFETY: plain COM call.
        let ctx = unsafe { device.GetImmediateContext() }
            .map_err(|e| format!("GetImmediateContext: {}", err_str(&e)))?;
        let (inner, extradata) = if path != InputPath::SystemMemory {
            let hdr = if format.ten_bit {
                Some(Box::new(ScrgbToP010::new(device, width, height)?))
            } else {
                None
            };
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
                    format,
                    array_pool,
                )
            }?;
            let extra = enc.extradata().to_vec();
            (Inner::Zero { enc, hdr }, extra)
        } else {
            let mut cfg = EncoderConfig::new(name, width, height, refresh_mhz, bitrate_kbps);
            cfg.pix_fmt = if format.ten_bit {
                RawPixelFormat::P010
            } else {
                RawPixelFormat::Nv12
            };
            cfg.color = format.color;
            cfg.hdr = format.hdr;
            let enc = Encoder::open(cfg).map_err(|e| e.to_string())?;
            let extra = enc.extradata().to_vec();
            if format.ten_bit {
                let path = P010Path::new(device, &ctx, width, height, enc)?;
                (Inner::P010(Box::new(path)), extra)
            } else {
                let path = Nv12Path::new(device, &ctx, width, height, refresh_mhz, enc)?;
                (Inner::Nv12(Box::new(path)), extra)
            }
        };
        let params = if hevc {
            annexb::hevc_param_sets_from_extradata(&extradata)
        } else {
            annexb::param_sets_from_extradata(&extradata)
        }
        .unwrap_or_default();
        Ok(GpuEncoder {
            inner,
            ctx,
            name: name.to_string(),
            params,
            sent_config: false,
            started: false,
            hevc,
            stages: [Duration::ZERO; 3],
        })
    }

    /// The FFmpeg encoder name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// True if the encoder works on a worker thread: [`push`](Self::push) then returns the packet
    /// of the previous frame and [`finish`](Self::finish) collects the last one.
    pub fn pipelined(&self) -> bool {
        matches!(self.inner, Inner::Nv12(_))
    }

    /// True if a pushed frame's packet has not been returned yet (pipelined path only).
    pub fn in_flight(&self) -> bool {
        matches!(&self.inner, Inner::Nv12(p) if p.in_flight)
    }

    /// Encodes the texture `src` and waits for its packet. `Ok(None)` if the encoder produced
    /// nothing yet.
    pub fn encode(
        &mut self,
        src: &ID3D11Texture2D,
        force_key: bool,
    ) -> Result<Option<Output>, String> {
        let out = self.push(src, force_key)?;
        if self.pipelined() {
            // For callers that encode one frame at a time, nothing was in flight before: `out` is
            // empty and the packet is this frame's.
            return self.finish();
        }
        Ok(out)
    }

    /// Waits for the frame in flight and returns its packet; `Ok(None)` if there is none.
    pub fn finish(&mut self) -> Result<Option<Output>, String> {
        let packet = match &mut self.inner {
            Inner::Nv12(path) => {
                let p = path.collect()?;
                self.stages[1] = path.times[1];
                self.stages[2] = path.times[2];
                p
            }
            _ => None,
        };
        self.finish_packet(packet)
    }

    /// Submits the texture `src` (BGRA, or FP16 scRGB for a ten-bit encoder).
    ///
    /// Synchronous paths return the packet of this frame, if the encoder produced one. The
    /// pipelined path converts and reads the frame back, waits for the frame that was in flight
    /// (if any) and returns its packet, and then hands this frame to the worker.
    pub fn push(
        &mut self,
        src: &ID3D11Texture2D,
        force_key: bool,
    ) -> Result<Option<Output>, String> {
        // The very first frame is always an IDR.
        let force = force_key || !self.started;
        self.started = true;
        let packet = match &mut self.inner {
            Inner::Zero { enc, hdr } => {
                let t0 = Instant::now();
                let (tex, index) = enc.acquire()?;
                // SAFETY: `tex` is the ID3D11Texture2D* of a live pool frame; borrowed only for this call.
                let dst = unsafe { ID3D11Texture2D::from_raw_borrowed(&tex) }
                    .ok_or("encoder returned a null texture")?;
                match hdr {
                    // Render PQ BT.2020 P010 into the frame.
                    Some(conv) => conv.convert(src, dst, index)?,
                    // SAFETY: both textures belong to the same device; `index` is the array slice of the frame.
                    None => unsafe {
                        self.ctx
                            .CopySubresourceRegion(dst, index, 0, 0, 0, src, 0, None)
                    },
                }
                // Make the copy or render visible to the encoder's own submission queue.
                // SAFETY: plain COM call.
                unsafe { self.ctx.Flush() };
                let t1 = Instant::now();
                enc.submit(force)?;
                let t2 = Instant::now();
                let p = enc.receive()?.map(|p| (p.data, p.keyframe));
                self.stages = [t1 - t0, t2 - t1, t2.elapsed()];
                p
            }
            Inner::Nv12(path) => {
                let p = path.push(src, force)?;
                self.stages = path.times;
                p
            }
            Inner::P010(path) => {
                let t0 = Instant::now();
                let p = path.encode(src, force)?;
                self.stages = [Duration::ZERO, t0.elapsed(), Duration::ZERO];
                p
            }
        };
        self.finish_packet(packet)
    }

    /// Adds the parameter sets to a keyframe and reports the codec config with the first packet.
    /// Packets arrive in encode order, so the first one is the first frame's.
    fn finish_packet(&mut self, packet: Option<(Vec<u8>, bool)>) -> Result<Option<Output>, String> {
        let Some((mut data, keyframe)) = packet else {
            return Ok(None);
        };
        if keyframe {
            data = if self.hevc {
                annexb::with_hevc_param_sets(data, &self.params)
            } else {
                annexb::with_param_sets(data, &self.params)
            };
        }
        let mut codec_config = None;
        if !self.sent_config {
            let params = if !self.params.is_empty() {
                self.params.clone()
            } else if self.hevc {
                annexb::extract_hevc_param_sets(&data).unwrap_or_default()
            } else {
                annexb::extract_h264_param_sets(&data).unwrap_or_default()
            };
            if params.is_empty() {
                return Err(format!(
                    "{} produced no {} (neither in extradata nor in the first frame)",
                    self.name,
                    if self.hevc { "VPS/SPS/PPS" } else { "SPS/PPS" }
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
    worker: EncodeWorker,
    width: u32,
    height: u32,
    /// The buffer the next frame is read back into.
    raw: Vec<u8>,
    /// The second buffer, while the worker does not hold it.
    spare: Option<Vec<u8>>,
    /// A frame was handed to the worker and its packet not collected yet.
    in_flight: bool,
    /// Convert and read back (calling thread), encode (worker), blocked waiting for the worker.
    /// The first is from the last [`push`](Self::push), the others from the last
    /// [`collect`](Self::collect).
    times: [Duration; 3],
}

/// A frame for the encode worker.
struct Job {
    raw: Vec<u8>,
    force: bool,
}

/// The worker's answer: the buffer back, the packet (data, keyframe) and the encode time.
struct Done {
    raw: Vec<u8>,
    packet: Result<Option<(Vec<u8>, bool)>, String>,
    took: Duration,
}

/// Runs `ezd_ffmpeg::Encoder` on its own thread, one frame at a time. Packets are never dropped:
/// every submitted frame is answered, in order.
struct EncodeWorker {
    jobs: Option<mpsc::Sender<Job>>,
    done: mpsc::Receiver<Done>,
    thread: Option<JoinHandle<()>>,
}

impl EncodeWorker {
    fn spawn(mut enc: Encoder) -> Result<EncodeWorker, String> {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel::<Done>();
        let thread = std::thread::Builder::new()
            .name("ezd-encode".into())
            .spawn(move || {
                // Ends when the sender is dropped.
                while let Ok(job) = job_rx.recv() {
                    let t0 = Instant::now();
                    let packet = enc
                        .encode(&job.raw, job.force)
                        .map(|p| p.map(|p| (p.data, p.keyframe)))
                        .map_err(|e| e.to_string());
                    let done = Done {
                        raw: job.raw,
                        packet,
                        took: t0.elapsed(),
                    };
                    if done_tx.send(done).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| format!("encode thread: {e}"))?;
        Ok(EncodeWorker {
            jobs: Some(jobs),
            done,
            thread: Some(thread),
        })
    }
}

impl Drop for EncodeWorker {
    fn drop(&mut self) {
        // Closing the channel ends the loop once the frame in flight (if any) is encoded.
        self.jobs = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// SAFETY: the COM objects are only used from the thread that owns the encoder (the device is
// multithread protected); the raw pointer in `in_view` is only an identity key, never dereferenced.
// The `Encoder` lives on the worker thread.
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

            let len = (width * height * 3 / 2) as usize;
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
                worker: EncodeWorker::spawn(enc)?,
                width,
                height,
                raw: vec![0; len],
                spare: Some(vec![0; len]),
                in_flight: false,
                times: [Duration::ZERO; 3],
            })
        }
    }

    /// Converts and reads back `src`, waits for the frame in flight (if any) and returns its
    /// packet, then hands the new frame to the worker.
    fn push(
        &mut self,
        src: &ID3D11Texture2D,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        let t0 = Instant::now();
        self.convert(src)?;
        self.times[0] = t0.elapsed();
        // The D3D11 work above overlapped the worker's encode of the previous frame.
        let prev = self.collect()?;
        let spare = self.spare.take().ok_or("encode buffer missing")?;
        let raw = std::mem::replace(&mut self.raw, spare);
        self.worker
            .jobs
            .as_ref()
            .and_then(|j| j.send(Job { raw, force }).ok())
            .ok_or("encode thread ended")?;
        self.in_flight = true;
        Ok(prev)
    }

    /// Waits for the frame in flight and returns its packet (`None` if there is none).
    fn collect(&mut self) -> Result<Option<(Vec<u8>, bool)>, String> {
        if !self.in_flight {
            self.times[1] = Duration::ZERO;
            self.times[2] = Duration::ZERO;
            return Ok(None);
        }
        let t0 = Instant::now();
        let done = self.worker.done.recv();
        self.times[2] = t0.elapsed();
        self.in_flight = false;
        let done = done.map_err(|_| "encode thread ended")?;
        self.spare = Some(done.raw);
        self.times[1] = done.took;
        done.packet
    }

    /// BGRA `src` -> NV12 -> `self.raw`.
    fn convert(&mut self, src: &ID3D11Texture2D) -> Result<(), String> {
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
        self.read_back()
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

/// P010 -> staging -> system memory -> `ezd_ffmpeg::Encoder` (HEVC Main10 on QSV).
struct P010Path {
    ctx: ID3D11DeviceContext,
    conv: ScrgbToP010,
    tex: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    enc: Encoder,
    width: u32,
    height: u32,
    raw: Vec<u8>,
}

// SAFETY: the COM objects are only used from the thread that owns the encoder (the device is
// multithread protected).
unsafe impl Send for P010Path {}

impl P010Path {
    fn new(
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        width: u32,
        height: u32,
        enc: Encoder,
    ) -> Result<P010Path, String> {
        let conv = ScrgbToP010::new(device, width, height)?;
        let tex = create_p010_texture(device, width, height)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid out-parameter.
        unsafe { tex.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        // SAFETY: valid descriptor.
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }
            .map_err(|e| format!("CreateTexture2D(P010 staging): {}", err_str(&e)))?;
        Ok(P010Path {
            ctx: ctx.clone(),
            conv,
            tex,
            staging: staging.ok_or("no staging texture")?,
            enc,
            width,
            height,
            raw: vec![0; RawPixelFormat::P010.frame_len(width, height)],
        })
    }

    fn encode(
        &mut self,
        src: &ID3D11Texture2D,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        self.conv.convert(src, &self.tex, 0)?;
        self.read_back()?;
        let p = self
            .enc
            .encode(&self.raw, force)
            .map_err(|e| e.to_string())?;
        Ok(p.map(|p| (p.data, p.keyframe)))
    }

    /// Copies the P010 texture into `self.raw`: the luma plane (`2 * width` bytes a row) then the
    /// interleaved chroma plane (`height / 2` rows of `2 * width` bytes), tightly packed.
    fn read_back(&mut self) -> Result<(), String> {
        let (row, h) = (self.width as usize * 2, self.height as usize);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging is a CPU-readable texture; the mapping is released below before returning.
        unsafe {
            self.ctx.CopyResource(&self.staging, &self.tex);
            self.ctx
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .map_err(|e| format!("Map(staging): {}", err_str(&e)))?;
            let pitch = m.RowPitch as usize;
            let base = m.pData as *const u8;
            // The chroma plane follows the luma plane, `pitch * height` bytes in.
            for y in 0..h {
                std::ptr::copy_nonoverlapping(
                    base.add(y * pitch),
                    self.raw.as_mut_ptr().add(y * row),
                    row,
                );
            }
            let uv = base.add(pitch * h);
            for y in 0..h / 2 {
                std::ptr::copy_nonoverlapping(
                    uv.add(y * pitch),
                    self.raw.as_mut_ptr().add(row * h + y * row),
                    row,
                );
            }
            self.ctx.Unmap(&self.staging, 0);
        }
        Ok(())
    }
}

/// Opens `name` on `device` and encodes a black frame; the result is a short description.
/// HEVC encoders are opened for HDR10 (Main10, BT.2020 PQ, with static metadata) and fed a black
/// FP16 scRGB frame, as in a real HDR session.
///
/// The input paths of `name` are tried best first: some encoders open on D3D11 frames and then
/// fail the first one (AMF on older GPUs, with error 18). The first path that encodes is used by
/// later [`GpuEncoder::open`] calls.
pub fn probe(device: &ID3D11Device, name: &str, width: u32, height: u32) -> Result<String, String> {
    let ten_bit = name.starts_with("hevc_");
    let (tex, format) = if ten_bit {
        let meta = HdrMeta {
            mastering: Some(ezd_proto::Mastering {
                primaries: [[35400, 14600], [8500, 39850], [6550, 2300]],
                white: [15635, 16450],
                max_luminance: 10_000_000,
                min_luminance: 1,
            }),
            max_cll: 1000,
            max_fall: 400,
        };
        (
            super::hdr::create_fp16_texture(device, width, height)?,
            GpuEncoder::hdr10_format(Some(meta)),
        )
    } else {
        (
            super::capture::create_bgra_texture(device, width, height)?,
            StreamFormat::SDR,
        )
    };
    let mut failed = Vec::new();
    for &path in GpuEncoder::input_paths(name) {
        let what = path.describe(ten_bit);
        match probe_on(device, &tex, name, width, height, format, path) {
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
                tracing::warn!("{name}: {what} input failed: {e}");
                failed.push(format!("{what}: {e}"));
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
    format: StreamFormat,
    path: InputPath,
) -> Result<String, String> {
    let ten_bit = format.ten_bit;
    let mut enc = GpuEncoder::open_on(name, device, width, height, 60_000, 8_000, format, path)?;
    for _ in 0..4 {
        if let Some(o) = enc.encode(tex, true)? {
            return Ok(format!(
                "encoded {} bytes, keyframe={}, parameter sets {} bytes, {}",
                o.data.len(),
                o.keyframe,
                o.codec_config.as_ref().map_or(0, Vec::len),
                path.describe(ten_bit)
            ));
        }
    }
    Err("opened, but no packet after 4 frames".into())
}
