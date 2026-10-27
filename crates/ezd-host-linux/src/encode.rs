//! Encode paths: VAAPI (DMA-BUF zero-copy or shared-memory upload), NVENC (DMA-BUF through Vulkan
//! and CUDA, or system-memory BGR0) and libx264, built on raw FFmpeg (`ezd_ffmpeg::sys`) so that
//! DRM-PRIME frames and filter graphs can be used. Option presets come from
//! `ezd_ffmpeg::option_sets`.

use crate::adapter::RenderNode;
use crate::frame::{DmaBuf, Frame, Pixels, ShmBuf, av_pix_fmt_name, drm_fourcc, is_ten_bit};
use crate::vkdma::VkDma;
use ezd_ffmpeg::ColorInfo;
use ezd_ffmpeg::HwDevice;
use ezd_ffmpeg::HwKind;
use ezd_ffmpeg::sys as ff;
use ezd_host_core::now_us;
use std::ffi::{CStr, CString, c_void};
use std::os::fd::IntoRawFd;
use std::ptr;
use std::sync::{Arc, Mutex};

/// The family of an encode path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// h264_vaapi on a render node.
    Vaapi,
    /// h264_nvenc: DMA-BUF imported through Vulkan and copied to CUDA, or system-memory input.
    Nvenc,
    /// libx264.
    X264,
}

/// One candidate encode path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// FFmpeg encoder name.
    pub name: &'static str,
    /// Path family.
    pub kind: Kind,
    /// Render node used (VAAPI, NVENC for description).
    pub node: Option<String>,
}

impl Plan {
    /// Hardware encoder?
    pub fn hardware(&self) -> bool {
        self.kind != Kind::X264
    }

    /// The HDR10 variant: HEVC Main10 (P010) instead of 8-bit H.264.
    pub fn hdr10(&self) -> bool {
        ten_bit_encoder(self.name)
    }
}

/// Encoders that only run the 10-bit HDR10 chain (there is no 8-bit HEVC plan).
fn ten_bit_encoder(name: &str) -> bool {
    name.starts_with("hevc_")
}

/// Chooses the HDR10 encoders for `node`: HEVC Main10, best first. Empty without a hardware
/// node: there is no software HDR path. `hevc_vaapi` always runs the full encoder (never
/// `low_power`): Coffee Lake and older have no low-power Main10.
pub fn plan_hdr_encoders(node: Option<&RenderNode>, works: &dyn Fn(&Plan) -> bool) -> Vec<Plan> {
    let Some(n) = node else {
        return Vec::new();
    };
    let plan = if n.is_nvidia() {
        Plan {
            name: "hevc_nvenc",
            kind: Kind::Nvenc,
            node: Some(n.path.clone()),
        }
    } else {
        Plan {
            name: "hevc_vaapi",
            kind: Kind::Vaapi,
            node: Some(n.path.clone()),
        }
    };
    if works(&plan) { vec![plan] } else { Vec::new() }
}

/// Chooses the encoders to offer for `node`, best first. `works` tells whether a plan was
/// verified on this machine. libx264 is always the last resort.
pub fn plan_encoders(node: Option<&RenderNode>, works: &dyn Fn(&Plan) -> bool) -> Vec<Plan> {
    let mut out = Vec::new();
    if let Some(n) = node {
        let primary = if n.is_nvidia() {
            Plan {
                name: "h264_nvenc",
                kind: Kind::Nvenc,
                node: Some(n.path.clone()),
            }
        } else {
            Plan {
                name: "h264_vaapi",
                kind: Kind::Vaapi,
                node: Some(n.path.clone()),
            }
        };
        if works(&primary) {
            out.push(primary);
        }
    }
    let x264 = Plan {
        name: "libx264",
        kind: Kind::X264,
        node: None,
    };
    if works(&x264) {
        out.push(x264);
    }
    out
}

fn averr(what: &str, code: i32) -> String {
    let mut buf = [0 as std::os::raw::c_char; 128];
    // SAFETY: valid buffer of the stated size, NUL-terminated by av_strerror.
    let text = unsafe {
        ff::av_strerror(code, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    };
    format!("{what}: {text} ({code})")
}

const EAGAIN: i32 = -11;

/// Owned `AVBufferRef`.
struct BufRef(*mut ff::AVBufferRef);

// SAFETY: AVBufferRef refcounting is atomic; users only touch it from the encoder thread.
unsafe impl Send for BufRef {}

impl BufRef {
    fn new_ref(&self) -> *mut ff::AVBufferRef {
        // SAFETY: self.0 is a valid buffer ref.
        unsafe { ff::av_buffer_ref(self.0) }
    }

    fn try_clone(&self) -> Result<BufRef, String> {
        let r = self.new_ref();
        if r.is_null() {
            return Err("av_buffer_ref failed".into());
        }
        Ok(BufRef(r))
    }
}

impl Drop for BufRef {
    fn drop(&mut self) {
        // SAFETY: we own one reference.
        unsafe { ff::av_buffer_unref(&mut self.0) };
    }
}

/// An `AVFrame` freed on drop.
struct OwnedFrame(*mut ff::AVFrame);

impl OwnedFrame {
    fn alloc() -> Result<Self, String> {
        // SAFETY: plain allocation.
        let p = unsafe { ff::av_frame_alloc() };
        if p.is_null() {
            Err("av_frame_alloc failed".into())
        } else {
            Ok(OwnedFrame(p))
        }
    }
}

impl Drop for OwnedFrame {
    fn drop(&mut self) {
        // SAFETY: owned frame, free resets the pointer.
        unsafe { ff::av_frame_free(&mut self.0) };
    }
}

/// Filter graph with one buffer source and one sink.
struct Graph {
    g: *mut ff::AVFilterGraph,
    src: *mut ff::AVFilterContext,
    sink: *mut ff::AVFilterContext,
}

// SAFETY: only used from the encoder thread.
unsafe impl Send for Graph {}

impl Drop for Graph {
    fn drop(&mut self) {
        // SAFETY: owned graph; frees all filters.
        unsafe { ff::avfilter_graph_free(&mut self.g) };
    }
}

struct GraphInput {
    pix_fmt: ff::AVPixelFormat,
    width: u32,
    height: u32,
    fps_mhz: u32,
    hw_frames: Option<*mut ff::AVBufferRef>,
}

impl Graph {
    /// Builds the chain `desc` (comma separated filters). `devices[i]` is the hardware device given
    /// to the i-th filter; filters beyond the slice's end get none.
    fn build(
        input: &GraphInput,
        desc: &str,
        devices: &[Option<*mut ff::AVBufferRef>],
    ) -> Result<Graph, String> {
        // SAFETY: standard libavfilter construction sequence; every pointer is checked and the
        // Graph's Drop frees the graph on any early return.
        unsafe {
            let g = ff::avfilter_graph_alloc();
            if g.is_null() {
                return Err("avfilter_graph_alloc failed".into());
            }
            let mut graph = Graph {
                g,
                src: ptr::null_mut(),
                sink: ptr::null_mut(),
            };
            let buffer = ff::avfilter_get_by_name(c"buffer".as_ptr());
            let buffersink = ff::avfilter_get_by_name(c"buffersink".as_ptr());
            if buffer.is_null() || buffersink.is_null() {
                return Err("libavfilter lacks buffer/buffersink".into());
            }
            graph.src = ff::avfilter_graph_alloc_filter(g, buffer, c"in".as_ptr());
            if graph.src.is_null() {
                return Err("cannot allocate buffer source".into());
            }
            let par = ff::av_buffersrc_parameters_alloc();
            if par.is_null() {
                return Err("av_buffersrc_parameters_alloc failed".into());
            }
            (*par).format = input.pix_fmt as i32;
            (*par).width = input.width as i32;
            (*par).height = input.height as i32;
            (*par).time_base = ff::AVRational {
                num: 1000,
                den: input.fps_mhz as i32,
            };
            if let Some(h) = input.hw_frames {
                (*par).hw_frames_ctx = h;
            }
            let r = ff::av_buffersrc_parameters_set(graph.src, par);
            ff::av_free(par as *mut c_void);
            if r < 0 {
                return Err(averr("av_buffersrc_parameters_set", r));
            }
            let r = ff::avfilter_init_str(graph.src, ptr::null());
            if r < 0 {
                return Err(averr("init buffer source", r));
            }
            let r = ff::avfilter_graph_create_filter(
                &mut graph.sink,
                buffersink,
                c"out".as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                g,
            );
            if r < 0 {
                return Err(averr("create buffersink", r));
            }
            // Build the chain by hand (comma separated `name=args`): hardware device references
            // must be attached before a filter is initialised, which the text parser cannot do.
            let mut prev = graph.src;
            for (i, part) in desc.split(',').enumerate() {
                let (name, args) = part.split_once('=').unwrap_or((part, ""));
                let cname = CString::new(name).map_err(|_| "bad filter name")?;
                let def = ff::avfilter_get_by_name(cname.as_ptr());
                if def.is_null() {
                    return Err(format!("filter {name:?} not available in this FFmpeg"));
                }
                let inst = CString::new(format!("f{i}")).unwrap();
                let f = ff::avfilter_graph_alloc_filter(g, def, inst.as_ptr());
                if f.is_null() {
                    return Err(format!("cannot allocate filter {name}"));
                }
                if let Some(Some(dev)) = devices.get(i) {
                    (*f).hw_device_ctx = ff::av_buffer_ref(*dev);
                }
                let cargs = CString::new(args).map_err(|_| "bad filter args")?;
                let r = ff::avfilter_init_str(
                    f,
                    if args.is_empty() {
                        ptr::null()
                    } else {
                        cargs.as_ptr()
                    },
                );
                if r < 0 {
                    return Err(averr(&format!("init filter {part:?}"), r));
                }
                let r = ff::avfilter_link(prev, 0, f, 0);
                if r < 0 {
                    return Err(averr(&format!("link filter {name}"), r));
                }
                prev = f;
            }
            let r = ff::avfilter_link(prev, 0, graph.sink, 0);
            if r < 0 {
                return Err(averr("link buffersink", r));
            }
            let r = ff::avfilter_graph_config(g, ptr::null_mut());
            if r < 0 {
                return Err(averr(&format!("configure filter graph {desc:?}"), r));
            }
            Ok(graph)
        }
    }

    fn sink_size(&self) -> (u32, u32) {
        // SAFETY: valid, configured sink.
        unsafe {
            (
                ff::av_buffersink_get_w(self.sink) as u32,
                ff::av_buffersink_get_h(self.sink) as u32,
            )
        }
    }

    /// Pushes `frame` (ownership of its references moves into the graph) and pulls one result.
    fn run(&mut self, frame: *mut ff::AVFrame) -> Result<Option<OwnedFrame>, String> {
        // SAFETY: src/sink are configured; `frame` is a valid frame whose refs the graph takes.
        unsafe {
            let r = ff::av_buffersrc_add_frame_flags(self.src, frame, 0);
            if r < 0 {
                return Err(averr("av_buffersrc_add_frame", r));
            }
            let out = OwnedFrame::alloc()?;
            let r = ff::av_buffersink_get_frame(self.sink, out.0);
            if r == EAGAIN {
                return Ok(None);
            }
            if r < 0 {
                return Err(averr("av_buffersink_get_frame", r));
            }
            Ok(Some(out))
        }
    }
}

/// Opened encoder context.
struct Enc {
    ctx: *mut ff::AVCodecContext,
    pkt: *mut ff::AVPacket,
    extradata: Vec<u8>,
}

// SAFETY: exclusively owned FFmpeg objects, moved between threads only as a whole.
unsafe impl Send for Enc {}

impl Drop for Enc {
    fn drop(&mut self) {
        // SAFETY: owned; the free functions reset the pointers.
        unsafe {
            ff::av_packet_free(&mut self.pkt);
            ff::avcodec_free_context(&mut self.ctx);
        }
    }
}

struct EncParams<'a> {
    name: &'a str,
    width: u32,
    height: u32,
    fps_mhz: u32,
    kbps: u32,
    pix_fmt: ff::AVPixelFormat,
    hw_frames: Option<*mut ff::AVBufferRef>,
    /// Colour signalling written to the stream (VUI).
    color: ColorInfo,
    /// 10-bit input (HEVC Main10 option sets).
    ten_bit: bool,
}

