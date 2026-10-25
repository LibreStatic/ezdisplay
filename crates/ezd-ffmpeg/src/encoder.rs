use crate::error::{EAGAIN, FfmpegError, averror_eof, init_logging};
use crate::hw::HwDevice;
use crate::options::option_sets;
use ffmpeg_sys_next as ffi;
use std::ffi::{CStr, CString};
use std::ptr;

/// Layout of the raw frames handed to [`Encoder::encode`] (tightly packed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPixelFormat {
    /// Planar YUV 4:2:0 (`yuv420p`).
    I420,
    /// Semi-planar YUV 4:2:0 (`nv12`).
    Nv12,
}

impl RawPixelFormat {
    fn av(self) -> ffi::AVPixelFormat {
        match self {
            RawPixelFormat::I420 => ffi::AVPixelFormat::AV_PIX_FMT_YUV420P,
            RawPixelFormat::Nv12 => ffi::AVPixelFormat::AV_PIX_FMT_NV12,
        }
    }
}

/// Parameters for [`Encoder::open`].
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// FFmpeg encoder name, e.g. `libx264`, `h264_nvenc`, `h264_vaapi`.
    pub name: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frame rate in millihertz (60000 = 60 fps).
    pub refresh_mhz: u32,
    /// Target bitrate in kbit/s.
    pub bitrate_kbps: u32,
    /// Layout of the frames passed to `encode`.
    pub pix_fmt: RawPixelFormat,
    /// If set, software frames are uploaded to this device and encoded from
    /// hardware surfaces (VAAPI, QSV, CUDA, D3D11). NV12 is used on the device.
    pub hw: Option<HwDevice>,
    /// Extra AVOptions applied on top of the preset (override it).
    pub extra_options: Vec<(String, String)>,
}

impl EncoderConfig {
    /// Config for software frames in I420 without extra options.
    pub fn new(
        name: impl Into<String>,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        bitrate_kbps: u32,
    ) -> Self {
        EncoderConfig {
            name: name.into(),
            width,
            height,
            refresh_mhz,
            bitrate_kbps,
            pix_fmt: RawPixelFormat::I420,
            hw: None,
            extra_options: Vec::new(),
        }
    }
}

/// One encoded access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    /// Annex-B bitstream.
    pub data: Vec<u8>,
    /// Keyframe flag from the encoder.
    pub keyframe: bool,
    /// Presentation timestamp (frame counter).
    pub pts: i64,
}

/// A configured, opened encoder tuned for low latency.
pub struct Encoder {
    ctx: *mut ffi::AVCodecContext,
    sw_frame: *mut ffi::AVFrame,
    pkt: *mut ffi::AVPacket,
    cfg: EncoderConfig,
    extradata: Vec<u8>,
    pts: i64,
    /// Format of `sw_frame`.
    sw_fmt: RawPixelFormat,
}

// SAFETY: the FFmpeg contexts are owned exclusively by this struct and only
// accessed through &mut self, so moving it between threads is fine.
unsafe impl Send for Encoder {}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("name", &self.cfg.name)
            .finish_non_exhaustive()
    }
}

fn cstr(s: &str) -> Result<CString, FfmpegError> {
    CString::new(s).map_err(|_| FfmpegError::msg(format!("string contains NUL: {s:?}")))
}

impl Encoder {
    /// Opens the encoder. Option presets (see [`option_sets`]) are tried in
    /// order until one opens, so e.g. `low_power` VAAPI falls back to normal mode.
    pub fn open(cfg: EncoderConfig) -> Result<Encoder, FfmpegError> {
        init_logging();
        if cfg.width == 0
            || cfg.height == 0
            || cfg.width % 2 != 0
            || cfg.height % 2 != 0
            || cfg.refresh_mhz == 0
        {
            return Err(FfmpegError::msg(
                "encoder needs even non-zero dimensions and a refresh rate",
            ));
        }
        let cname = cstr(&cfg.name)?;
        // SAFETY: cname is a valid NUL-terminated string.
        let codec = unsafe { ffi::avcodec_find_encoder_by_name(cname.as_ptr()) };
        if codec.is_null() {
            return Err(FfmpegError::msg(format!(
                "encoder {:?} not available in this FFmpeg build",
                cfg.name
            )));
        }
        let mut last = FfmpegError::msg("no option set");
        for set in option_sets(&cfg.name) {
            match Self::try_open(codec, cfg.clone(), &set) {
                Ok(e) => return Ok(e),
                Err(e) => {
                    tracing::debug!("opening {} with {set:?} failed: {e}", cfg.name);
                    last = e;
                }
            }
        }
        Err(last)
    }

