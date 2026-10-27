//! Opus audio encoder on top of libavcodec's `libopus` wrapper (48 kHz, 10 ms frames).

use crate::error::{EAGAIN, FfmpegError, init_logging};
use ezd_host_core::{AudioEncoder, SourceError};
use ffmpeg_sys_next as ffi;
use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;

/// Sample rate of the audio stream.
const SAMPLE_RATE: i32 = 48_000;
/// Samples per channel in one 10 ms frame.
const FRAME_SAMPLES: usize = 480;

/// Whether this FFmpeg build has the `libopus` encoder. The experimental native `opus` encoder is
/// deliberately not used.
pub fn opus_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        // SAFETY: plain lookup with a valid NUL-terminated name.
        unsafe { !ffi::avcodec_find_encoder_by_name(c"libopus".as_ptr()).is_null() }
    })
}

/// Creates an Opus encoder for `channels` (1 or 2) at 48 kHz with 10 ms frames.
pub fn create_opus_encoder(channels: u8) -> Result<Box<dyn AudioEncoder>, SourceError> {
    OpusEncoder::open(channels)
        .map(|e| Box::new(e) as Box<dyn AudioEncoder>)
        .map_err(SourceError::backend)
}

/// The 19-byte OpusHead identification header (RFC 7845, channel mapping family 0).
pub fn opus_head(channels: u8, pre_skip: u16, input_sample_rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1); // version
    h.push(channels);
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&input_sample_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes()); // output gain
    h.push(0); // mapping family 0
    h
}

/// A `libopus` encoder producing one packet per 10 ms frame.
pub struct OpusEncoder {
    ctx: *mut ffi::AVCodecContext,
    frame: *mut ffi::AVFrame,
    pkt: *mut ffi::AVPacket,
    channels: usize,
    head: Vec<u8>,
    pts: i64,
}

// SAFETY: the FFmpeg objects are owned exclusively by this struct and only used through &mut self.
unsafe impl Send for OpusEncoder {}

impl std::fmt::Debug for OpusEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpusEncoder")
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

fn set_opt(ctx: *mut ffi::AVCodecContext, name: &CStr, value: &CStr) -> Result<(), FfmpegError> {
    // SAFETY: ctx is a valid context whose priv_data holds the encoder's options; both strings are
    // NUL-terminated.
    let r = unsafe { ffi::av_opt_set((*ctx).priv_data, name.as_ptr(), value.as_ptr(), 0) };
    if r < 0 {
        return Err(FfmpegError::av(
            format!("av_opt_set {}", name.to_string_lossy()),
            r,
        ));
    }
    Ok(())
}

