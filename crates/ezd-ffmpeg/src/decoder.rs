use crate::error::{EAGAIN, FfmpegError, averror_eof, init_logging};
use ezd_proto::Codec;

/// `AVERROR_INVALIDDATA` (`FFERRTAG('I','N','D','A')` negated).
const AVERROR_INVALIDDATA: i32 = -(i32::from_le_bytes(*b"INDA"));
use ffmpeg_sys_next as ffi;
use std::ptr;

/// A decoded picture with tightly packed planes (stride == width).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Timestamp passed to [`Decoder::decode`] for the packet that produced it.
    pub pts: i64,
    /// Decoder flagged this as a keyframe.
    pub keyframe: bool,
    /// Luma plane, `width * height` bytes.
    pub y: Vec<u8>,
    /// Cb plane, `(width/2) * (height/2)` bytes.
    pub u: Vec<u8>,
    /// Cr plane, `(width/2) * (height/2)` bytes.
    pub v: Vec<u8>,
}

impl DecodedFrame {
    /// Converts to packed RGB24 assuming BT.709 limited range.
    pub fn to_rgb24(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = Vec::with_capacity(w * h * 3);
        for yy in 0..h {
            for xx in 0..w {
                let y = (self.y[yy * w + xx] as f32 - 16.0) * 1.164;
                let ci = (yy / 2) * (w / 2) + xx / 2;
                let (u, v) = (self.u[ci] as f32 - 128.0, self.v[ci] as f32 - 128.0);
                let r = y + 1.793 * v;
                let g = y - 0.213 * u - 0.533 * v;
                let b = y + 2.112 * u;
                out.extend_from_slice(&[
                    r.clamp(0.0, 255.0) as u8,
                    g.clamp(0.0, 255.0) as u8,
                    b.clamp(0.0, 255.0) as u8,
                ]);
            }
        }
        out
    }
}

/// Low-delay software decoder (single thread, no frame reordering delay).
pub struct Decoder {
    ctx: *mut ffi::AVCodecContext,
    pkt: *mut ffi::AVPacket,
    frame: *mut ffi::AVFrame,
}

// SAFETY: the contexts are owned exclusively and only used through &mut self.
unsafe impl Send for Decoder {}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").finish_non_exhaustive()
    }
}

impl Decoder {
    /// Opens a decoder for `codec`.
    pub fn new(codec: Codec) -> Result<Decoder, FfmpegError> {
        init_logging();
        let id = match codec {
            Codec::H264 => ffi::AVCodecID::AV_CODEC_ID_H264,
            Codec::Hevc => ffi::AVCodecID::AV_CODEC_ID_HEVC,
            Codec::Av1 => ffi::AVCodecID::AV_CODEC_ID_AV1,
        };
        // SAFETY: plain lookup.
        let c = unsafe { ffi::avcodec_find_decoder(id) };
        if c.is_null() {
            return Err(FfmpegError::msg(format!("no decoder for {codec:?}")));
        }
        // SAFETY: c is a valid decoder.
        let ctx = unsafe { ffi::avcodec_alloc_context3(c) };
        if ctx.is_null() {
            return Err(FfmpegError::msg("avcodec_alloc_context3 failed"));
        }
        let mut d = Decoder {
            ctx,
            pkt: ptr::null_mut(),
            frame: ptr::null_mut(),
        };
        // SAFETY: ctx is exclusively owned and not yet opened; Drop frees partially built state.
        unsafe {
            (*ctx).thread_count = 1;
            (*ctx).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;
            let r = ffi::avcodec_open2(ctx, c, ptr::null_mut());
            if r < 0 {
                return Err(FfmpegError::av("avcodec_open2(decoder)", r));
            }
            d.pkt = ffi::av_packet_alloc();
            d.frame = ffi::av_frame_alloc();
            if d.pkt.is_null() || d.frame.is_null() {
                return Err(FfmpegError::msg("allocation failed"));
            }
        }
        Ok(d)
    }

