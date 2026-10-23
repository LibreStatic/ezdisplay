//! Fakes for tests of session code and platform backends.

use crate::source::{
    CursorSink, DisplayMode, EncodedFrame, OutputInfo, SourceError, VideoSource, VirtualDisplay,
};
use crate::time::now_us;
use ezd_proto::{Codec, CursorShape, CursorState, EncoderCaps, StreamConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A [`VirtualDisplay`] that only records its lifetime in a counter.
#[derive(Debug)]
pub struct FakeVirtualDisplay {
    info: OutputInfo,
    live: Arc<AtomicUsize>,
}

impl FakeVirtualDisplay {
    /// Creates a display and increments `live`; dropping decrements it.
    pub fn create_counted(mode: DisplayMode, live: Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::SeqCst);
        FakeVirtualDisplay {
            info: OutputInfo {
                name: "Fake-1".into(),
                mode,
                origin: (0, 0),
                adapter: None,
            },
            live,
        }
    }
}

impl VirtualDisplay for FakeVirtualDisplay {
    fn create(mode: DisplayMode) -> Result<Self, SourceError> {
        Ok(Self::create_counted(mode, Arc::default()))
    }
    fn output(&self) -> &OutputInfo {
        &self.info
    }
}

impl Drop for FakeVirtualDisplay {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A [`VideoSource`] producing fake "encoded" frames (no codec involved):
/// frame data is 8 bytes of the frame counter followed by padding.
/// Every `keyframe_interval`-th frame is a keyframe; a keyframe request forces
/// the next one. It also emits one cursor shape and a moving cursor.
#[derive(Debug)]
pub struct ScriptedSource {
    /// Frame interval.
    pub interval: Duration,
    /// Keyframe period in frames (0 = only the first).
    pub keyframe_interval: u64,
    /// Padding bytes per frame.
    pub payload_len: usize,
    n: u64,
    force_key: bool,
    cursor: Option<CursorSink>,
    config: Vec<u8>,
    sent_config: bool,
    /// Set by `stop`.
    pub stopped: bool,
}

impl ScriptedSource {
    /// Creates a source producing `payload_len`-byte frames every `interval`.
    pub fn new(interval: Duration, payload_len: usize) -> Self {
        ScriptedSource {
            interval,
            keyframe_interval: 0,
            payload_len,
            n: 0,
            force_key: false,
            cursor: None,
            config: vec![0, 0, 0, 1, 0x67, 1, 2, 3],
            sent_config: false,
            stopped: false,
        }
    }
}

impl VideoSource for ScriptedSource {
    fn encoders(&self) -> Vec<EncoderCaps> {
        vec![EncoderCaps {
            codec: Codec::H264,
            name: "scripted".into(),
            hardware: false,
        }]
    }
    fn start(&mut self, _cfg: &StreamConfig, cursor: CursorSink) -> Result<(), SourceError> {
        let _ = cursor.set_shape(CursorShape {
            shape_id: 1,
            width: 2,
            height: 2,
            hot_x: 0,
            hot_y: 0,
            rgba: vec![255; 16],
        });
        self.cursor = Some(cursor);
        Ok(())
    }
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<EncodedFrame>, SourceError> {
        std::thread::sleep(self.interval.min(timeout));
        let capture_us = now_us();
        let n = self.n;
        self.n += 1;
        let keyframe = n == 0
            || self.force_key
            || (self.keyframe_interval != 0 && n % self.keyframe_interval == 0);
        self.force_key = false;
        if let Some(c) = &self.cursor {
            let _ = c.update(CursorState {
                x: n as i32,
                y: 0,
                visible: true,
                shape_id: 1,
                ts_us: capture_us,
            });
        }
        let mut data = n.to_le_bytes().to_vec();
        data.resize(8 + self.payload_len, 0xAB);
        let codec_config = (!self.sent_config).then(|| self.config.clone());
        self.sent_config = true;
        Ok(Some(EncodedFrame {
            data,
            keyframe,
            codec_config,
            capture_us,
            encoded_us: now_us(),
            frame_id: Some(n),
        }))
    }
    fn request_keyframe(&mut self) {
        self.force_key = true;
    }
    fn stop(&mut self) {
        self.stopped = true;
    }
}