impl OpusEncoder {
    /// Opens the encoder for 1 or 2 channels.
    pub fn open(channels: u8) -> Result<OpusEncoder, FfmpegError> {
        init_logging();
        if !(1..=2).contains(&channels) {
            return Err(FfmpegError::msg(format!(
                "Opus encoder supports 1 or 2 channels, not {channels}"
            )));
        }
        // SAFETY: plain lookup with a valid NUL-terminated name.
        let codec = unsafe { ffi::avcodec_find_encoder_by_name(c"libopus".as_ptr()) };
        if codec.is_null() {
            return Err(FfmpegError::msg(
                "libopus encoder not available in this FFmpeg build",
            ));
        }
        // SAFETY: `codec` is a valid encoder.
        let ctx = unsafe { ffi::avcodec_alloc_context3(codec) };
        if ctx.is_null() {
            return Err(FfmpegError::msg("avcodec_alloc_context3 failed"));
        }
        // From here on Drop releases everything that has been allocated.
        let mut enc = OpusEncoder {
            ctx,
            frame: ptr::null_mut(),
            pkt: ptr::null_mut(),
            channels: channels as usize,
            head: Vec::new(),
            pts: 0,
        };
        // SAFETY: ctx is a freshly allocated, exclusively owned AVCodecContext; we only write plain
        // fields before opening it. libopus always supports interleaved f32 input.
        let r = unsafe {
            let c = &mut *ctx;
            c.sample_rate = SAMPLE_RATE;
            c.sample_fmt = ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT;
            c.bit_rate = i64::from(channels) * 96_000;
            c.time_base = ffi::AVRational {
                num: 1,
                den: SAMPLE_RATE,
            };
            ffi::av_channel_layout_default(&mut c.ch_layout, i32::from(channels));
            c.ch_layout.nb_channels
        };
        if r != i32::from(channels) {
            return Err(FfmpegError::msg("could not set the channel layout"));
        }
        set_opt(ctx, c"frame_duration", c"10")?;
        set_opt(ctx, c"application", c"lowdelay")?;
        set_opt(ctx, c"vbr", c"on")?;
        // SAFETY: ctx is configured; no options dictionary.
        let r = unsafe { ffi::avcodec_open2(ctx, codec, ptr::null_mut()) };
        if r < 0 {
            return Err(FfmpegError::av("avcodec_open2 (libopus)", r));
        }
        // SAFETY: ctx is opened; reading plain fields and the extradata buffer it owns.
        let (frame_size, pre_skip, extradata) = unsafe {
            let c = &*ctx;
            let extra = if c.extradata.is_null() || c.extradata_size <= 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec()
            };
            (c.frame_size, c.initial_padding, extra)
        };
        if frame_size != FRAME_SAMPLES as i32 {
            return Err(FfmpegError::msg(format!(
                "libopus opened with frame size {frame_size}, expected {FRAME_SAMPLES}"
            )));
        }
        enc.head = if extradata.len() >= 19 && extradata.starts_with(b"OpusHead") {
            extradata
        } else {
            opus_head(
                channels,
                pre_skip.clamp(0, i32::from(u16::MAX)) as u16,
                SAMPLE_RATE as u32,
            )
        };
        // SAFETY: plain allocations; null is checked.
        unsafe {
            enc.frame = ffi::av_frame_alloc();
            enc.pkt = ffi::av_packet_alloc();
            if enc.frame.is_null() || enc.pkt.is_null() {
                return Err(FfmpegError::msg("av_frame_alloc/av_packet_alloc failed"));
            }
            let f = &mut *enc.frame;
            f.nb_samples = FRAME_SAMPLES as i32;
            f.format = ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT as i32;
            f.sample_rate = SAMPLE_RATE;
            let r = ffi::av_channel_layout_copy(&mut f.ch_layout, &(*ctx).ch_layout);
            if r < 0 {
                return Err(FfmpegError::av("av_channel_layout_copy", r));
            }
            let r = ffi::av_frame_get_buffer(enc.frame, 0);
            if r < 0 {
                return Err(FfmpegError::av("av_frame_get_buffer", r));
            }
        }
        Ok(enc)
    }

    fn encode_frame(&mut self, pcm: &[f32]) -> Result<Vec<u8>, FfmpegError> {
        let want = FRAME_SAMPLES * self.channels;
        if pcm.len() != want {
            return Err(FfmpegError::msg(format!(
                "Opus frame needs {want} samples, got {}",
                pcm.len()
            )));
        }
        // SAFETY: ctx is an opened encoder; frame owns a buffer of FRAME_SAMPLES interleaved f32
        // samples per channel (allocated in `open`), made writable before the copy; the packet
        // data is readable until av_packet_unref.
        unsafe {
            let r = ffi::av_frame_make_writable(self.frame);
            if r < 0 {
                return Err(FfmpegError::av("av_frame_make_writable", r));
            }
            ptr::copy_nonoverlapping(pcm.as_ptr(), (*self.frame).data[0].cast::<f32>(), want);
            (*self.frame).pts = self.pts;
            let r = ffi::avcodec_send_frame(self.ctx, self.frame);
            if r < 0 {
                return Err(FfmpegError::av("avcodec_send_frame", r));
            }
            self.pts += FRAME_SAMPLES as i64;
            let r = ffi::avcodec_receive_packet(self.ctx, self.pkt);
            if r == EAGAIN {
                // libopus with a 10 ms frame duration yields one packet per frame, so this is not
                // expected; an empty packet tells the caller there is nothing to send.
                return Ok(Vec::new());
            }
            if r < 0 {
                return Err(FfmpegError::av("avcodec_receive_packet", r));
            }
            let data =
                std::slice::from_raw_parts((*self.pkt).data, (*self.pkt).size as usize).to_vec();
            ffi::av_packet_unref(self.pkt);
            Ok(data)
        }
    }
}