/// The FFmpeg enums of the H.273 codes of `c`; codes we do not stream stay unspecified.
fn av_color(
    c: ColorInfo,
) -> (
    ff::AVColorPrimaries,
    ff::AVColorTransferCharacteristic,
    ff::AVColorSpace,
    ff::AVColorRange,
) {
    use ff::AVColorPrimaries as P;
    use ff::AVColorRange as R;
    use ff::AVColorSpace as S;
    use ff::AVColorTransferCharacteristic as T;
    (
        match c.primaries {
            1 => P::AVCOL_PRI_BT709,
            9 => P::AVCOL_PRI_BT2020,
            _ => P::AVCOL_PRI_UNSPECIFIED,
        },
        match c.transfer {
            1 => T::AVCOL_TRC_BT709,
            16 => T::AVCOL_TRC_SMPTE2084,
            18 => T::AVCOL_TRC_ARIB_STD_B67,
            _ => T::AVCOL_TRC_UNSPECIFIED,
        },
        match c.matrix {
            1 => S::AVCOL_SPC_BT709,
            9 => S::AVCOL_SPC_BT2020_NCL,
            _ => S::AVCOL_SPC_UNSPECIFIED,
        },
        if c.full_range {
            R::AVCOL_RANGE_JPEG
        } else {
            R::AVCOL_RANGE_MPEG
        },
    )
}

impl Enc {
    fn open(p: &EncParams<'_>) -> Result<Enc, String> {
        let cname = CString::new(p.name).map_err(|_| "bad encoder name")?;
        // SAFETY: valid NUL-terminated name.
        let codec = unsafe { ff::avcodec_find_encoder_by_name(cname.as_ptr()) };
        if codec.is_null() {
            return Err(format!("encoder {} not available in this FFmpeg", p.name));
        }
        let mut last = String::from("no option set");
        for set in ezd_ffmpeg::option_sets_for(p.name, p.ten_bit) {
            match Self::try_open(codec, p, &set) {
                Ok(e) => return Ok(e),
                Err(e) => {
                    tracing::debug!("opening {} with {set:?} failed: {e}", p.name);
                    last = e;
                }
            }
        }
        Err(last)
    }

