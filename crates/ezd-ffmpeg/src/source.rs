use crate::encoder::{Encoder, EncoderConfig, RawPixelFormat};
use crate::error::FfmpegError;
use crate::probe::software_encoders;
use ezd_host_core::synthetic::{PixelFormat, SyntheticFrames};
use ezd_host_core::{CursorSink, EncodedFrame, SourceError, VideoSource, now_us};
use ezd_proto::{Codec, CursorShape, CursorState, EncoderCaps, StreamConfig};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const CURSOR_SHAPE_ID: u32 = 1;

fn to_source_err(e: FfmpegError) -> SourceError {
    SourceError::backend(e)
}

/// A 16x16 white arrow with a black outline, straight RGBA.
fn arrow_shape() -> CursorShape {
    let (w, h) = (16usize, 16usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..=y.min(w - 1) {
            let i = (y * w + x) * 4;
            let edge = x == 0 || x == y || y == h - 1;
            let c = if edge { 0 } else { 255 };
            rgba[i..i + 4].copy_from_slice(&[c, c, c, 255]);
        }
    }
    CursorShape {
        shape_id: CURSOR_SHAPE_ID,
        width: w as u16,
        height: h as u16,
        hot_x: 0,
        hot_y: 0,
        rgba,
    }
}

/// [`VideoSource`] backed by [`SyntheticFrames`] and a software encoder.
///
/// Frames are paced at the configured refresh rate. If the consumer is late,
/// the source jumps to the newest frame number instead of queueing (coalescing
/// before encode), so `frame_id`/the number embedded in the picture can skip
/// values but always increases. A synthetic cursor (one shape, moving
/// position) is published through the [`CursorSink`] on every frame.
pub struct SwVideoSource {
    with_cursor: bool,
    state: Option<State>,
    force_key: bool,
}

struct State {
    gen_: SyntheticFrames,
    enc: Encoder,
    cursor: CursorSink,
    start: Instant,
    fps: f64,
    next_n: u64,
    raw: Vec<u8>,
    pending: VecDeque<EncodedFrame>,
    sent_config: bool,
    width: u32,
    height: u32,
}

impl Default for SwVideoSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SwVideoSource {
    /// Creates a source with the synthetic cursor enabled.
    pub fn new() -> Self {
        SwVideoSource {
            with_cursor: true,
            state: None,
            force_key: false,
        }
    }

    /// Enables or disables the synthetic cursor.
    pub fn with_cursor(mut self, on: bool) -> Self {
        self.with_cursor = on;
        self
    }
}

impl VideoSource for SwVideoSource {
    fn encoders(&self) -> Vec<EncoderCaps> {
        software_encoders()
            .into_iter()
            .map(|n| EncoderCaps {
                codec: Codec::H264,
                name: n.to_string(),
                hardware: false,
            })
            .collect()
    }

    fn gpu_description(&self) -> String {
        "none (software synthetic source)".into()
    }

    fn start(&mut self, cfg: &StreamConfig, cursor: CursorSink) -> Result<(), SourceError> {
        if cfg.codec != Codec::H264 {
            return Err(SourceError::msg("SwVideoSource only produces H.264"));
        }
        let gen_ = SyntheticFrames::new(cfg.width, cfg.height, PixelFormat::I420)
            .map_err(SourceError::backend)?;
        let mut ec = EncoderConfig::new(
            &cfg.encoder_name,
            cfg.width,
            cfg.height,
            cfg.refresh_mhz,
            cfg.bitrate_kbps,
        );
        ec.pix_fmt = RawPixelFormat::I420;
        let enc = Encoder::open(ec).map_err(to_source_err)?;
        if self.with_cursor {
            let _ = cursor.set_shape(arrow_shape());
        }
        self.state = Some(State {
            gen_,
            enc,
            cursor,
            start: Instant::now(),
            fps: cfg.refresh_mhz as f64 / 1000.0,
            next_n: 0,
            raw: Vec::new(),
            pending: VecDeque::new(),
            sent_config: false,
            width: cfg.width,
            height: cfg.height,
        });
        Ok(())
    }

    fn next_frame(&mut self, timeout: Duration) -> Result<Option<EncodedFrame>, SourceError> {
        let force = std::mem::take(&mut self.force_key);
        let with_cursor = self.with_cursor;
        let Some(st) = self.state.as_mut() else {
            return Err(SourceError::msg("source not started"));
        };
        if let Some(f) = st.pending.pop_front() {
            return Ok(Some(f));
        }
        // Newest frame whose time has come; if none is due yet, wait for the next one.
        let elapsed = st.start.elapsed().as_secs_f64();
        let due = (elapsed * st.fps).floor() as u64;
        let n = if due >= st.next_n {
            due
        } else {
            let wait = Duration::from_secs_f64(st.next_n as f64 / st.fps - elapsed);
            if wait > timeout {
                std::thread::sleep(timeout);
                self.force_key |= force;
                return Ok(None);
            }
            std::thread::sleep(wait);
            st.next_n
        };
        st.next_n = n + 1;

        let capture_us = now_us();
        st.gen_.render(n, &mut st.raw);
        let key = force || !st.sent_config;
        let packet = st.enc.encode(&st.raw, key).map_err(to_source_err)?;
        let encoded_us = now_us();
        if with_cursor {
            let x = ((n * 7) % st.width as u64) as i32;
            let y = ((n * 3) % st.height as u64) as i32;
            let _ = st.cursor.update(CursorState {
                x,
                y,
                visible: true,
                shape_id: CURSOR_SHAPE_ID,
                ts_us: capture_us,
            });
        }
        let Some(p) = packet else { return Ok(None) };
        let codec_config = if st.sent_config || st.enc.extradata().is_empty() {
            None
        } else {
            Some(st.enc.extradata().to_vec())
        };
        st.sent_config = true;
        Ok(Some(EncodedFrame {
            data: p.data,
            keyframe: p.keyframe,
            codec_config,
            capture_us,
            encoded_us,
            frame_id: Some(n),
        }))
    }

    fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    fn stop(&mut self) {
        self.state = None;
    }
}