impl AudioEncoder for OpusEncoder {
    fn codec_config(&self) -> Vec<u8> {
        self.head.clone()
    }

    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, SourceError> {
        self.encode_frame(pcm).map_err(SourceError::backend)
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        // SAFETY: each pointer is either null or exclusively owned by us; the FFmpeg free
        // functions accept pointers to null and reset them.
        unsafe {
            ffi::av_frame_free(&mut self.frame);
            ffi::av_packet_free(&mut self.pkt);
            ffi::avcodec_free_context(&mut self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_head_layout() {
        let h = opus_head(2, 312, 48_000);
        assert_eq!(h.len(), 19);
        assert_eq!(&h[..8], b"OpusHead");
        assert_eq!(h[8], 1);
        assert_eq!(h[9], 2);
        assert_eq!(u16::from_le_bytes([h[10], h[11]]), 312);
        assert_eq!(u32::from_le_bytes([h[12], h[13], h[14], h[15]]), 48_000);
        assert_eq!(i16::from_le_bytes([h[16], h[17]]), 0);
        assert_eq!(h[18], 0);
    }

    fn sine(channels: usize, frame: usize) -> Vec<f32> {
        (0..FRAME_SAMPLES)
            .flat_map(|i| {
                let t = (frame * FRAME_SAMPLES + i) as f32 / 48_000.0;
                let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    #[test]
    fn encodes_one_packet_per_frame() {
        if !opus_available() {
            eprintln!("libopus not available, skipping");
            return;
        }
        for channels in [1u8, 2] {
            let mut enc = create_opus_encoder(channels).unwrap();
            let head = enc.codec_config();
            assert!(head.starts_with(b"OpusHead"));
            assert_eq!(head[9], channels);
            let mut total = 0;
            for n in 0..100 {
                let p = enc.encode(&sine(channels as usize, n)).unwrap();
                assert!(!p.is_empty(), "frame {n} produced no packet");
                total += p.len();
            }
            let avg = total / 100;
            let kbps = u64::from(channels) * 96_000;
            let expect = (kbps / 8 / 100) as usize;
            assert!(
                avg > 10 && avg < expect * 2,
                "average packet {avg} bytes for {kbps} bit/s"
            );
        }
        assert!(create_opus_encoder(3).is_err());
    }

    #[test]
    fn rejects_wrong_frame_length() {
        if !opus_available() {
            return;
        }
        let mut enc = create_opus_encoder(2).unwrap();
        assert!(enc.encode(&[0.0; 480]).is_err());
    }

    #[test]
    fn decodes_back_to_the_same_level() {
        if !opus_available() {
            return;
        }
        // SAFETY: lookup only.
        let dec = unsafe { ffi::avcodec_find_decoder(ffi::AVCodecID::AV_CODEC_ID_OPUS) };
        if dec.is_null() {
            return;
        }
        let mut enc = create_opus_encoder(2).unwrap();
        let head = enc.codec_config();
        let mut rms_sum = 0.0f64;
        let mut count = 0usize;
        // SAFETY: standard decode loop on a context, packet and frame owned by this block and
        // freed at its end; extradata is allocated with the required padding.
        unsafe {
            let mut ctx = ffi::avcodec_alloc_context3(dec);
            let extra = ffi::av_mallocz(head.len() + ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize)
                .cast::<u8>();
            ptr::copy_nonoverlapping(head.as_ptr(), extra, head.len());
            (*ctx).extradata = extra;
            (*ctx).extradata_size = head.len() as i32;
            (*ctx).sample_rate = SAMPLE_RATE;
            ffi::av_channel_layout_default(&mut (*ctx).ch_layout, 2);
            assert!(ffi::avcodec_open2(ctx, dec, ptr::null_mut()) >= 0);
            let mut pkt = ffi::av_packet_alloc();
            let mut frame = ffi::av_frame_alloc();
            for n in 0..100 {
                let data = enc.encode(&sine(2, n)).unwrap();
                assert_eq!(ffi::av_new_packet(pkt, data.len() as i32), 0);
                ptr::copy_nonoverlapping(data.as_ptr(), (*pkt).data, data.len());
                assert!(ffi::avcodec_send_packet(ctx, pkt) >= 0);
                ffi::av_packet_unref(pkt);
                while ffi::avcodec_receive_frame(ctx, frame) >= 0 {
                    if n >= 20 {
                        let ns = (*frame).nb_samples as usize;
                        // The native decoder outputs planar f32; read the left channel.
                        let left = std::slice::from_raw_parts((*frame).data[0].cast::<f32>(), ns);
                        rms_sum += left.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
                        count += ns;
                    }
                    ffi::av_frame_unref(frame);
                }
            }
            ffi::av_frame_free(&mut frame);
            ffi::av_packet_free(&mut pkt);
            ffi::avcodec_free_context(&mut ctx);
        }
        assert!(count > 0, "decoder produced no samples");
        let rms = (rms_sum / count as f64).sqrt();
        // A 0.5 amplitude sine has an RMS of about 0.354.
        assert!((rms - 0.354).abs() < 0.05, "decoded RMS {rms}");
    }
}