    fn try_open(
        codec: *const ff::AVCodec,
        p: &EncParams<'_>,
        preset: &[(&'static str, String)],
    ) -> Result<Enc, String> {
        // SAFETY: standard codec context setup, mirrored from `ezd_ffmpeg::Encoder`; the Enc's Drop
        // releases everything on early return.
        unsafe {
            let ctx = ff::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err("avcodec_alloc_context3 failed".into());
            }
            let mut enc = Enc {
                ctx,
                pkt: ptr::null_mut(),
                extradata: Vec::new(),
            };
            let bitrate = p.kbps as i64 * 1000;
            let fps = p.fps_mhz as f64 / 1000.0;
            let c = &mut *ctx;
            c.width = p.width as i32;
            c.height = p.height as i32;
            c.time_base = ff::AVRational {
                num: 1000,
                den: p.fps_mhz as i32,
            };
            c.framerate = ff::AVRational {
                num: p.fps_mhz as i32,
                den: 1000,
            };
            c.pix_fmt = p.pix_fmt;
            c.bit_rate = bitrate;
            c.rc_max_rate = bitrate;
            c.rc_buffer_size = ((bitrate as f64 * 2.0 / fps) as i32).max(1000);
            c.gop_size = i32::MAX;
            c.max_b_frames = 0;
            c.thread_count = 1;
            (c.color_primaries, c.color_trc, c.colorspace, c.color_range) = av_color(p.color);
            c.flags |= (ff::AV_CODEC_FLAG_GLOBAL_HEADER | ff::AV_CODEC_FLAG_LOW_DELAY) as i32;
            if let Some(h) = p.hw_frames {
                c.hw_frames_ctx = ff::av_buffer_ref(h);
            }
            let mut dict: *mut ff::AVDictionary = ptr::null_mut();
            for (k, v) in preset {
                let (ck, cv) = (CString::new(*k).unwrap(), CString::new(v.as_str()).unwrap());
                ff::av_dict_set(&mut dict, ck.as_ptr(), cv.as_ptr(), 0);
            }
            let r = ff::avcodec_open2(ctx, codec, &mut dict);
            ff::av_dict_free(&mut dict);
            if r < 0 {
                return Err(averr(&format!("avcodec_open2({})", p.name), r));
            }
            let c = &*ctx;
            if !c.extradata.is_null() && c.extradata_size > 0 {
                enc.extradata =
                    std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec();
            }
            enc.pkt = ff::av_packet_alloc();
            if enc.pkt.is_null() {
                return Err("av_packet_alloc failed".into());
            }
            Ok(enc)
        }
    }

    fn send(&mut self, frame: *const ff::AVFrame) -> Result<(), String> {
        // SAFETY: opened context, valid frame.
        let r = unsafe { ff::avcodec_send_frame(self.ctx, frame) };
        if r < 0 {
            return Err(averr("avcodec_send_frame", r));
        }
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<(Vec<u8>, bool)>, String> {
        // SAFETY: opened context; after success the packet buffer is readable until unref.
        unsafe {
            let r = ff::avcodec_receive_packet(self.ctx, self.pkt);
            if r == EAGAIN || r == -(i32::from_le_bytes(*b"EOF ")) {
                return Ok(None);
            }
            if r < 0 {
                return Err(averr("avcodec_receive_packet", r));
            }
            let p = &*self.pkt;
            let data = std::slice::from_raw_parts(p.data, p.size as usize).to_vec();
            let key = p.flags & ff::AV_PKT_FLAG_KEY != 0;
            ff::av_packet_unref(self.pkt);
            Ok(Some((data, key)))
        }
    }
}

unsafe extern "C" fn free_boxed_vec(opaque: *mut c_void, _data: *mut u8) {
    // SAFETY: `opaque` is the Box<Vec<u8>> leaked in `shm_frame`.
    drop(unsafe { Box::from_raw(opaque as *mut Vec<u8>) });
}

unsafe extern "C" fn free_drm_descriptor(opaque: *mut c_void, _data: *mut u8) {
    // SAFETY: `opaque` is the Box<ff::AVDRMFrameDescriptor> leaked in `drm_frame`; objects hold the
    // fds we own.
    let d = unsafe { Box::from_raw(opaque as *mut ff::AVDRMFrameDescriptor) };
    for o in &d.objects[..d.nb_objects.max(0) as usize] {
        if o.fd >= 0 {
            // SAFETY: fd was detached from an OwnedFd with into_raw_fd.
            unsafe { libc::close(o.fd) };
        }
    }
}

fn pix_fmt_of(spa: u32) -> Result<ff::AVPixelFormat, String> {
    use ff::AVPixelFormat::*;
    Ok(match av_pix_fmt_name(spa) {
        Some("bgr0") => AV_PIX_FMT_BGR0,
        Some("bgra") => AV_PIX_FMT_BGRA,
        Some("rgb0") => AV_PIX_FMT_RGB0,
        Some("rgba") => AV_PIX_FMT_RGBA,
        Some("0rgb") => AV_PIX_FMT_0RGB,
        Some("0bgr") => AV_PIX_FMT_0BGR,
        Some("argb") => AV_PIX_FMT_ARGB,
        Some("abgr") => AV_PIX_FMT_ABGR,
        Some("x2rgb10le") => AV_PIX_FMT_X2RGB10LE,
        Some("x2bgr10le") => AV_PIX_FMT_X2BGR10LE,
        _ => return Err(format!("unsupported pixel format {spa}")),
    })
}

/// Wraps shared-memory pixels in an `AVFrame` without copying.
fn shm_frame(w: u32, h: u32, shm: ShmBuf) -> Result<OwnedFrame, String> {
    let fmt = pix_fmt_of(shm.spa_format)?;
    let mut boxed = Box::new(shm.data);
    let (data, len) = (boxed.as_mut_ptr(), boxed.len());
    let opaque = Box::into_raw(boxed) as *mut c_void;
    let f = OwnedFrame::alloc()?;
    // SAFETY: `data`/`len` describe the Vec kept alive by the buffer ref created here; on failure the
    // Box is reclaimed.
    unsafe {
        let buf = ff::av_buffer_create(data, len, Some(free_boxed_vec), opaque, 0);
        if buf.is_null() {
            drop(Box::from_raw(opaque as *mut Vec<u8>));
            return Err("av_buffer_create failed".into());
        }
        let fr = &mut *f.0;
        fr.buf[0] = buf;
        fr.data[0] = data;
        fr.linesize[0] = shm.stride as i32;
        fr.format = fmt as i32;
        fr.width = w as i32;
        fr.height = h as i32;
    }
    Ok(f)
}

/// Wraps a DMA-BUF in a `DRM_PRIME` `AVFrame`. Plane fds are moved into the frame.
fn drm_frame(
    w: u32,
    h: u32,
    dma: DmaBuf,
    frames: &BufRef,
) -> Result<(OwnedFrame, crate::frame::Release), String> {
    let DmaBuf {
        spa_format,
        modifier,
        planes,
        release,
    } = dma;
    let fourcc = drm_fourcc(spa_format).ok_or("unsupported DRM format")?;
    if planes.is_empty() || planes.len() > 4 {
        return Err(format!("unsupported plane count {}", planes.len()));
    }
    // SAFETY: AVDRMFrameDescriptor is plain data; zero is a valid initial value.
    let mut d: Box<ff::AVDRMFrameDescriptor> = Box::new(unsafe { std::mem::zeroed() });
    d.nb_objects = planes.len() as i32;
    d.nb_layers = 1;
    d.layers[0].format = fourcc;
    d.layers[0].nb_planes = planes.len() as i32;
    for (i, p) in planes.into_iter().enumerate() {
        d.objects[i].size = p.size as usize;
        d.objects[i].format_modifier = modifier;
        d.layers[0].planes[i].object_index = i as i32;
        d.layers[0].planes[i].offset = p.offset as isize;
        d.layers[0].planes[i].pitch = p.pitch as isize;
        d.objects[i].fd = p.fd.into_raw_fd();
    }
    let data = &mut *d as *mut ff::AVDRMFrameDescriptor as *mut u8;
    let opaque = Box::into_raw(d) as *mut c_void;
    let f = OwnedFrame::alloc()?;
    // SAFETY: the descriptor lives until the buffer ref is released (free_drm_descriptor closes
    // the fds); on failure we reclaim it.
    unsafe {
        let buf = ff::av_buffer_create(
            data,
            std::mem::size_of::<ff::AVDRMFrameDescriptor>(),
            Some(free_drm_descriptor),
            opaque,
            0,
        );
        if buf.is_null() {
            free_drm_descriptor(opaque, data);
            return Err("av_buffer_create failed".into());
        }
        let fr = &mut *f.0;
        fr.buf[0] = buf;
        fr.data[0] = data;
        fr.format = ff::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
        fr.width = w as i32;
        fr.height = h as i32;
        fr.hw_frames_ctx = frames.new_ref();
    }
    Ok((f, release))
}

/// Result of encoding one frame.
#[derive(Debug)]
pub struct Output {
    /// Annex-B access unit.
    pub data: Vec<u8>,
    /// IDR?
    pub keyframe: bool,
    /// SPS/PPS to send with this frame (first frame and forced keyframes).
    pub codec_config: Option<Vec<u8>>,
    /// `now_us` when encoding finished.
    pub encoded_us: u64,
    /// Which input path produced it.
    pub path: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputKey {
    dma: bool,
    spa_format: u32,
    width: u32,
    height: u32,
}

/// Input stage of an opened pipeline.
struct Conv {
    key: InputKey,
    graph: Option<Graph>,
    drm_frames: Option<BufRef>,
    /// Maps DMA-BUFs into Vulkan before the graph (NVENC).
    vk_import: Option<Arc<VkImport>>,
    /// Keeps the hardware devices alive as long as the graph.
    _devices: Vec<BufRef>,
    _vaapi: Option<HwDevice>,
    /// Encoder input format and (even) size.
    pix_fmt: ff::AVPixelFormat,
    width: u32,
    height: u32,
    path: &'static str,
    /// Mark input frames BT.709 full range (scale_vulkan rejects unspecified colorspaces).
    tag_colorspace: bool,
}

/// Colour signalling of a stream: the H.273 codes written to the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTags {
    color: ColorInfo,
}

impl Default for StreamTags {
    /// BT.709 limited range, what 8-bit streams have always been tagged with.
    fn default() -> Self {
        StreamTags {
            color: ColorInfo::BT709_LIMITED,
        }
    }
}

impl From<&ezd_proto::StreamColor> for StreamTags {
    fn from(c: &ezd_proto::StreamColor) -> Self {
        StreamTags {
            color: ColorInfo::from(c),
        }
    }
}

/// A frame converted for the encoder.
pub struct Prepared {
    frame: OwnedFrame,
    /// Changes whenever the converter reopens; the encoder then reopens too.
    generation: u64,
    pix_fmt: ff::AVPixelFormat,
    width: u32,
    height: u32,
    path: &'static str,
    /// The input was a DMA-BUF.
    pub dma: bool,
    /// Capture time of the source frame.
    pub capture_us: u64,
    /// Capture counter of the source frame.
    pub seq: u64,
    /// `now_us` when the conversion finished.
    ready_us: u64,
    fence_us: u64,
    graph_us: u64,
    sync_us: u64,
    /// The capture buffer, while the conversion may still read it.
    release: Option<crate::frame::Release>,
}

// SAFETY: the frame is exclusively owned and moves between threads as a whole; FFmpeg buffer
// reference counts are atomic.
unsafe impl Send for Prepared {}

/// Converts captured frames to the encoder's input: DMA-BUF import or upload, then color
/// conversion.
pub struct Converter {
    plan: Plan,
    tags: StreamTags,
    fps_mhz: u32,
    active: Option<Conv>,
    generation: u64,
    pts: i64,
}

/// `scale_vaapi` conversion to NV12 with BT.709 limited range.
fn scale_vaapi(w: u32, h: u32, color: bool) -> String {
    let base = format!("scale_vaapi=w={w}:h={h}:format=nv12");
    if color {
        format!("{base}:out_color_matrix=bt709:out_range=tv")
    } else {
        base
    }
}

/// `scale_vaapi` variants to P010 with BT.2020 limited range, most complete first: with the
/// primaries and transfer tags of the output (FFmpeg 6.1 and newer have them), then only the
/// matrix and range, then the bare format.
fn scale_vaapi_p010(w: u32, h: u32) -> [String; 3] {
    let base = format!("scale_vaapi=w={w}:h={h}:format=p010");
    [
        format!(
            "{base}:out_color_matrix=bt2020nc:out_color_primaries=bt2020:out_color_transfer=smpte2084:out_range=tv"
        ),
        format!("{base}:out_color_matrix=bt2020nc:out_range=tv"),
        base,
    ]
}

/// The `scale_vaapi` variants to try for an output of `w`x`h`, best first.
fn scale_vaapi_variants(w: u32, h: u32, hdr: bool) -> Vec<String> {
    if hdr {
        scale_vaapi_p010(w, h).to_vec()
    } else {
        vec![scale_vaapi(w, h, true), scale_vaapi(w, h, false)]
    }
}

fn even(v: u32) -> u32 {
    v & !1
}

#[link(name = "va")]
unsafe extern "C" {
    fn vaSyncSurface(dpy: *mut c_void, surface: u32) -> i32;
}

/// Waits until the GPU has finished writing a VAAPI frame; other frames are already complete.
fn sync_vaapi(frame: &OwnedFrame) -> Result<(), String> {
    // SAFETY: a VAAPI frame keeps its VASurfaceID in data[3] and its frames context alive; the
    // device's hwctx is an AVVAAPIDeviceContext, whose first field is the VADisplay.
    unsafe {
        let f = &*frame.0;
        if f.format != ff::AVPixelFormat::AV_PIX_FMT_VAAPI as i32 || f.hw_frames_ctx.is_null() {
            return Ok(());
        }
        let fc = &*((*f.hw_frames_ctx).data as *const ff::AVHWFramesContext);
        let display = *((*fc.device_ctx).hwctx as *const *mut c_void);
        let r = vaSyncSurface(display, f.data[3] as usize as u32);
        if r != 0 {
            return Err(format!("vaSyncSurface failed ({r})"));
        }
    }
    Ok(())
}

impl Converter {
    /// New converter; nothing is opened until the first frame arrives.
    pub fn new(plan: Plan, fps_mhz: u32) -> Converter {
        Converter {
            plan,
            tags: StreamTags::default(),
            fps_mhz,
            active: None,
            generation: 0,
            pts: 0,
        }
    }

    /// Sets the colour signalling of the stream (default BT.709 limited range).
    pub fn with_tags(mut self, tags: StreamTags) -> Converter {
        self.tags = tags;
        self
    }

    /// Input path currently in use (`dmabuf`/`shm`), if started.
    pub fn path(&self) -> Option<&'static str> {
        self.active.as_ref().map(|a| a.path)
    }

    fn open(&self, frame: &Frame) -> Result<Conv, String> {
        let (w, h) = (even(frame.width), even(frame.height));
        let (dma, spa_format) = match &frame.pixels {
            Pixels::Dma(d) => (true, d.spa_format),
            Pixels::Shm(s) => (false, s.spa_format),
        };
        let key = InputKey {
            dma,
            spa_format,
            width: frame.width,
            height: frame.height,
        };
        let in_fmt = pix_fmt_of(spa_format)?;
        let hdr = self.plan.hdr10();
        if hdr && !is_ten_bit(spa_format) {
            return Err(format!(
                "the HDR10 encoder needs a 10-bit capture format, got {spa_format}"
            ));
        }
        let node = self.plan.node.clone().unwrap_or_default();
        match self.plan.kind {
            Kind::Vaapi => {
                let vaapi =
                    HwDevice::open(HwKind::Vaapi, Some(&node)).map_err(|e| e.to_string())?;
                let mut devices = Vec::new();
                let mut drm_frames = None;
                let (graph, path) = if dma {
                    // DRM device + frames context describing the imported buffers.
                    let (dev, frames) = drm_context(&node, in_fmt, frame.width, frame.height)?;
                    let input = GraphInput {
                        pix_fmt: ff::AVPixelFormat::AV_PIX_FMT_DRM_PRIME,
                        width: frame.width,
                        height: frame.height,
                        fps_mhz: self.fps_mhz,
                        hw_frames: Some(frames.0),
                    };
                    let mut last = String::new();
                    let mut g = None;
                    for scale in scale_vaapi_variants(w, h, hdr) {
                        let desc = format!("hwmap=derive_device=vaapi,{scale}");
                        match Graph::build(&input, &desc, &[]) {
                            Ok(x) => {
                                g = Some(x);
                                break;
                            }
                            Err(e) => last = e,
                        }
                    }
                    devices.push(dev);
                    drm_frames = Some(frames);
                    (g.ok_or(last)?, "vaapi-dmabuf")
                } else {
                    let input = GraphInput {
                        pix_fmt: in_fmt,
                        width: frame.width,
                        height: frame.height,
                        fps_mhz: self.fps_mhz,
                        hw_frames: None,
                    };
                    let mut last = String::new();
                    let mut g = None;
                    let software = if hdr {
                        format!(
                            "scale=w={w}:h={h}:out_color_matrix=bt2020nc:out_range=tv,format=p010le,hwupload"
                        )
                    } else {
                        format!(
                            "scale=w={w}:h={h}:out_color_matrix=bt709:out_range=tv,format=nv12,hwupload"
                        )
                    };
                    let descs = scale_vaapi_variants(w, h, hdr)
                        .into_iter()
                        .map(|scale| format!("hwupload,{scale}"))
                        .chain([software]);
                    for desc in descs {
                        match Graph::build(&input, &desc, &[Some(vaapi.as_ptr()); 3]) {
                            Ok(x) => {
                                g = Some(x);
                                break;
                            }
                            Err(e) => last = e,
                        }
                    }
                    (g.ok_or(last)?, "vaapi-shm")
                };
                let (gw, gh) = graph.sink_size();
                if (gw, gh) != (w, h) {
                    return Err(format!("filter output is {gw}x{gh}, expected {w}x{h}"));
                }
                Ok(Conv {
                    key,
                    graph: Some(graph),
                    drm_frames,
                    vk_import: None,
                    _devices: devices,
                    _vaapi: Some(vaapi),
                    pix_fmt: ff::AVPixelFormat::AV_PIX_FMT_VAAPI,
                    width: w,
                    height: h,
                    path,
                    tag_colorspace: false,
                })
            }
            Kind::Nvenc => {
                if dma {
                    // Buffers are mapped into Vulkan by the shared import (see VkImport), scaled
                    // there and copied to CUDA by hwupload. They stay RGB and NVENC converts them,
                    // as on the shared-memory path: since FFmpeg 9, scale_vulkan goes through
                    // swscale, which cannot convert RGB to YUV on Vulkan. HDR needs that conversion
                    // (to P010), so it takes shared memory.
                    if hdr {
                        return Err("NVENC HDR takes shared-memory frames".into());
                    }
                    let import = VkImport::get(&node, in_fmt, frame.width, frame.height)?;
                    let cu = import.cu.try_clone()?;
                    let input = GraphInput {
                        pix_fmt: ff::AVPixelFormat::AV_PIX_FMT_VULKAN,
                        width: frame.width,
                        height: frame.height,
                        fps_mhz: self.fps_mhz,
                        hw_frames: Some(import.vk_frames.0),
                    };
                    let desc = format!("scale_vulkan=w={w}:h={h},hwupload");
                    let graph = Graph::build(&input, &desc, &[Some(import.vk.0), Some(cu.0)])?;
                    let (gw, gh) = graph.sink_size();
                    if (gw, gh) != (w, h) {
                        return Err(format!("filter output is {gw}x{gh}, expected {w}x{h}"));
                    }
                    return Ok(Conv {
                        key,
                        graph: Some(graph),
                        drm_frames: Some(import.drm_frames.try_clone()?),
                        vk_import: Some(import),
                        _devices: vec![cu],
                        _vaapi: None,
                        pix_fmt: ff::AVPixelFormat::AV_PIX_FMT_CUDA,
                        width: w,
                        height: h,
                        path: "nvenc-dmabuf-rgb",
                        tag_colorspace: false,
                    });
                }
                if hdr {
                    let input = GraphInput {
                        pix_fmt: in_fmt,
                        width: frame.width,
                        height: frame.height,
                        fps_mhz: self.fps_mhz,
                        hw_frames: None,
                    };
                    let desc = format!(
                        "scale=w={w}:h={h}:out_color_matrix=bt2020nc:out_range=tv,format=p010le"
                    );
                    let graph = Graph::build(&input, &desc, &[])?;
                    return Ok(Conv {
                        key,
                        graph: Some(graph),
                        drm_frames: None,
                        vk_import: None,
                        _devices: Vec::new(),
                        _vaapi: None,
                        pix_fmt: ff::AVPixelFormat::AV_PIX_FMT_P010LE,
                        width: w,
                        height: h,
                        path: "nvenc-sysmem",
                        tag_colorspace: false,
                    });
                }
                Ok(Conv {
                    key,
                    graph: None,
                    drm_frames: None,
                    vk_import: None,
                    _devices: Vec::new(),
                    _vaapi: None,
                    pix_fmt: in_fmt,
                    width: w,
                    height: h,
                    path: "nvenc-sysmem",
                    tag_colorspace: false,
                })
            }
            Kind::X264 => {
                if dma {
                    return Err("libx264 path takes shared-memory frames".into());
                }
                let input = GraphInput {
                    pix_fmt: in_fmt,
                    width: frame.width,
                    height: frame.height,
                    fps_mhz: self.fps_mhz,
                    hw_frames: None,
                };
                let desc = format!(
                    "scale=w={w}:h={h}:flags=fast_bilinear:out_color_matrix=bt709:out_range=tv,format=yuv420p"
                );
                let graph = Graph::build(&input, &desc, &[])?;
                Ok(Conv {
                    key,
                    graph: Some(graph),
                    drm_frames: None,
                    vk_import: None,
                    _devices: Vec::new(),
                    _vaapi: None,
                    pix_fmt: ff::AVPixelFormat::AV_PIX_FMT_YUV420P,
                    width: w,
                    height: h,
                    path: "x264-swscale",
                    tag_colorspace: false,
                })
            }
        }
    }

    /// Converts `frame`, reopening when its format or size changed. With `sync`, waits until the
    /// GPU has finished and gives the capture buffer back at once; without, the buffer goes along
    /// with the result and is released after encoding.
    pub fn convert(&mut self, frame: Frame, sync: bool) -> Result<Option<Prepared>, String> {
        let key = InputKey {
            dma: frame.is_dma(),
            spa_format: match &frame.pixels {
                Pixels::Dma(d) => d.spa_format,
                Pixels::Shm(s) => s.spa_format,
            },
            width: frame.width,
            height: frame.height,
        };
        if self.active.as_ref().is_none_or(|a| a.key != key) {
            self.active = None;
            self.active = Some(self.open(&frame)?);
            self.generation += 1;
            tracing::info!(path = self.path(), plan = ?self.plan, "encode pipeline opened");
        }
        let a = self.active.as_mut().expect("opened above");
        let hdr = self.plan.hdr10();
        let (w, h) = (frame.width, frame.height);
        let (capture_us, seq, dma) = (frame.capture_us, frame.seq, key.dma);

        // Waits until the compositor's GPU has finished drawing into the buffer. The conversion
        // would wait for it anyway; waiting here splits the compositor's time from ours in the
        // "encode stages" trace.
        let t_fence = now_us();
        if let Pixels::Dma(d) = &frame.pixels
            && let Some(p) = d.planes.first()
        {
            use std::os::fd::AsRawFd;
            let mut pfd = libc::pollfd {
                fd: p.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd; the fd stays open for the call.
            unsafe { libc::poll(&mut pfd, 1, 1000) };
        }
        let fence_us = now_us() - t_fence;
        let (av, release) = match frame.pixels {
            Pixels::Dma(d) => {
                let (f, rel) = match &a.vk_import {
                    Some(i) => i.import(w, h, d)?,
                    None => {
                        let frames = a.drm_frames.as_ref().ok_or("no DRM frames context")?;
                        drm_frame(w, h, d, frames)?
                    }
                };
                if a.tag_colorspace {
                    // SAFETY: the frame is exclusively ours.
                    unsafe {
                        // The RGB -> YUV matrix follows the input frame's colorspace.
                        (*f.0).colorspace = if hdr {
                            ff::AVColorSpace::AVCOL_SPC_BT2020_NCL
                        } else {
                            ff::AVColorSpace::AVCOL_SPC_BT709
                        };
                        (*f.0).color_range = ff::AVColorRange::AVCOL_RANGE_JPEG;
                    }
                }
                (f, Some(rel))
            }
            Pixels::Shm(s) => (shm_frame(w, h, s)?, None),
        };
        if hdr {
            // The capture is PQ-encoded BT.2020 RGB; say so, so the conversion does not tone-map.
            // SAFETY: the frame is exclusively ours.
            unsafe {
                (*av.0).color_primaries = ff::AVColorPrimaries::AVCOL_PRI_BT2020;
                (*av.0).color_trc = ff::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084;
            }
        }
        // SAFETY: the frame is exclusively ours.
        unsafe {
            (*av.0).pts = self.pts;
        }
        self.pts += 1;
        let t_graph = now_us();
        let out = match a.graph.as_mut() {
            Some(g) => match g.run(av.0)? {
                Some(o) => o,
                None => return Ok(None),
            },
            None => av,
        };
        let t_sync = now_us();
        // CUDA output: the Vulkan->CUDA copy waits on scale_vulkan's semaphore and NVENC runs on
        // the same CUDA stream, but nothing here waits for them, so the GPU may still be reading
        // the DMA-BUF. Keep the buffer until the packet is out, as without `sync`.
        let release = if sync && a.pix_fmt != ff::AVPixelFormat::AV_PIX_FMT_CUDA {
            sync_vaapi(&out)?;
            // The GPU is done reading the capture buffer.
            drop(release);
            None
        } else {
            release
        };
        let ready_us = now_us();
        Ok(Some(Prepared {
            frame: out,
            generation: self.generation,
            pix_fmt: a.pix_fmt,
            width: a.width,
            height: a.height,
            path: a.path,
            dma,
            capture_us,
            seq,
            ready_us,
            fence_us,
            graph_us: t_sync - t_graph,
            sync_us: ready_us - t_sync,
            release,
        }))
    }
}

struct Active {
    generation: u64,
    enc: Enc,
    sent_config: bool,
}

/// Encodes [`Prepared`] frames. A new encoder is opened when the converter reopened; its first
/// packet is an IDR with codec config.
pub struct Encoder {
    name: &'static str,
    tags: StreamTags,
    fps_mhz: u32,
    kbps: u32,
    active: Option<Active>,
}

impl Encoder {
    /// New encoder; nothing is opened until the first frame arrives.
    pub fn new(name: &'static str, fps_mhz: u32, kbps: u32) -> Encoder {
        Encoder {
            name,
            tags: StreamTags::default(),
            fps_mhz,
            kbps,
            active: None,
        }
    }

    /// Sets the colour signalling of the stream (default BT.709 limited range).
    pub fn with_tags(mut self, tags: StreamTags) -> Encoder {
        self.tags = tags;
        self
    }

    /// Encodes `p`. The capture buffer, if `p` still holds it, is released once the packet is out.
    pub fn encode(&mut self, p: Prepared, force_key: bool) -> Result<Option<Output>, String> {
        let taken_us = now_us();
        if self
            .active
            .as_ref()
            .is_none_or(|a| a.generation != p.generation)
        {
            self.active = None;
            // SAFETY: valid frame; the frames context outlives the call.
            let hw = unsafe { (*p.frame.0).hw_frames_ctx };
            let enc = Enc::open(&EncParams {
                name: self.name,
                width: p.width,
                height: p.height,
                fps_mhz: self.fps_mhz,
                kbps: self.kbps,
                pix_fmt: p.pix_fmt,
                hw_frames: (!hw.is_null()).then_some(hw),
                color: self.tags.color,
                ten_bit: ten_bit_encoder(self.name),
            })?;
            self.active = Some(Active {
                generation: p.generation,
                enc,
                sent_config: false,
            });
        }
        let a = self.active.as_mut().expect("opened above");
        let want_key = force_key || !a.sent_config;
        let Prepared {
            frame,
            path,
            ready_us,
            fence_us,
            graph_us,
            sync_us,
            release,
            ..
        } = p;
        // SAFETY: `frame` is a valid frame we own.
        unsafe {
            let f = &mut *frame.0;
            if ten_bit_encoder(self.name) {
                // HDR10 frames carry the stream's colour tags (8-bit streams keep what the
                // conversion left, which the encoder ignores in favour of the context).
                (f.color_primaries, f.color_trc, f.colorspace, f.color_range) =
                    av_color(self.tags.color);
            }
            if want_key {
                f.pict_type = ff::AVPictureType::AV_PICTURE_TYPE_I;
                f.flags |= ff::AV_FRAME_FLAG_KEY;
            } else {
                f.pict_type = ff::AVPictureType::AV_PICTURE_TYPE_NONE;
                f.flags &= !(ff::AV_FRAME_FLAG_KEY);
            }
        }
        let t_send = now_us();
        a.enc.send(frame.0)?;
        drop(frame);
        let t_receive = now_us();
        let packet = a.enc.receive()?;
        let encoded_us = now_us();
        tracing::trace!(
            fence_us,
            graph_us,
            sync_us,
            wait_us = taken_us.saturating_sub(ready_us),
            send_us = t_receive - t_send,
            receive_us = encoded_us - t_receive,
            "encode stages"
        );
        // The GPU/CPU is done with the capture buffer once the packet is out.
        drop(release);
        let Some((data, keyframe)) = packet else {
            return Ok(None);
        };
        let codec_config = if want_key && !a.enc.extradata.is_empty() {
            Some(a.enc.extradata.clone())
        } else {
            None
        };
        a.sent_config = true;
        Ok(Some(Output {
            data,
            keyframe,
            codec_config,
            encoded_us,
            path,
        }))
    }
}

/// Converter and encoder in series on the calling thread.
pub struct Pipeline {
    conv: Converter,
    enc: Encoder,
}

impl Pipeline {
    /// New pipeline; nothing is opened until the first frame arrives.
    pub fn new(plan: Plan, fps_mhz: u32, kbps: u32) -> Pipeline {
        Pipeline {
            enc: Encoder::new(plan.name, fps_mhz, kbps),
            conv: Converter::new(plan, fps_mhz),
        }
    }

    /// Sets the colour signalling of the stream (default BT.709 limited range).
    pub fn with_tags(self, tags: StreamTags) -> Pipeline {
        Pipeline {
            enc: self.enc.with_tags(tags),
            conv: self.conv.with_tags(tags),
        }
    }

    /// Input path currently in use (`dmabuf`/`shm`), if started.
    #[cfg(test)]
    pub fn path(&self) -> Option<&'static str> {
        self.conv.path()
    }

    /// Converts and encodes `frame`. A new encoder is opened when the input format or size
    /// changes (the result then starts with an IDR and codec config). The PipeWire buffer behind
    /// a DMA-BUF frame is released after the encoder produced its packet.
    pub fn encode(&mut self, frame: Frame, force_key: bool) -> Result<Option<Output>, String> {
        match self.conv.convert(frame, false)? {
            Some(p) => self.enc.encode(p, force_key),
            None => Ok(None),
        }
    }
}

/// Device of `kind` derived from `src`, with an optional `key=1` option (e.g. Vulkan's
/// `disable_multiplane`).
fn derive_device(
    src: &BufRef,
    kind: ff::AVHWDeviceType,
    flag: Option<&CStr>,
) -> Result<BufRef, String> {
    // SAFETY: the options dictionary is freed on every path; the result is wrapped in BufRef.
    unsafe {
        let mut opts: *mut ff::AVDictionary = ptr::null_mut();
        if let Some(k) = flag {
            ff::av_dict_set(&mut opts, k.as_ptr(), c"1".as_ptr(), 0);
        }
        let mut dev = ptr::null_mut();
        let r = ff::av_hwdevice_ctx_create_derived_opts(&mut dev, kind, src.0, opts, 0);
        ff::av_dict_free(&mut opts);
        if r < 0 {
            return Err(averr(&format!("derive {kind:?} device"), r));
        }
        Ok(BufRef(dev))
    }
}

/// The default CUDA device.
fn cuda_device() -> Result<BufRef, String> {
    // SAFETY: standard hwdevice construction; the ref is wrapped in BufRef immediately.
    unsafe {
        let mut dev = ptr::null_mut();
        let r = ff::av_hwdevice_ctx_create(
            &mut dev,
            ff::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
            ptr::null(),
            ptr::null_mut(),
            0,
        );
        if r < 0 {
            return Err(averr("create CUDA device", r));
        }
        Ok(BufRef(dev))
    }
}

/// DRM device and a `DRM_PRIME` frames context for imported buffers.
fn drm_context(
    node: &str,
    pix_fmt: ff::AVPixelFormat,
    w: u32,
    h: u32,
) -> Result<(BufRef, BufRef), String> {
    let dev = drm_device(node)?;
    let frames = drm_frames(&dev, pix_fmt, w, h)?;
    Ok((dev, frames))
}

fn drm_device(node: &str) -> Result<BufRef, String> {
    let cnode = CString::new(node).map_err(|_| "bad device path")?;
    // SAFETY: standard hwdevice construction; the ref is wrapped in BufRef immediately.
    unsafe {
        let mut dev = ptr::null_mut();
        let r = ff::av_hwdevice_ctx_create(
            &mut dev,
            ff::AVHWDeviceType::AV_HWDEVICE_TYPE_DRM,
            cnode.as_ptr(),
            ptr::null_mut(),
            0,
        );
        if r < 0 {
            return Err(averr(&format!("create DRM device {node}"), r));
        }
        Ok(BufRef(dev))
    }
}

fn drm_frames(dev: &BufRef, pix_fmt: ff::AVPixelFormat, w: u32, h: u32) -> Result<BufRef, String> {
    // SAFETY: standard hwframes construction; the ref is wrapped in BufRef immediately.
    unsafe {
        let frames = ff::av_hwframe_ctx_alloc(dev.0);
        if frames.is_null() {
            return Err("av_hwframe_ctx_alloc failed".into());
        }
        let mut frames = BufRef(frames);
        let fc = &mut *((*frames.0).data as *mut ff::AVHWFramesContext);
        fc.format = ff::AVPixelFormat::AV_PIX_FMT_DRM_PRIME;
        fc.sw_format = pix_fmt;
        fc.width = w as i32;
        fc.height = h as i32;
        let r = ff::av_hwframe_ctx_init(frames.0);
        if r < 0 {
            ff::av_buffer_unref(&mut frames.0);
            frames.0 = ptr::null_mut();
            return Err(averr("av_hwframe_ctx_init(DRM)", r));
        }
        Ok(frames)
    }
}

/// Imports DMA-BUFs of one node, format and size into Vulkan. Shared by every converter in the
/// process and never freed.
///
/// Single-plane buffers are imported by [`VkDma`] with a dedicated allocation, which FFmpeg's map
/// does not use (see `vkdma`). Other buffers go through FFmpeg's map.
///
/// FFmpeg (n8.1, and master as of October 2026) never frees a Vulkan frames context that a DRM
/// frame was mapped into. `vulkan_map_from_drm_frame_sync` leaves the mapped frame as a dependency
/// of a command buffer in the context's own pool, that pool is only released with the context, and
/// the frame references the context. The Vulkan device goes with it: about 40 fds and 65 MB per
/// converter on NVIDIA, and vkCreateDevice crashed after about 16. One shared context per format
/// bounds that to the formats a process sees. Until their command buffers are reused, the last few
/// mapped buffers stay referenced.
struct VkImport {
    /// CUDA device that NVENC encodes on.
    cu: BufRef,
    /// Vulkan device derived from `cu`, with single-plane images.
    vk: BufRef,
    /// Functions of `vk` for the dedicated imports.
    dma: Arc<VkDma>,
    /// Image format of `vk_frames`.
    vk_format: ash::vk::Format,
    /// `DRM_PRIME` frames context that captured buffers are described with.
    drm_frames: BufRef,
    /// Vulkan frames context the buffers are mapped into.
    vk_frames: BufRef,
    /// The command buffers of `vk_frames` are shared by all its users.
    map_lock: Mutex<()>,
}

// SAFETY: the references are only cloned (atomic refcounts), `vk_frames` is only mapped into under
// `map_lock`, and dedicated imports create objects without touching shared state; FFmpeg's Vulkan
// device locks its queues itself.
unsafe impl Sync for VkImport {}

impl VkImport {
    /// The import for frames of `pix_fmt` at `w`x`h` on `node`, created on first use. All imports
    /// of a node share one Vulkan device.
    fn get(
        node: &str,
        pix_fmt: ff::AVPixelFormat,
        w: u32,
        h: u32,
    ) -> Result<Arc<VkImport>, String> {
        type Key = (String, i32, u32, u32);
        struct Cache {
            /// Node, its DRM device, the CUDA device, the Vulkan device derived from CUDA and its
            /// functions.
            devices: Vec<(String, BufRef, BufRef, BufRef, Arc<VkDma>)>,
            imports: Vec<(Key, Arc<VkImport>)>,
        }
        static CACHE: Mutex<Cache> = Mutex::new(Cache {
            devices: Vec::new(),
            imports: Vec::new(),
        });
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let key = (node.to_string(), pix_fmt as i32, w, h);
        if let Some((_, i)) = cache.imports.iter().find(|(k, _)| *k == key) {
            return Ok(i.clone());
        }
        if !cache.devices.iter().any(|(n, ..)| n == node) {
            let dev = drm_device(node)?;
            // Vulkan is derived from CUDA, not the other way round: since FFmpeg 9 a CUDA device
            // derived from Vulkan cannot push its context, and the copy to CUDA fails.
            let cu = cuda_device()?;
            let vk = derive_device(
                &cu,
                ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
                Some(c"disable_multiplane"),
            )?;
            // SAFETY: a Vulkan device context, kept alive by the cache for the whole process.
            let dma = Arc::new(unsafe { VkDma::new(vk.0) }?);
            cache.devices.push((node.to_string(), dev, cu, vk, dma));
        }
        let (_, dev, cu, vk, dma) = cache
            .devices
            .iter()
            .find(|(n, ..)| n == node)
            .expect("added above");
        let drm = drm_frames(dev, pix_fmt, w, h)?;
        // A plain context, not one derived from `drm`: filters cannot allocate their output from a
        // derived one since FFmpeg 9 (ENOMEM). Mapping into it works the same.
        // SAFETY: valid device ref; the context is filled before init and wrapped in BufRef.
        let vk_frames = unsafe { ff::av_hwframe_ctx_alloc(vk.0) };
        if vk_frames.is_null() {
            return Err("av_hwframe_ctx_alloc failed".into());
        }
        let vk_frames = BufRef(vk_frames);
        // SAFETY: `vk_frames` is a fresh, uninitialized frames context.
        let r = unsafe {
            let fc = &mut *((*vk_frames.0).data as *mut ff::AVHWFramesContext);
            fc.format = ff::AVPixelFormat::AV_PIX_FMT_VULKAN;
            fc.sw_format = pix_fmt;
            fc.width = w as i32;
            fc.height = h as i32;
            ff::av_hwframe_ctx_init(vk_frames.0)
        };
        if r < 0 {
            return Err(averr("create Vulkan frames", r));
        }
        // SAFETY: lookup in a static table; null for formats Vulkan cannot hold.
        let vk_format = unsafe { ff::av_vkfmt_from_pixfmt(pix_fmt) };
        if vk_format.is_null() {
            return Err(format!("no Vulkan format for {pix_fmt:?}"));
        }
        let import = Arc::new(VkImport {
            cu: cu.try_clone()?,
            vk: vk.try_clone()?,
            dma: dma.clone(),
            // SAFETY: non-null pointer into FFmpeg's static table.
            vk_format: ash::vk::Format::from_raw(unsafe { *vk_format } as i32),
            drm_frames: drm,
            vk_frames,
            map_lock: Mutex::new(()),
        });
        cache.imports.push((key, import.clone()));
        Ok(import)
    }