    /// Feeds a `CODEC_CONFIG` payload (parameter sets only). FFmpeg reports
    /// "invalid data" for packets without a slice even though the parameter
    /// sets are stored; that specific condition is not treated as an error.
    pub fn push_config(&mut self, data: &[u8]) -> Result<(), FfmpegError> {
        match self.decode(data, 0) {
            Ok(_) => Ok(()),
            Err(FfmpegError::Av { code, .. }) if code == AVERROR_INVALIDDATA => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Feeds one access unit (Annex-B) and returns the pictures it completed
    /// (0 or 1 with low delay). Use [`push_config`](Self::push_config) for
    /// `CODEC_CONFIG` payloads.
    pub fn decode(&mut self, data: &[u8], pts: i64) -> Result<Vec<DecodedFrame>, FfmpegError> {
        // SAFETY: pkt is valid; av_new_packet allocates `len` + padding bytes which we fill completely.
        unsafe {
            let r = ffi::av_new_packet(self.pkt, data.len() as i32);
            if r < 0 {
                return Err(FfmpegError::av("av_new_packet", r));
            }
            ptr::copy_nonoverlapping(data.as_ptr(), (*self.pkt).data, data.len());
            (*self.pkt).pts = pts;
            let r = ffi::avcodec_send_packet(self.ctx, self.pkt);
            ffi::av_packet_unref(self.pkt);
            if r < 0 {
                return Err(FfmpegError::av("avcodec_send_packet", r));
            }
        }
        self.drain()
    }

    /// Flushes buffered pictures at end of stream.
    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, FfmpegError> {
        // SAFETY: null packet = drain request.
        let r = unsafe { ffi::avcodec_send_packet(self.ctx, ptr::null()) };
        if r < 0 && r != averror_eof() {
            return Err(FfmpegError::av("flush", r));
        }
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<DecodedFrame>, FfmpegError> {
        let mut out = Vec::new();
        loop {
            // SAFETY: ctx/frame are valid; on success the frame planes are readable for
            // linesize * rows bytes until av_frame_unref, and we copy them out before that.
            unsafe {
                let r = ffi::avcodec_receive_frame(self.ctx, self.frame);
                if r == EAGAIN || r == averror_eof() {
                    return Ok(out);
                }
                if r < 0 {
                    return Err(FfmpegError::av("avcodec_receive_frame", r));
                }
                let f = &*self.frame;
                let (w, h) = (f.width as usize, f.height as usize);
                let fmt = f.format;
                let plane = |i: usize, row_len: usize, rows: usize| -> Vec<u8> {
                    let mut v = Vec::with_capacity(row_len * rows);
                    for y in 0..rows {
                        let p = f.data[i].add(y * f.linesize[i] as usize);
                        v.extend_from_slice(std::slice::from_raw_parts(p, row_len));
                    }
                    v
                };
                let yuv420 = [
                    ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32,
                    ffi::AVPixelFormat::AV_PIX_FMT_YUVJ420P as i32,
                ];
                let frame = if yuv420.contains(&fmt) {
                    DecodedFrame {
                        width: w as u32,
                        height: h as u32,
                        pts: f.pts,
                        keyframe: f.flags & ffi::AV_FRAME_FLAG_KEY as i32 != 0,
                        y: plane(0, w, h),
                        u: plane(1, w / 2, h / 2),
                        v: plane(2, w / 2, h / 2),
                    }
                } else if fmt == ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32 {
                    let uv = plane(1, w, h / 2);
                    DecodedFrame {
                        width: w as u32,
                        height: h as u32,
                        pts: f.pts,
                        keyframe: f.flags & ffi::AV_FRAME_FLAG_KEY as i32 != 0,
                        y: plane(0, w, h),
                        u: uv.iter().step_by(2).copied().collect(),
                        v: uv.iter().skip(1).step_by(2).copied().collect(),
                    }
                } else {
                    ffi::av_frame_unref(self.frame);
                    return Err(FfmpegError::msg(format!(
                        "unsupported decoded pixel format {fmt}"
                    )));
                };
                ffi::av_frame_unref(self.frame);
                out.push(frame);
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: owned pointers, null-safe free functions.
        unsafe {
            ffi::av_frame_free(&mut self.frame);
            ffi::av_packet_free(&mut self.pkt);
            ffi::avcodec_free_context(&mut self.ctx);
        }
    }
}
