//! Traits implemented by platform backends.

use ezd_link::{MuxClosed, MuxSender};
use ezd_proto::{CursorShape, CursorState, EncoderCaps, StreamConfig};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Error returned by [`VideoSource`] / [`VirtualDisplay`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Free-form failure description.
    #[error("{0}")]
    Msg(String),
    /// Wrapped backend error.
    #[error(transparent)]
    Backend(Box<dyn std::error::Error + Send + Sync>),
}

impl SourceError {
    /// Builds a [`SourceError::Msg`].
    pub fn msg(s: impl Into<String>) -> Self {
        SourceError::Msg(s.into())
    }
    /// Wraps any error.
    pub fn backend(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        SourceError::Backend(Box::new(e))
    }
}

/// One encoded access unit produced by a [`VideoSource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Annex-B bitstream (H.264/HEVC) of one access unit.
    pub data: Vec<u8>,
    /// Independently decodable frame.
    pub keyframe: bool,
    /// Parameter sets (Annex-B SPS/PPS, plus VPS for HEVC) when they are
    /// known/changed. Set on the first frame and whenever they change; the
    /// session sends them as a `CODEC_CONFIG` frame before this frame and
    /// suppresses repeats of identical bytes.
    pub codec_config: Option<Vec<u8>>,
    /// [`now_us`](crate::now_us) when the content was captured/rendered.
    pub capture_us: u64,
    /// [`now_us`](crate::now_us) when encoding finished.
    pub encoded_us: u64,
    /// Optional source-provided frame id (e.g. a capture counter that may skip
    /// numbers when frames were coalesced). `None` makes the session number
    /// frames sequentially.
    pub frame_id: Option<u64>,
}

/// Cloneable handle through which a source publishes cursor data.
///
/// Shapes are deduplicated by `shape_id`: the bitmap is sent once per session.
/// States are latest-wins (see [`ezd_link::mux`]). The shape of a state's
/// `shape_id` is always written before the state.
#[derive(Clone)]
pub struct CursorSink {
    tx: Option<MuxSender>,
    sent_shapes: Arc<Mutex<HashSet<u32>>>,
}

impl std::fmt::Debug for CursorSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorSink")
            .field("connected", &self.tx.is_some())
            .finish()
    }
}

impl CursorSink {
    /// Creates a sink that sends through `tx`.
    pub fn new(tx: MuxSender) -> Self {
        CursorSink {
            tx: Some(tx),
            sent_shapes: Default::default(),
        }
    }

    /// A sink that discards everything (for tests and diagnostics).
    pub fn null() -> Self {
        CursorSink {
            tx: None,
            sent_shapes: Default::default(),
        }
    }

    /// True if a shape with this id was already sent, so the caller can skip
    /// building the bitmap.
    pub fn has_shape(&self, shape_id: u32) -> bool {
        self.sent_shapes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&shape_id)
    }

    /// Sends `shape` unless its id was already sent. Returns whether it was sent.
    pub fn set_shape(&self, shape: CursorShape) -> Result<bool, MuxClosed> {
        let Some(tx) = &self.tx else { return Ok(false) };
        let mut set = self.sent_shapes.lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(&shape.shape_id) {
            return Ok(false);
        }
        tx.send(&ezd_proto::Message::CursorShape(shape.clone()))
            .map_err(|_| MuxClosed)?;
        set.insert(shape.shape_id);
        Ok(true)
    }

    /// Publishes the current cursor position/visibility (latest wins).
    pub fn update(&self, state: CursorState) -> Result<(), MuxClosed> {
        match &self.tx {
            Some(tx) => tx.send_cursor_state(state).map_err(|_| MuxClosed),
            None => Ok(()),
        }
    }
}

/// A capture + encode pipeline for one virtual display.
///
/// Implemented per platform (and by `ezd_ffmpeg::SwVideoSource` for tests).
/// The session owns the source on a dedicated thread and calls, in order:
/// `start`, then repeatedly `request_keyframe` (if the client asked) and
/// `next_frame`, and finally `stop`.
pub trait VideoSource: Send {
    /// Encoders this host can offer for negotiation (hardware first is fine;
    /// negotiation picks the first hardware encoder for the chosen codec).
    fn encoders(&self) -> Vec<EncoderCaps>;

    /// Human readable GPU description for `HostCaps` (default "unknown").
    fn gpu_description(&self) -> String {
        "unknown".into()
    }

    /// Starts capture and encoding with the negotiated configuration. The
    /// first produced frame must be a keyframe carrying `codec_config`.
    /// `cursor` stays valid until `stop`.
    fn start(&mut self, cfg: &StreamConfig, cursor: CursorSink) -> Result<(), SourceError>;

    /// Blocks up to `timeout` for the next encoded frame.
    ///
    /// Called from a dedicated thread, *only when the link can take a frame*.
    /// Must return the **newest** available content: if several frames were
    /// produced while the link was busy, skip to the latest (coalesce before
    /// encoding; never drop already-encoded P-frames, that corrupts the
    /// stream). `Ok(None)` means nothing new yet (timeout).
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<EncodedFrame>, SourceError>;

    /// Makes the next frame returned by `next_frame` a keyframe.
    fn request_keyframe(&mut self);

    /// Stops capture and releases resources. Also called if `start` failed.
    fn stop(&mut self);
}

/// Mode of a virtual display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Refresh rate in millihertz.
    pub refresh_mhz: u32,
}

/// Description of a created virtual output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    /// Platform name of the output (e.g. `Virtual-1`, `\\.\DISPLAY5`).
    pub name: String,
    /// Active mode.
    pub mode: DisplayMode,
    /// Position of the output in the desktop coordinate space.
    pub origin: (i32, i32),
    /// Identifier of the GPU/adapter that owns the output (render node path,
    /// LUID, ...). Capture and encode must run on this same adapter for zero-copy.
    pub adapter: Option<String>,
}

/// A virtual monitor that exists for as long as the value lives.
///
/// Implemented by the Linux (KDE) and Windows (IDD) backends: `create`
/// instantiates an output with the requested mode, and dropping the value
/// destroys it. The output description tells the capture pipeline what to
/// capture and on which adapter.
pub trait VirtualDisplay: Send + Sized {
    /// Creates a virtual output with `mode`.
    fn create(mode: DisplayMode) -> Result<Self, SourceError>;
    /// Describes the created output.
    fn output(&self) -> &OutputInfo;
}