    /// Imports `dma`, a `w`x`h` buffer of this import's format, as a Vulkan frame of `vk_frames`
    /// for reading. The capture buffer's release comes back with it.
    fn import(
        &self,
        w: u32,
        h: u32,
        dma: DmaBuf,
    ) -> Result<(OwnedFrame, crate::frame::Release), String> {
        if dma.planes.len() != 1 {
            let (f, rel) = drm_frame(w, h, dma, &self.drm_frames)?;
            return Ok((self.map(&f, ff::AV_HWFRAME_MAP_READ as i32)?, rel));
        }
        let f = OwnedFrame::alloc()?;
        // SAFETY: `f` is a fresh frame; `vk_frames` holds images of `vk_format` on the device the
        // functions were loaded from.
        unsafe {
            self.dma
                .import(f.0, w, h, self.vk_format, dma.modifier, &dma.planes[0])?;
            (*f.0).hw_frames_ctx = self.vk_frames.new_ref();
            if (*f.0).hw_frames_ctx.is_null() {
                return Err("av_buffer_ref failed".into());
            }
        }
        Ok((f, dma.release))
    }

    /// Maps `src`, a frame of `drm_frames`, onto a Vulkan image with `flags` (`AV_HWFRAME_MAP_*`)
    /// and copies its properties. The result keeps `src`'s buffer referenced.
    fn map(&self, src: &OwnedFrame, flags: i32) -> Result<OwnedFrame, String> {
        let dst = OwnedFrame::alloc()?;
        let _guard = self.map_lock.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `dst` is a fresh frame we own, `src` a valid DRM_PRIME frame of `drm_frames`.
        unsafe {
            (*dst.0).format = ff::AVPixelFormat::AV_PIX_FMT_VULKAN as i32;
            (*dst.0).hw_frames_ctx = self.vk_frames.new_ref();
            if (*dst.0).hw_frames_ctx.is_null() {
                return Err("av_buffer_ref failed".into());
            }
            let r = ff::av_hwframe_map(dst.0, src.0, flags);
            if r < 0 {
                return Err(averr("map DMA-BUF into Vulkan", r));
            }
            let r = ff::av_frame_copy_props(dst.0, src.0);
            if r < 0 {
                return Err(averr("av_frame_copy_props", r));
            }
        }
        Ok(dst)
    }
}

/// Exports a VAAPI surface filled with `rgba_pattern` as a DMA-BUF `Frame`, for self-tests and
/// diagnostics of the zero-copy path. The pattern is `(r, g, b)` per quadrant of a BGRx image.
#[cfg(test)]
pub fn export_test_dmabuf(
    node: &str,
    w: u32,
    h: u32,
    quadrants: [(u8, u8, u8); 4],
) -> Result<Frame, String> {
    use crate::frame::spa_fmt;
    export_dmabuf(
        node,
        (w, h),
        ff::AVPixelFormat::AV_PIX_FMT_BGR0,
        &[spa_fmt::BGRX, spa_fmt::BGRA, spa_fmt::RGBX, spa_fmt::RGBA],
        &|q| {
            let (r, g, b) = quadrants[q];
            [b, g, r, 0]
        },
    )
}

/// Like [`export_test_dmabuf`] for a 10-bit xRGB surface; the pattern is `(r, g, b)` code values
/// per quadrant.
#[cfg(test)]
pub fn export_test_dmabuf_10(
    node: &str,
    w: u32,
    h: u32,
    quadrants: [(u16, u16, u16); 4],
) -> Result<Frame, String> {
    export_dmabuf(
        node,
        (w, h),
        ff::AVPixelFormat::AV_PIX_FMT_X2RGB10LE,
        &crate::frame::OFFERED_FORMATS_10,
        &|q| {
            let (r, g, b) = quadrants[q];
            let word =
                (u32::from(r & 0x3ff) << 20) | (u32::from(g & 0x3ff) << 10) | u32::from(b & 0x3ff);
            word.to_le_bytes()
        },
    )
}

/// Exports a VAAPI surface of software format `sw_fmt` as a DMA-BUF `Frame`; `pixel` gives the
/// four bytes of a pixel for each quadrant (0 top left, 1 top right, 2 bottom left, 3 bottom
/// right) and the exported fourcc must be one of `spa_candidates`.
#[cfg(test)]
fn export_dmabuf(
    node: &str,
    (w, h): (u32, u32),
    sw_fmt: ff::AVPixelFormat,
    spa_candidates: &[u32],
    pixel: &dyn Fn(usize) -> [u8; 4],
) -> Result<Frame, String> {
    let dev = HwDevice::open(HwKind::Vaapi, Some(node)).map_err(|e| e.to_string())?;
    // SAFETY: construction of a VAAPI frames context, upload of a software frame and export through
    // av_hwframe_map; all objects are freed on every path by the wrappers.
    unsafe {
        let frames = ff::av_hwframe_ctx_alloc(dev.as_ptr());
        if frames.is_null() {
            return Err("av_hwframe_ctx_alloc failed".into());
        }
        let mut frames = BufRef(frames);
        let fc = &mut *((*frames.0).data as *mut ff::AVHWFramesContext);
        fc.format = ff::AVPixelFormat::AV_PIX_FMT_VAAPI;
        fc.sw_format = sw_fmt;
        fc.width = w as i32;
        fc.height = h as i32;
        fc.initial_pool_size = 2;
        let r = ff::av_hwframe_ctx_init(frames.0);
        if r < 0 {
            ff::av_buffer_unref(&mut frames.0);
            frames.0 = ptr::null_mut();
            return Err(averr(&format!("av_hwframe_ctx_init(VAAPI {sw_fmt:?})"), r));
        }
        // Software source.
        let sw = OwnedFrame::alloc()?;
        (*sw.0).format = sw_fmt as i32;
        (*sw.0).width = w as i32;
        (*sw.0).height = h as i32;
        let r = ff::av_frame_get_buffer(sw.0, 0);
        if r < 0 {
            return Err(averr("av_frame_get_buffer", r));
        }
        for y in 0..h as usize {
            let row = (*sw.0).data[0].add(y * (*sw.0).linesize[0] as usize);
            for x in 0..w as usize {
                let q = (y >= h as usize / 2) as usize * 2 + (x >= w as usize / 2) as usize;
                let px = row.add(x * 4);
                std::ptr::copy_nonoverlapping(pixel(q).as_ptr(), px, 4);
            }
        }
        let hw = OwnedFrame::alloc()?;
        let r = ff::av_hwframe_get_buffer(frames.0, hw.0, 0);
        if r < 0 {
            return Err(averr("av_hwframe_get_buffer", r));
        }
        let r = ff::av_hwframe_transfer_data(hw.0, sw.0, 0);
        if r < 0 {
            return Err(averr("upload to VAAPI surface", r));
        }
        // Export as DRM PRIME.
        let drm = OwnedFrame::alloc()?;
        (*drm.0).format = ff::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
        let r = ff::av_hwframe_map(drm.0, hw.0, ff::AV_HWFRAME_MAP_READ as i32);
        if r < 0 {
            return Err(averr("map VAAPI surface to DRM PRIME", r));
        }
        let d = &*((*drm.0).data[0] as *const ff::AVDRMFrameDescriptor);
        if d.nb_layers < 1 {
            return Err("exported frame has no layers".into());
        }
        let layer = &d.layers[0];
        let mut planes = Vec::new();
        for i in 0..layer.nb_planes as usize {
            let p = &layer.planes[i];
            let o = &d.objects[p.object_index as usize];
            let fd = libc::fcntl(o.fd, libc::F_DUPFD_CLOEXEC, 0);
            if fd < 0 {
                return Err("dup of exported dma-buf failed".into());
            }
            planes.push(crate::frame::Plane {
                fd: std::os::fd::FromRawFd::from_raw_fd(fd),
                size: o.size as u64,
                offset: p.offset as u32,
                pitch: p.pitch as u32,
            });
        }
        let modifier = d.objects[0].format_modifier;
        let fourcc = layer.format;
        let spa_format = spa_candidates
            .iter()
            .copied()
            .find(|f| drm_fourcc(*f) == Some(fourcc))
            .ok_or_else(|| format!("exported fourcc {fourcc:#x} not supported"))?;
        tracing::debug!(
            modifier = format_args!("{modifier:#x}"),
            fourcc = format_args!("{fourcc:#x}"),
            "exported test dma-buf"
        );
        Ok(Frame {
            width: w,
            height: h,
            capture_us: now_us(),
            seq: 0,
            pixels: Pixels::Dma(DmaBuf {
                spa_format,
                modifier,
                planes,
                release: crate::frame::Release(None),
            }),
        })
    }
}

/// A BGRx test image with four colored quadrants, as a shared-memory frame.
pub fn test_shm_frame(w: u32, h: u32, quadrants: [(u8, u8, u8); 4]) -> Frame {
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let q = (y >= h as usize / 2) as usize * 2 + (x >= w as usize / 2) as usize;
            let (r, g, b) = quadrants[q];
            let i = (y * w as usize + x) * 4;
            data[i..i + 4].copy_from_slice(&[b, g, r, 0]);
        }
    }
    Frame {
        width: w,
        height: h,
        capture_us: now_us(),
        seq: 0,
        pixels: Pixels::Shm(ShmBuf {
            spa_format: crate::frame::spa_fmt::BGRX,
            stride: w as usize * 4,
            data,
        }),
    }
}