    fn try_open(
        codec: *const ffi::AVCodec,
        cfg: EncoderConfig,
        preset: &[(&'static str, String)],
    ) -> Result<Encoder, FfmpegError> {
        // SAFETY: `codec` is a valid encoder returned by avcodec_find_encoder_by_name.
        let ctx = unsafe { ffi::avcodec_alloc_context3(codec) };
        if ctx.is_null() {
            return Err(FfmpegError::msg("avcodec_alloc_context3 failed"));
        }
        let sw_fmt = if cfg.hw.is_some() {
            RawPixelFormat::Nv12
        } else {
            cfg.pix_fmt
        };
        // From here on Drop releases everything that has been allocated.
        let mut enc = Encoder {
            ctx,
            sw_frame: ptr::null_mut(),
            pkt: ptr::null_mut(),
            cfg,
            extradata: Vec::new(),
            pts: 0,
            sw_fmt,
        };
        let cfg = &enc.cfg;
        let bitrate = cfg.bitrate_kbps as i64 * 1000;
        let fps = cfg.refresh_mhz as f64 / 1000.0;
        // SAFETY: ctx is a freshly allocated, exclusively owned AVCodecContext; we only write plain fields.
        unsafe {
            let c = &mut *ctx;
            c.width = cfg.width as i32;
            c.height = cfg.height as i32;
            c.time_base = ffi::AVRational {
                num: 1000,
                den: cfg.refresh_mhz as i32,
            };
            c.framerate = ffi::AVRational {
                num: cfg.refresh_mhz as i32,
                den: 1000,
            };
            c.pix_fmt = match &cfg.hw {
                Some(h) => h.kind.pix_fmt(),
                None => cfg.pix_fmt.av(),
            };
            c.bit_rate = bitrate;
            c.rc_max_rate = bitrate;
            // Two frames worth of VBV buffer: tight enough for low latency, loose enough for keyframes.
            c.rc_buffer_size = ((bitrate as f64 * 2.0 / fps) as i32).max(1000);
            c.gop_size = i32::MAX;
            c.max_b_frames = 0;
            // Hardware encoders gain nothing from threads. libx264 needs them: one thread manages
            // only a few frames per second at tablet resolutions. Its sliced threads (see
            // `option_sets`) split each frame, so they add no latency.
            c.thread_count = if cfg.name == "libx264" { 0 } else { 1 };
            c.colorspace = ffi::AVColorSpace::AVCOL_SPC_BT709;
            c.color_primaries = ffi::AVColorPrimaries::AVCOL_PRI_BT709;
            c.color_trc = ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            c.color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
            c.flags |= (ffi::AV_CODEC_FLAG_GLOBAL_HEADER | ffi::AV_CODEC_FLAG_LOW_DELAY) as i32;
        }
        if let Some(dev) = enc.cfg.hw.clone() {
            enc.init_hw_frames(&dev)?;
        }

        let mut dict: *mut ffi::AVDictionary = ptr::null_mut();
        let cfg = &enc.cfg;
        let opts = preset
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .chain(cfg.extra_options.iter().cloned());
        for (k, v) in opts {
            let (ck, cv) = (cstr(&k)?, cstr(&v)?);
            // SAFETY: dict is a valid in/out pointer; key and value are NUL-terminated and copied by FFmpeg.
            unsafe { ffi::av_dict_set(&mut dict, ck.as_ptr(), cv.as_ptr(), 0) };
        }
        // SAFETY: ctx and codec are valid; `dict` is consumed/updated by avcodec_open2 and freed below.
        let r = unsafe { ffi::avcodec_open2(ctx, codec, &mut dict) };
        // SAFETY: dict is a valid (possibly null) dictionary; leftover entries are options the encoder ignored.
        unsafe {
            let empty = cstr("")?;
            let mut e: *const ffi::AVDictionaryEntry = ptr::null();
            loop {
                e = ffi::av_dict_get(dict, empty.as_ptr(), e, ffi::AV_DICT_IGNORE_SUFFIX as i32);
                if e.is_null() {
                    break;
                }
                tracing::warn!(
                    "{}: option {}={} not recognised",
                    enc.cfg.name,
                    CStr::from_ptr((*e).key).to_string_lossy(),
                    CStr::from_ptr((*e).value).to_string_lossy()
                );
            }
            ffi::av_dict_free(&mut dict);
        }
        if r < 0 {
            return Err(FfmpegError::av(
                format!("avcodec_open2({})", enc.cfg.name),
                r,
            ));
        }
        // SAFETY: after a successful open, extradata/extradata_size describe a valid buffer (or are 0/null).
        unsafe {
            let c = &*ctx;
            if !c.extradata.is_null() && c.extradata_size > 0 {
                enc.extradata =
                    std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec();
            }
            enc.pkt = ffi::av_packet_alloc();
            enc.sw_frame = ffi::av_frame_alloc();
            if enc.pkt.is_null() || enc.sw_frame.is_null() {
                return Err(FfmpegError::msg("allocation failed"));
            }
            let f = &mut *enc.sw_frame;
            f.format = enc.sw_fmt.av() as i32;
            f.width = enc.cfg.width as i32;
            f.height = enc.cfg.height as i32;
            let r = ffi::av_frame_get_buffer(enc.sw_frame, 0);
            if r < 0 {
                return Err(FfmpegError::av("av_frame_get_buffer", r));
            }
        }
        Ok(enc)
    }

    fn init_hw_frames(&mut self, dev: &HwDevice) -> Result<(), FfmpegError> {
        // SAFETY: ctx is valid; dev.buf is a valid device ref. We take our own references for the
        // codec context, and the frames context ref is released after being stored in the context.
        unsafe {
            (*self.ctx).hw_device_ctx = ffi::av_buffer_ref(dev.buf);
            let mut frames = ffi::av_hwframe_ctx_alloc(dev.buf);
            if frames.is_null() {
                return Err(FfmpegError::msg("av_hwframe_ctx_alloc failed"));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = dev.kind.pix_fmt();
            fc.sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
            fc.width = self.cfg.width as i32;
            fc.height = self.cfg.height as i32;
            fc.initial_pool_size = 4;
            let r = ffi::av_hwframe_ctx_init(frames);
            if r < 0 {
                ffi::av_buffer_unref(&mut frames);
                return Err(FfmpegError::av("av_hwframe_ctx_init", r));
            }
            (*self.ctx).hw_frames_ctx = ffi::av_buffer_ref(frames);
            ffi::av_buffer_unref(&mut frames);
        }
        Ok(())
    }

    /// Name of the encoder.
    pub fn name(&self) -> &str {
        &self.cfg.name
    }

    /// Global header / parameter sets (Annex-B SPS+PPS, plus VPS for HEVC) as
    /// produced at open time. Empty if the encoder only emits them in-band.
    pub fn extradata(&self) -> &[u8] {
        &self.extradata
    }

    fn fill_sw_frame(&mut self, raw: &[u8], force_keyframe: bool) -> Result<(), FfmpegError> {
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        let need = w * h * 3 / 2;
        if raw.len() != need {
            return Err(FfmpegError::msg(format!(
                "raw frame is {} bytes, expected {need}",
                raw.len()
            )));
        }
        // SAFETY: sw_frame has buffers allocated by av_frame_get_buffer for exactly w x h in sw_fmt;
        // the plane copies below stay within `linesize * rows` as checked by construction.
        unsafe {
            let r = ffi::av_frame_make_writable(self.sw_frame);
            if r < 0 {
                return Err(FfmpegError::av("av_frame_make_writable", r));
            }
            let f = &mut *self.sw_frame;
            let copy_plane =
                |dst: *mut u8, stride: i32, src: &[u8], row_len: usize, rows: usize| {
                    for y in 0..rows {
                        ptr::copy_nonoverlapping(
                            src.as_ptr().add(y * row_len),
                            dst.add(y * stride as usize),
                            row_len,
                        );
                    }
                };
            let (y_src, uv_src) = raw.split_at(w * h);
            copy_plane(f.data[0], f.linesize[0], y_src, w, h);
            match (self.cfg.pix_fmt, self.sw_fmt) {
                (RawPixelFormat::I420, RawPixelFormat::I420) => {
                    let (u, v) = uv_src.split_at(w * h / 4);
                    copy_plane(f.data[1], f.linesize[1], u, w / 2, h / 2);
                    copy_plane(f.data[2], f.linesize[2], v, w / 2, h / 2);
                }
                (RawPixelFormat::Nv12, RawPixelFormat::Nv12) => {
                    copy_plane(f.data[1], f.linesize[1], uv_src, w, h / 2);
                }
                (RawPixelFormat::I420, RawPixelFormat::Nv12) => {
                    // Interleave U and V for the hardware upload.
                    let (u, v) = uv_src.split_at(w * h / 4);
                    for y in 0..h / 2 {
                        let row = f.data[1].add(y * f.linesize[1] as usize);
                        for x in 0..w / 2 {
                            *row.add(2 * x) = u[y * (w / 2) + x];
                            *row.add(2 * x + 1) = v[y * (w / 2) + x];
                        }
                    }
                }
                (RawPixelFormat::Nv12, RawPixelFormat::I420) => {
                    return Err(FfmpegError::msg("unsupported conversion"));
                }
            }
            f.pts = self.pts;
            f.pict_type = if force_keyframe {
                ffi::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                ffi::AVPictureType::AV_PICTURE_TYPE_NONE
            };
            if force_keyframe {
                f.flags |= ffi::AV_FRAME_FLAG_KEY as i32;
            } else {
                f.flags &= !(ffi::AV_FRAME_FLAG_KEY as i32);
            }
        }
        Ok(())
    }

    /// Submits one raw frame (tightly packed in the configured layout).
    /// `force_keyframe` requests an IDR for this frame.
    pub fn send_frame(&mut self, raw: &[u8], force_keyframe: bool) -> Result<(), FfmpegError> {
        self.fill_sw_frame(raw, force_keyframe)?;
        let r = if self.cfg.hw.is_some() {
            // SAFETY: hw frame is allocated from the codec's frames context, filled from sw_frame,
            // sent (the encoder takes its own reference) and freed here.
            unsafe {
                let mut hw = ffi::av_frame_alloc();
                if hw.is_null() {
                    return Err(FfmpegError::msg("av_frame_alloc failed"));
                }
                let mut r = ffi::av_hwframe_get_buffer((*self.ctx).hw_frames_ctx, hw, 0);
                if r >= 0 {
                    r = ffi::av_hwframe_transfer_data(hw, self.sw_frame, 0);
                }
                if r >= 0 {
                    (*hw).pts = self.pts;
                    (*hw).pict_type = (*self.sw_frame).pict_type;
                    (*hw).flags = (*self.sw_frame).flags;
                    r = ffi::avcodec_send_frame(self.ctx, hw);
                }
                ffi::av_frame_free(&mut hw);
                r
            }
        } else {
            // SAFETY: ctx is an opened encoder and sw_frame a fully initialised frame; FFmpeg refs the buffers.
            unsafe { ffi::avcodec_send_frame(self.ctx, self.sw_frame) }
        };
        if r < 0 {
            return Err(FfmpegError::av("avcodec_send_frame", r));
        }
        self.pts += 1;
        Ok(())
    }

    /// Extension point for zero-copy backends: submits a frame the caller built
    /// (e.g. a VAAPI/CUDA/D3D11 surface from the capture pipeline).
    ///
    /// # Safety
    /// `frame` must be a valid `AVFrame` whose format, size and hardware frames
    /// context match this encoder's configuration. Pass null to flush.
    pub unsafe fn send_raw_frame(&mut self, frame: *const ffi::AVFrame) -> Result<(), FfmpegError> {
        // SAFETY: forwarded caller contract.
        let r = unsafe { ffi::avcodec_send_frame(self.ctx, frame) };
        if r < 0 {
            return Err(FfmpegError::av("avcodec_send_frame", r));
        }
        Ok(())
    }

    /// Fetches one encoded packet if available.
    pub fn receive_packet(&mut self) -> Result<Option<EncodedPacket>, FfmpegError> {
        // SAFETY: ctx is an opened encoder, pkt a valid packet; after success pkt->data/size describe
        // a readable buffer until av_packet_unref.
        unsafe {
            let r = ffi::avcodec_receive_packet(self.ctx, self.pkt);
            if r == EAGAIN || r == averror_eof() {
                return Ok(None);
            }
            if r < 0 {
                return Err(FfmpegError::av("avcodec_receive_packet", r));
            }
            let p = &*self.pkt;
            let data = std::slice::from_raw_parts(p.data, p.size as usize).to_vec();
            let out = EncodedPacket {
                data,
                keyframe: p.flags & ffi::AV_PKT_FLAG_KEY as i32 != 0,
                pts: p.pts,
            };
            ffi::av_packet_unref(self.pkt);
            Ok(Some(out))
        }
    }

    /// Convenience: submit a frame and return the packet if the encoder emitted
    /// one immediately (always the case for the low-latency presets).
    pub fn encode(
        &mut self,
        raw: &[u8],
        force_keyframe: bool,
    ) -> Result<Option<EncodedPacket>, FfmpegError> {
        self.send_frame(raw, force_keyframe)?;
        self.receive_packet()
    }

    /// Drains the encoder at end of stream.
    pub fn flush(&mut self) -> Result<Vec<EncodedPacket>, FfmpegError> {
        // SAFETY: a null frame signals end of stream.
        let r = unsafe { ffi::avcodec_send_frame(self.ctx, ptr::null()) };
        if r < 0 && r != averror_eof() {
            return Err(FfmpegError::av("flush", r));
        }
        let mut out = Vec::new();
        while let Some(p) = self.receive_packet()? {
            out.push(p);
        }
        Ok(out)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: each pointer is either null or exclusively owned by us; the FFmpeg free
        // functions accept pointers to null and reset them.
        unsafe {
            ffi::av_frame_free(&mut self.sw_frame);
            ffi::av_packet_free(&mut self.pkt);
            ffi::avcodec_free_context(&mut self.ctx);
        }
    }
}