/// An xRGB_210LE test image with four colored quadrants (`(r, g, b)` in 10-bit code values), as
/// a shared-memory frame: what a compositor sends for an HDR10 output.
pub fn test_shm_frame_10(w: u32, h: u32, quadrants: [(u16, u16, u16); 4]) -> Frame {
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let q = (y >= h as usize / 2) as usize * 2 + (x >= w as usize / 2) as usize;
            let (r, g, b) = quadrants[q];
            let word =
                (u32::from(r & 0x3ff) << 20) | (u32::from(g & 0x3ff) << 10) | u32::from(b & 0x3ff);
            let i = (y * w as usize + x) * 4;
            data[i..i + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
    Frame {
        width: w,
        height: h,
        capture_us: now_us(),
        seq: 0,
        pixels: Pixels::Shm(ShmBuf {
            spa_format: crate::frame::spa_fmt10::XRGB_210LE,
            stride: w as usize * 4,
            data,
        }),
    }
}

/// Probes whether `plan` can encode a shared-memory frame through the real pipeline.
pub fn probe_plan(plan: &Plan) -> Result<(), String> {
    let (mut p, f) = if plan.hdr10() {
        let tags = StreamTags::from(&ezd_proto::StreamColor::hdr10(None, 0, 0));
        (
            Pipeline::new(plan.clone(), 60_000, 8_000).with_tags(tags),
            test_shm_frame_10(
                1280,
                720,
                [(940, 64, 64), (64, 940, 64), (64, 64, 940), (940, 940, 940)],
            ),
        )
    } else {
        (
            Pipeline::new(plan.clone(), 60_000, 8_000),
            test_shm_frame(
                1280,
                720,
                [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)],
            ),
        )
    };
    match p.encode(f, true)? {
        Some(o) if o.keyframe && !o.data.is_empty() => Ok(()),
        Some(_) => Err("first packet was not a keyframe".into()),
        None => Err("no packet produced".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::RenderNode;
    use crate::gbm_test::{Bo, DRM_FORMAT_MOD_LINEAR, Gbm};
    use ezd_ffmpeg::Decoder;
    use ezd_proto::Codec;
    use std::os::fd::OwnedFd;

    fn node(path: &str, vendor: u16) -> RenderNode {
        RenderNode {
            path: path.into(),
            vendor,
            driver: "x".into(),
            pci: String::new(),
            compositor: false,
        }
    }

    #[test]
    fn plan_selection() {
        let all = |_: &Plan| true;
        let intel = plan_encoders(Some(&node("/dev/dri/renderD129", 0x8086)), &all);
        assert_eq!(
            intel.iter().map(|p| p.name).collect::<Vec<_>>(),
            ["h264_vaapi", "libx264"]
        );
        assert_eq!(intel[0].node.as_deref(), Some("/dev/dri/renderD129"));
        assert!(intel[0].hardware() && !intel[1].hardware());
        let nv = plan_encoders(Some(&node("/dev/dri/renderD128", 0x10de)), &all);
        assert_eq!(nv[0].name, "h264_nvenc");
        // A broken hardware path leaves only x264; nothing works -> empty.
        let sw = plan_encoders(Some(&node("/dev/dri/renderD129", 0x8086)), &|p| {
            p.kind == Kind::X264
        });
        assert_eq!(sw.len(), 1);
        assert!(plan_encoders(None, &|_| false).is_empty());
        assert_eq!(plan_encoders(None, &all)[0].name, "libx264");
    }

    fn decode_first(out: &Output) -> ezd_ffmpeg::DecodedFrame {
        let mut d = Decoder::new(Codec::H264).unwrap();
        if let Some(c) = &out.codec_config {
            d.push_config(c).unwrap();
        }
        let mut frames = d.decode(&out.data, 0).unwrap();
        frames.extend(d.flush().unwrap());
        frames.into_iter().next().expect("decoded a picture")
    }

    fn rgb_at(f: &ezd_ffmpeg::DecodedFrame, x: u32, y: u32) -> [u8; 3] {
        let rgb = f.to_rgb24();
        let i = ((y * f.width + x) * 3) as usize;
        [rgb[i], rgb[i + 1], rgb[i + 2]]
    }

    fn near(a: [u8; 3], b: [u8; 3]) -> bool {
        a.iter()
            .zip(b)
            .all(|(x, y)| (*x as i32 - y as i32).abs() < 40)
    }

    const Q: [(u8, u8, u8); 4] = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (240, 240, 240)];

    fn check_colors(f: &ezd_ffmpeg::DecodedFrame) {
        let (w, h) = (f.width, f.height);
        assert!(
            near(rgb_at(f, w / 4, h / 4), [255, 0, 0]),
            "{:?}",
            rgb_at(f, w / 4, h / 4)
        );
        assert!(near(rgb_at(f, 3 * w / 4, h / 4), [0, 255, 0]));
        assert!(near(rgb_at(f, w / 4, 3 * h / 4), [0, 0, 255]));
        assert!(near(rgb_at(f, 3 * w / 4, 3 * h / 4), [240, 240, 240]));
    }

    #[test]
    fn x264_path_encodes_and_decodes_with_colors() {
        let plan = Plan {
            name: "libx264",
            kind: Kind::X264,
            node: None,
        };
        let mut p = Pipeline::new(plan, 60_000, 8_000);
        let out = p
            .encode(test_shm_frame(320, 240, Q), false)
            .unwrap()
            .unwrap();
        assert!(out.keyframe, "first frame must be an IDR");
        assert!(out.codec_config.is_some());
        assert_eq!(p.path(), Some("x264-swscale"));
        check_colors(&decode_first(&out));
        // Next frame: delta, no config; forced keyframe again IDR with config.
        let out = p
            .encode(test_shm_frame(320, 240, Q), false)
            .unwrap()
            .unwrap();
        assert!(!out.keyframe && out.codec_config.is_none());
        let out = p
            .encode(test_shm_frame(320, 240, Q), true)
            .unwrap()
            .unwrap();
        assert!(out.keyframe && out.codec_config.is_some());
    }

    #[test]
    fn size_change_reopens_with_new_idr() {
        let plan = Plan {
            name: "libx264",
            kind: Kind::X264,
            node: None,
        };
        let mut p = Pipeline::new(plan, 60_000, 8_000);
        p.encode(test_shm_frame(320, 240, Q), false)
            .unwrap()
            .unwrap();
        let out = p
            .encode(test_shm_frame(640, 360, Q), false)
            .unwrap()
            .unwrap();
        assert!(out.keyframe && out.codec_config.is_some());
        assert_eq!(decode_first(&out).width, 640);
    }

    fn intel_node() -> Option<String> {
        crate::adapter::list_render_nodes()
            .into_iter()
            .find(|n| n.driver == "i915" || n.driver == "xe" || n.driver == "amdgpu")
            .map(|n| n.path)
    }

    #[test]
    fn vaapi_shm_path_if_available() {
        let Some(node) = intel_node() else {
            eprintln!("skip: no VAAPI-capable render node");
            return;
        };
        let plan = Plan {
            name: "h264_vaapi",
            kind: Kind::Vaapi,
            node: Some(node),
        };
        let mut p = Pipeline::new(plan, 60_000, 8_000);
        match p.encode(test_shm_frame(640, 360, Q), false) {
            Ok(Some(out)) => {
                assert!(out.keyframe && out.codec_config.is_some());
                assert_eq!(p.path(), Some("vaapi-shm"));
                check_colors(&decode_first(&out));
            }
            Ok(None) => panic!("no packet"),
            Err(e) => eprintln!("skip: VAAPI unusable here: {e}"),
        }
    }

    fn vaapi_hdr_plan(node: String) -> Plan {
        Plan {
            name: "hevc_vaapi",
            kind: Kind::Vaapi,
            node: Some(node),
        }
    }

    fn hdr_tags() -> StreamTags {
        StreamTags::from(&ezd_proto::StreamColor::hdr10(None, 0, 0))
    }

    /// What FFmpeg's HEVC decoder reports for the first picture of `out`.
    #[derive(Debug)]
    struct Decoded10 {
        pix_fmt: ff::AVPixelFormat,
        primaries: ff::AVColorPrimaries,
        trc: ff::AVColorTransferCharacteristic,
        matrix: ff::AVColorSpace,
        range: ff::AVColorRange,
        width: usize,
        height: usize,
        /// 10-bit luma samples.
        luma: Vec<u16>,
        /// 10-bit Cb and Cr samples (half resolution).
        cb: Vec<u16>,
        cr: Vec<u16>,
    }

    fn decode_hevc_10bit(out: &Output) -> Decoded10 {
        // SAFETY: plain libavcodec decoding of one access unit; every object is freed below and
        // the plane reads stay within width/height and linesize of the decoded frame.
        unsafe {
            let codec = ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_HEVC);
            assert!(!codec.is_null(), "no HEVC decoder in this FFmpeg");
            let mut ctx = ff::avcodec_alloc_context3(codec);
            (*ctx).thread_count = 1;
            assert!(ff::avcodec_open2(ctx, codec, ptr::null_mut()) >= 0);
            let mut bytes = out.codec_config.clone().unwrap_or_default();
            bytes.extend_from_slice(&out.data);
            let mut pkt = ff::av_packet_alloc();
            assert!(ff::av_new_packet(pkt, bytes.len() as i32) >= 0);
            std::slice::from_raw_parts_mut((*pkt).data, bytes.len()).copy_from_slice(&bytes);
            assert!(ff::avcodec_send_packet(ctx, pkt) >= 0);
            assert!(ff::avcodec_send_packet(ctx, ptr::null()) >= 0);
            let frame = ff::av_frame_alloc();
            let r = ff::avcodec_receive_frame(ctx, frame);
            assert!(
                r >= 0,
                "HEVC decoder produced no picture: {}",
                averr("recv", r)
            );
            let f = &*frame;
            let (w, h) = (f.width as usize, f.height as usize);
            let plane = |i: usize, pw: usize, ph: usize| -> Vec<u16> {
                let mut v = Vec::with_capacity(pw * ph);
                for y in 0..ph {
                    let row = f.data[i].add(y * f.linesize[i] as usize) as *const u16;
                    v.extend_from_slice(std::slice::from_raw_parts(row, pw));
                }
                v
            };
            let d = Decoded10 {
                pix_fmt: std::mem::transmute::<i32, ff::AVPixelFormat>(f.format),
                primaries: f.color_primaries,
                trc: f.color_trc,
                matrix: f.colorspace,
                range: f.color_range,
                width: w,
                height: h,
                luma: plane(0, w, h),
                cb: plane(1, w / 2, h / 2),
                cr: plane(2, w / 2, h / 2),
            };
            let mut frame = frame;
            ff::av_frame_free(&mut frame);
            ff::av_packet_free(&mut pkt);
            ff::avcodec_free_context(&mut ctx);
            d
        }
    }

    /// Full-range 10-bit RGB (PQ code values) to limited-range BT.2020 NCL 10-bit Y, Cb, Cr.
    fn expected_ycbcr_10(r: u16, g: u16, b: u16) -> (f64, f64, f64) {
        let (r, g, b) = (r as f64 / 1023.0, g as f64 / 1023.0, b as f64 / 1023.0);
        let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
        let cb = (b - y) / 1.8814;
        let cr = (r - y) / 1.4746;
        (64.0 + 876.0 * y, 512.0 + 896.0 * cb, 512.0 + 896.0 * cr)
    }

    /// Each quadrant of `d` has the limited-range BT.2020 value of its colour (10-bit codes).
    fn check_quadrants_10(d: &Decoded10, quads: [(u16, u16, u16); 4]) {
        let (w, h) = (d.width, d.height);
        for (i, (r, g, b)) in quads.into_iter().enumerate() {
            let (qx, qy) = (i % 2, i / 2);
            let (x, y) = (qx * w / 2 + w / 4, qy * h / 2 + h / 4);
            let (ey, ecb, ecr) = expected_ycbcr_10(r, g, b);
            let (gy, gcb, gcr) = (
                d.luma[y * d.width + x] as f64,
                d.cb[(y / 2) * (d.width / 2) + x / 2] as f64,
                d.cr[(y / 2) * (d.width / 2) + x / 2] as f64,
            );
            let tol = 24.0;
            assert!(
                (gy - ey).abs() < tol && (gcb - ecb).abs() < tol && (gcr - ecr).abs() < tol,
                "quadrant {i}: got Y/Cb/Cr {gy}/{gcb}/{gcr}, expected {ey:.0}/{ecb:.0}/{ecr:.0}"
            );
        }
    }

    #[test]
    fn hdr_filter_chains() {
        let v = scale_vaapi_variants(1280, 720, true);
        assert_eq!(
            v[0],
            "scale_vaapi=w=1280:h=720:format=p010:out_color_matrix=bt2020nc:out_color_primaries=bt2020:out_color_transfer=smpte2084:out_range=tv"
        );
        assert_eq!(
            v[1],
            "scale_vaapi=w=1280:h=720:format=p010:out_color_matrix=bt2020nc:out_range=tv"
        );
        assert_eq!(v[2], "scale_vaapi=w=1280:h=720:format=p010");
        // The SDR chain is exactly what it always was.
        assert_eq!(
            scale_vaapi_variants(1280, 720, false),
            [
                "scale_vaapi=w=1280:h=720:format=nv12:out_color_matrix=bt709:out_range=tv",
                "scale_vaapi=w=1280:h=720:format=nv12"
            ]
        );
    }

    #[test]
    fn hdr_plans_are_hevc_on_the_same_nodes() {
        let all = |_: &Plan| true;
        let intel = plan_hdr_encoders(Some(&node("/dev/dri/renderD129", 0x8086)), &all);
        assert_eq!(intel.len(), 1);
        assert_eq!((intel[0].name, intel[0].kind), ("hevc_vaapi", Kind::Vaapi));
        assert_eq!(intel[0].node.as_deref(), Some("/dev/dri/renderD129"));
        assert!(intel[0].hdr10() && intel[0].hardware());
        let nv = plan_hdr_encoders(Some(&node("/dev/dri/renderD128", 0x10de)), &all);
        assert_eq!((nv[0].name, nv[0].kind), ("hevc_nvenc", Kind::Nvenc));
        // No software HDR path, and a plan that does not open is not offered.
        assert!(plan_hdr_encoders(None, &all).is_empty());
        assert!(
            plan_hdr_encoders(Some(&node("/dev/dri/renderD129", 0x8086)), &|_| false).is_empty()
        );
        // SDR plans are never HDR plans.
        for p in plan_encoders(Some(&node("/dev/dri/renderD129", 0x8086)), &all) {
            assert!(!p.hdr10());
        }
    }

    #[test]
    fn stream_tags_follow_the_stream_color() {
        let sdr = StreamTags::default();
        assert_eq!(sdr.color, ColorInfo::BT709_LIMITED);
        assert_eq!(StreamTags::from(&ezd_proto::StreamColor::sdr_bt709()), sdr);
        let hdr = hdr_tags();
        assert_eq!(hdr.color, ColorInfo::BT2020_PQ_LIMITED);
        assert_eq!(
            av_color(sdr.color),
            (
                ff::AVColorPrimaries::AVCOL_PRI_BT709,
                ff::AVColorTransferCharacteristic::AVCOL_TRC_BT709,
                ff::AVColorSpace::AVCOL_SPC_BT709,
                ff::AVColorRange::AVCOL_RANGE_MPEG
            )
        );
        assert_eq!(
            av_color(hdr.color),
            (
                ff::AVColorPrimaries::AVCOL_PRI_BT2020,
                ff::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084,
                ff::AVColorSpace::AVCOL_SPC_BT2020_NCL,
                ff::AVColorRange::AVCOL_RANGE_MPEG
            )
        );
    }

    #[test]
    fn ten_bit_capture_formats_map_to_ffmpeg() {
        use crate::frame::spa_fmt10::*;
        assert_eq!(
            pix_fmt_of(XRGB_210LE),
            Ok(ff::AVPixelFormat::AV_PIX_FMT_X2RGB10LE)
        );
        assert_eq!(
            pix_fmt_of(ARGB_210LE),
            Ok(ff::AVPixelFormat::AV_PIX_FMT_X2RGB10LE)
        );
        assert_eq!(
            pix_fmt_of(XBGR_210LE),
            Ok(ff::AVPixelFormat::AV_PIX_FMT_X2BGR10LE)
        );
        // r = 1, g = 2, b = 3 in xRGB_210LE: x:R:G:B = 2:10:10:10 in a little-endian word.
        let f = test_shm_frame_10(2, 2, [(1, 2, 3); 4]);
        let Pixels::Shm(s) = f.pixels else { panic!() };
        assert_eq!(s.data[..4], ((1u32 << 20) | (2 << 10) | 3).to_le_bytes());
    }

    #[test]
    fn hdr_plan_rejects_8_bit_capture() {
        let plan = vaapi_hdr_plan("/dev/dri/renderD129".into());
        let mut c = Converter::new(plan, 60_000).with_tags(hdr_tags());
        let e = c.convert(test_shm_frame(64, 64, Q), false).err().unwrap();
        assert!(e.contains("10-bit"), "{e}");
    }

    #[test]
    fn hevc_main10_vaapi_encodes_hdr10_from_10_bit_rgb_if_available() {
        let Some(node) = intel_node() else {
            eprintln!("skip: no VAAPI-capable render node");
            return;
        };
        let (w, h) = (640, 360);
        // 10-bit PQ code values: red, green, blue and near-white quadrants.
        let quads = [(940, 64, 64), (64, 940, 64), (64, 64, 940), (900, 900, 900)];
        let mut p = Pipeline::new(vaapi_hdr_plan(node), 60_000, 8_000).with_tags(hdr_tags());
        let out = match p.encode(test_shm_frame_10(w, h, quads), false) {
            Ok(Some(out)) => out,
            Ok(None) => panic!("no packet"),
            Err(e) => {
                eprintln!("skip: HEVC Main10 on VAAPI unusable here: {e}");
                return;
            }
        };
        assert!(out.keyframe && out.codec_config.is_some());
        assert_eq!(p.path(), Some("vaapi-shm"));
        let d = decode_hevc_10bit(&out);
        assert_eq!(d.pix_fmt, ff::AVPixelFormat::AV_PIX_FMT_YUV420P10LE);
        assert_eq!(d.primaries, ff::AVColorPrimaries::AVCOL_PRI_BT2020);
        assert_eq!(
            d.trc,
            ff::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084
        );
        assert_eq!(d.matrix, ff::AVColorSpace::AVCOL_SPC_BT2020_NCL);
        assert_eq!(d.range, ff::AVColorRange::AVCOL_RANGE_MPEG);
        assert_eq!((d.width, d.height), (w as usize, h as usize));
        check_quadrants_10(&d, quads);
    }

    #[test]
    fn hevc_main10_vaapi_encodes_hdr10_from_a_10_bit_dmabuf_if_available() {
        let Some(node) = intel_node() else {
            eprintln!("skip: no VAAPI-capable render node");
            return;
        };
        let (w, h) = (640, 360);
        let quads = [(940, 64, 64), (64, 940, 64), (64, 64, 940), (900, 900, 900)];
        let frame = match export_test_dmabuf_10(&node, w, h, quads) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skip: no 10-bit RGB VAAPI surface here: {e}");
                return;
            }
        };
        let mut p = Pipeline::new(vaapi_hdr_plan(node), 60_000, 8_000).with_tags(hdr_tags());
        match p.encode(frame, false) {
            Ok(Some(out)) => {
                assert_eq!(p.path(), Some("vaapi-dmabuf"));
                let d = decode_hevc_10bit(&out);
                assert_eq!(d.pix_fmt, ff::AVPixelFormat::AV_PIX_FMT_YUV420P10LE);
                assert_eq!(d.primaries, ff::AVColorPrimaries::AVCOL_PRI_BT2020);
                assert_eq!(
                    d.trc,
                    ff::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084
                );
                check_quadrants_10(&d, quads);
            }
            Ok(None) => panic!("no packet"),
            Err(e) => eprintln!("skip: 10-bit DMA-BUF import unusable here: {e}"),
        }
    }

    #[test]
    fn vaapi_dmabuf_zero_copy_if_available() {
        let Some(node) = intel_node() else {
            eprintln!("skip: no VAAPI-capable render node");
            return;
        };
        let frame = match export_test_dmabuf(&node, 640, 360, Q) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skip: cannot export a dma-buf: {e}");
                return;
            }
        };
        let plan = Plan {
            name: "h264_vaapi",
            kind: Kind::Vaapi,
            node: Some(node),
        };
        let mut p = Pipeline::new(plan, 60_000, 8_000);
        let out = p
            .encode(frame, false)
            .expect("dma-buf import + encode")
            .expect("packet");
        assert_eq!(p.path(), Some("vaapi-dmabuf"));
        assert!(out.keyframe && out.codec_config.is_some());
        check_colors(&decode_first(&out));
    }

    #[test]
    fn vaapi_dmabuf_synced_conversion_encodes_on_another_thread() {
        let Some(node) = intel_node() else {
            eprintln!("skip: no VAAPI-capable render node");
            return;
        };
        let frame = match export_test_dmabuf(&node, 640, 360, Q) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skip: cannot export a dma-buf: {e}");
                return;
            }
        };
        let plan = Plan {
            name: "h264_vaapi",
            kind: Kind::Vaapi,
            node: Some(node),
        };
        let mut conv = Converter::new(plan, 60_000);
        let p = conv
            .convert(frame, true)
            .expect("dma-buf import + convert")
            .expect("converted frame");
        assert_eq!(conv.path(), Some("vaapi-dmabuf"));
        // Synced: the capture buffer went back before encoding.
        assert!(p.release.is_none());
        let out = std::thread::spawn(move || {
            let mut enc = Encoder::new("h264_vaapi", 60_000, 8_000);
            enc.encode(p, false).expect("encode").expect("packet")
        })
        .join()
        .unwrap();
        assert!(out.keyframe && out.codec_config.is_some());
        check_colors(&decode_first(&out));
    }

    fn fill_quadrants(base: *mut u8, stride: usize, w: u32, h: u32) {
        for y in 0..h as usize {
            for x in 0..w as usize {
                let q = (y >= h as usize / 2) as usize * 2 + (x >= w as usize / 2) as usize;
                let (r, g, b) = Q[q];
                // SAFETY: callers pass a mapping of at least `stride * h` bytes.
                unsafe {
                    base.add(y * stride + x * 4)
                        .copy_from_nonoverlapping([b, g, r, 0].as_ptr(), 4)
                };
            }
        }
    }

    /// Fills a (possibly block-linear) DMA-BUF through a Vulkan image mapped onto it.
    fn fill_via_vulkan(
        node: &str,
        fd: &OwnedFd,
        w: u32,
        h: u32,
        bo: &Bo<'_>,
    ) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        let import = VkImport::get(node, ff::AVPixelFormat::AV_PIX_FMT_BGR0, w, h)?;
        // SAFETY: lseek on a valid fd; dup'd fd is moved into the DRM frame below.
        let (size, dup) = unsafe {
            (
                libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_END),
                libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0),
            )
        };
        if size <= 0 || dup < 0 {
            return Err("cannot size/dup the dma-buf".into());
        }
        let dma = DmaBuf {
            spa_format: crate::frame::spa_fmt::BGRX,
            modifier: bo.modifier(),
            planes: vec![crate::frame::Plane {
                // SAFETY: fresh fd from fcntl.
                fd: unsafe { std::os::fd::FromRawFd::from_raw_fd(dup) },
                size: size as u64,
                offset: bo.offset(),
                pitch: bo.stride(),
            }],
            release: crate::frame::Release(None),
        };
        let (df, _rel) = drm_frame(w, h, dma, &import.drm_frames)?;
        let vf = import.map(&df, ff::AV_HWFRAME_MAP_WRITE as i32)?;
        // SAFETY: a software upload into the mapped frame and a download back; every frame is owned
        // by a wrapper that frees it.
        unsafe {
            let sw = OwnedFrame::alloc()?;
            (*sw.0).format = ff::AVPixelFormat::AV_PIX_FMT_BGR0 as i32;
            (*sw.0).width = w as i32;
            (*sw.0).height = h as i32;
            let r = ff::av_frame_get_buffer(sw.0, 0);
            if r < 0 {
                return Err(averr("av_frame_get_buffer", r));
            }
            fill_quadrants((*sw.0).data[0], (*sw.0).linesize[0] as usize, w, h);
            let r = ff::av_hwframe_transfer_data(vf.0, sw.0, 0);
            if r < 0 {
                return Err(averr("upload into mapped DMA-BUF", r));
            }
            // The upload is only submitted. Reading it back waits for it, so the pipeline's
            // mapping, which knows nothing of this one's semaphores, sees the pixels.
            let back = OwnedFrame::alloc()?;
            (*back.0).format = ff::AVPixelFormat::AV_PIX_FMT_BGR0 as i32;
            let r = ff::av_hwframe_transfer_data(back.0, vf.0, 0);
            if r < 0 {
                return Err(averr("read back mapped DMA-BUF", r));
            }
            let px = (*back.0).data[0]
                .add((h as usize / 4) * (*back.0).linesize[0] as usize + w as usize);
            let (r, g, b) = Q[0];
            if std::slice::from_raw_parts(px, 3) != [b, g, r] {
                return Err("read-back of the Vulkan fill differs".into());
            }
        }
        Ok(())
    }

    /// NVIDIA render node, GBM on it, and the FFmpeg pieces the path needs; `Err` is a skip reason.
    fn nvenc_env() -> Result<(String, Gbm), String> {
        let node = crate::adapter::list_render_nodes()
            .into_iter()
            .find(|n| n.is_nvidia())
            .ok_or("no NVIDIA render node")?;
        // SAFETY: lookups by static names.
        unsafe {
            if ff::avcodec_find_encoder_by_name(c"h264_nvenc".as_ptr()).is_null() {
                return Err("FFmpeg has no h264_nvenc".into());
            }
            if ff::avfilter_get_by_name(c"scale_vulkan".as_ptr()).is_null() {
                return Err("FFmpeg has no scale_vulkan".into());
            }
        }
        let gbm = Gbm::open(&node.path)?;
        Ok((node.path, gbm))
    }

    const NV_MODS: std::ops::RangeInclusive<u64> = 0x0300_0000_0060_6010..=0x0300_0000_0060_6015;
    const NV_W: u32 = 1280;
    const NV_H: u32 = 720;

    /// A GBM buffer filled with `Q`, as a captured frame (the bo must outlive the frame's use).
    fn nvenc_frame(node: &str, bo: &Bo<'_>, vulkan_fill: bool) -> Result<Frame, String> {
        fill_bo(node, bo, vulkan_fill)?;
        bo_frame(bo)
    }

    /// Fills `bo` with `Q`, through Vulkan for block-linear buffers.
    fn fill_bo(node: &str, bo: &Bo<'_>, vulkan_fill: bool) -> Result<(), String> {
        if vulkan_fill {
            fill_via_vulkan(node, &bo.fd()?, NV_W, NV_H, bo)
        } else {
            bo.write_mapped(NV_W, NV_H, |p, stride| {
                fill_quadrants(p, stride, NV_W, NV_H)
            })
        }
    }

    /// `bo` as a captured frame, with a fresh fd as PipeWire hands one over.
    fn bo_frame(bo: &Bo<'_>) -> Result<Frame, String> {
        use std::os::fd::AsRawFd;
        let fd = bo.fd()?;
        // SAFETY: lseek on a valid fd.
        let size = unsafe { libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_END) };
        if size <= 0 {
            return Err("cannot size the dma-buf".into());
        }
        Ok(Frame {
            width: NV_W,
            height: NV_H,
            capture_us: now_us(),
            seq: 0,
            pixels: Pixels::Dma(DmaBuf {
                spa_format: crate::frame::spa_fmt::BGRX,
                modifier: bo.modifier(),
                planes: vec![crate::frame::Plane {
                    fd,
                    size: size as u64,
                    offset: bo.offset(),
                    pitch: bo.stride(),
                }],
                release: crate::frame::Release(None),
            }),
        })
    }

    #[test]
    fn vulkan_import_is_shared_per_format() {
        let (node, _gbm) = match nvenc_env() {
            Ok(x) => x,
            Err(e) => return eprintln!("skip: {e}"),
        };
        let bgr0 = ff::AVPixelFormat::AV_PIX_FMT_BGR0;
        let a = VkImport::get(&node, bgr0, 640, 360).expect("import");
        let b = VkImport::get(&node, bgr0, 640, 360).expect("import");
        assert!(
            Arc::ptr_eq(&a, &b),
            "FFmpeg cannot free an import, so it must be reused"
        );
        let c = VkImport::get(&node, bgr0, 320, 180).expect("import");
        assert!(!Arc::ptr_eq(&a, &c));
        // SAFETY: valid buffer refs.
        assert_eq!(
            unsafe { (*a.vk.0).data },
            unsafe { (*c.vk.0).data },
            "one device per node"
        );
    }

    fn nvenc_plan(node: String) -> Plan {
        Plan {
            name: "h264_nvenc",
            kind: Kind::Nvenc,
            node: Some(node),
        }
    }

    fn nvenc_dmabuf_roundtrip(mods: &[u64], vulkan_fill: bool, linear: bool) {
        let (node, gbm) = match nvenc_env() {
            Ok(x) => x,
            Err(e) => return eprintln!("skip: {e}"),
        };
        let bo = match gbm.create_bo(NV_W, NV_H, mods) {
            Ok(b) => b,
            Err(e) => return eprintln!("skip: {e} (GBM backend {})", gbm.backend()),
        };
        eprintln!(
            "GBM backend {}, modifier {:#x}, stride {}",
            gbm.backend(),
            bo.modifier(),
            bo.stride()
        );
        assert_eq!(bo.modifier() == DRM_FORMAT_MOD_LINEAR, linear);
        let frame = nvenc_frame(&node, &bo, vulkan_fill).expect("fill buffer");
        let mut p = Pipeline::new(nvenc_plan(node), 60_000, 8_000);
        let out = p
            .encode(frame, false)
            .expect("dma-buf import + encode")
            .expect("packet");
        assert_eq!(p.path(), Some("nvenc-dmabuf-rgb"));
        assert!(out.keyframe && out.codec_config.is_some());
        check_colors(&decode_first(&out));
    }

    #[test]
    fn nvenc_dmabuf_linear_if_available() {
        nvenc_dmabuf_roundtrip(&[DRM_FORMAT_MOD_LINEAR], false, true);
    }

    #[test]
    fn nvenc_dmabuf_block_linear_if_available() {
        nvenc_dmabuf_roundtrip(&NV_MODS.collect::<Vec<_>>(), true, false);
    }

    #[test]
    fn nvenc_dmabuf_synced_conversion_encodes_on_another_thread() {
        let (node, gbm) = match nvenc_env() {
            Ok(x) => x,
            Err(e) => return eprintln!("skip: {e}"),
        };
        let bo = match gbm.create_bo(NV_W, NV_H, &[DRM_FORMAT_MOD_LINEAR]) {
            Ok(b) => b,
            Err(e) => return eprintln!("skip: {e}"),
        };
        let frame = nvenc_frame(&node, &bo, false).expect("fill buffer");
        let mut conv = Converter::new(nvenc_plan(node), 60_000);
        let p = conv
            .convert(frame, true)
            .expect("dma-buf import + convert")
            .expect("converted frame");
        assert_eq!(conv.path(), Some("nvenc-dmabuf-rgb"));
        // CUDA output: the capture buffer stays held until the packet is out.
        assert!(p.release.is_some());
        let out = std::thread::spawn(move || {
            let mut enc = Encoder::new("h264_nvenc", 60_000, 8_000);
            enc.encode(p, false).expect("encode").expect("packet")
        })
        .join()
        .unwrap();
        assert!(out.keyframe && out.codec_config.is_some());
        check_colors(&decode_first(&out));
    }

    /// Many frames from a small pool of buffers, as the compositor cycles them: every decoded frame
    /// must keep the content. Without dedicated allocations NVIDIA sampled some imports as black,
    /// one frame in 16 to 33 depending on the buffers (see `vkdma`).
    #[test]
    fn nvenc_dmabuf_many_frames_keep_their_content() {
        let (node, gbm) = match nvenc_env() {
            Ok(x) => x,
            Err(e) => return eprintln!("skip: {e}"),
        };
        for (label, mods) in [
            ("linear", vec![DRM_FORMAT_MOD_LINEAR]),
            ("block-linear", NV_MODS.collect()),
        ] {
            let linear = label == "linear";
            let bos: Vec<Bo<'_>> = match (0..4).map(|_| gbm.create_bo(NV_W, NV_H, &mods)).collect()
            {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("skip {label}: {e}");
                    continue;
                }
            };
            for bo in &bos {
                fill_bo(&node, bo, !linear).expect("fill buffer");
            }
            let mut p = Pipeline::new(nvenc_plan(node.clone()), 60_000, 8_000);
            let mut d = Decoder::new(Codec::H264).unwrap();
            let mut bad = Vec::new();
            for i in 0..100 {
                let frame = bo_frame(&bos[i % bos.len()]).expect("frame");
                let out = p.encode(frame, false).expect("encode").expect("packet");
                if let Some(c) = &out.codec_config {
                    d.push_config(c).unwrap();
                }
                for f in d.decode(&out.data, i as i64).unwrap() {
                    let c = rgb_at(&f, f.width / 4, f.height / 4);
                    if !near(c, [255, 0, 0]) {
                        bad.push((i, c));
                    }
                }
            }
            assert_eq!(p.path(), Some("nvenc-dmabuf-rgb"));
            assert!(bad.is_empty(), "{label} frames lost their content: {bad:?}");
        }
    }

    /// `cargo test -p ezd-host-linux --release -- --ignored --nocapture latency`
    #[test]
    #[ignore = "benchmark"]
    fn latency_per_path() {
        fn bench(label: &str, plan: Plan, mk: &dyn Fn() -> Frame) {
            let mut p = Pipeline::new(plan, 60_000, 20_000);
            let mut times = Vec::new();
            for i in 0..120 {
                let f = mk();
                let t = std::time::Instant::now();
                if p.encode(f, i == 0).is_err() {
                    eprintln!("{label}: failed");
                    return;
                }
                if i > 5 {
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "{label}: median {:.2} ms, p95 {:.2} ms (1920x1080, submit to packet)",
                times[times.len() / 2],
                times[times.len() * 95 / 100]
            );
        }
        if let Some(node) = intel_node() {
            let plan = Plan {
                name: "h264_vaapi",
                kind: Kind::Vaapi,
                node: Some(node.clone()),
            };
            bench("vaapi-shm", plan.clone(), &|| test_shm_frame(1920, 1080, Q));
            bench("vaapi-dmabuf", plan, &|| {
                export_test_dmabuf(&node, 1920, 1080, Q).unwrap()
            });
        }
        let x = Plan {
            name: "libx264",
            kind: Kind::X264,
            node: None,
        };
        bench("x264", x, &|| test_shm_frame(1920, 1080, Q));
    }
}
