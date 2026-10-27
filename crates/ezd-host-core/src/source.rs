//! Traits implemented by platform backends.

use ezd_link::{MuxClosed, MuxSender};
use ezd_proto::{
    AudioVolume, Codec, CursorShape, CursorState, DynamicRange, EncoderAttempt, EncoderCaps,
    Mastering, QualityLevel, RateControl, StreamColor, StreamConfig,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Error returned by [`VideoSource`] / [`VirtualDisplay`] / [`AudioSource`] / [`AudioEncoder`]
/// implementations.
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

/// HDR10 properties of the captured content, reported by a source whose output is HDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceHdr {
    /// Mastering display colour volume of the output, if known.
    pub mastering: Option<Mastering>,
    /// Maximum content light level in cd/m2 (0 = unknown).
    pub max_cll: u16,
    /// Maximum frame-average light level in cd/m2 (0 = unknown).
    pub max_fall: u16,
}

impl SourceHdr {
    /// The HDR10 stream colour description carrying these values.
    pub fn stream_color(&self) -> StreamColor {
        StreamColor::hdr10(self.mastering, self.max_cll, self.max_fall)
    }
}

/// What a source's encoder tried and what it runs with, for the `StreamReport` sent to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderSetup {
    /// Encoders and option sets tried, in order.
    pub attempts: Vec<EncoderAttempt>,
    /// Rate control of the encoder that opened.
    pub rate: RateControl,
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

    /// HDR properties of the captured output, `None` when it is SDR (the default). Queried
    /// before the handshake; an HDR source lets the session negotiate HDR10.
    fn source_hdr(&self) -> Option<SourceHdr> {
        None
    }

    /// Codecs this host can encode in 10 bits (HEVC Main10 / AV1 10-bit) with an HDR10-capable
    /// encoder, queried before the handshake (default: none).
    fn main10_codecs(&self) -> Vec<Codec> {
        Vec::new()
    }

    /// Tells the source the negotiated dynamic range and the stream colour, right before
    /// [`start`](Self::start). `StreamConfig::codec` already matches an HDR10 range. Default:
    /// ignored (SDR BT.709 stream).
    fn set_dynamic_range(&mut self, _range: DynamicRange, _color: &StreamColor) {}

    /// Tells the source the quality level the host applies, right before
    /// [`start`](Self::start). Default: ignored (the normal, rate-controlled stream).
    fn set_quality(&mut self, _quality: QualityLevel) {}

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

    /// The output the stream shows, valid after `start` (default: unknown). Input injection
    /// uses it to map normalized coordinates onto the desktop.
    fn output(&self) -> Option<OutputInfo> {
        None
    }

    /// The setup of the encoder that opened since the last call, `None` when nothing changed
    /// (the default for sources that do not report). The session polls it after every frame
    /// from `next_frame` and sends it to the client before that frame.
    fn take_encoder_setup(&mut self) -> Option<EncoderSetup> {
        None
    }

    /// Makes the next frame returned by `next_frame` a keyframe.
    fn request_keyframe(&mut self);

    /// Stops capture and releases resources. Also called if `start` failed.
    fn stop(&mut self);
}

/// A block of captured audio, see [`AudioSource::next_samples`].
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBlock {
    /// [`now_us`](crate::now_us) of the first sample (capture time).
    pub pts_us: u64,
    /// Interleaved samples, 48 kHz, in -1.0..=1.0.
    pub samples: Vec<f32>,
}

/// Capture of the PC audio that is streamed to the client.
///
/// Implemented per platform. The session owns the source on a dedicated thread ("ezd-audio") and
/// calls `start`, then repeatedly `next_samples`, and finally `stop`. A failure ends the audio
/// only; the video session goes on.
pub trait AudioSource: Send {
    /// Starts capturing 48 kHz audio with `channels` interleaved `f32` channels (the source
    /// downmixes or upmixes as needed).
    fn start(&mut self, channels: u8) -> Result<(), SourceError>;

    /// Blocks up to `timeout` for the next block of captured samples (any length; the session
    /// re-chunks them). `Ok(None)` means nothing captured yet (timeout, or silence on a backend
    /// that produces none).
    fn next_samples(&mut self, timeout: Duration) -> Result<Option<AudioBlock>, SourceError>;

    /// Stops capture and releases resources. Also called if `start` failed.
    fn stop(&mut self);

    /// The output volume if it changed since the last call (the user moved a slider on this
    /// host), which the session forwards to the client. Polled by the audio thread on every
    /// iteration, so it must not block. Default: no volume control. The session drops values
    /// that merely echo the one it applied through [`set_volume`](Self::set_volume).
    fn take_volume_change(&mut self) -> Option<AudioVolume> {
        None
    }

    /// Applies the volume of the client. The captured samples stay at full level; this only
    /// moves the host's own control. Default: ignored.
    fn set_volume(&mut self, _volume: AudioVolume) {}
}

/// An Opus encoder for one audio stream (48 kHz, the negotiated channel count, 10 ms frames).
pub trait AudioEncoder: Send {
    /// The OpusHead identification header, sent to the client in `AudioStream::codec_config`.
    fn codec_config(&self) -> Vec<u8>;

    /// Encodes one frame of `frame_samples * channels` interleaved `f32` samples into one packet.
    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, SourceError>;
}

/// Creates the audio parts of each session. Shared by all sessions of a host process.
pub trait AudioBackend: Send + Sync {
    /// A capture source for one session, or `None` when this host cannot capture audio (the
    /// session then has no audio). Must not start capturing: that happens in
    /// [`AudioSource::start`] on the audio thread.
    fn create_source(&self) -> Option<Box<dyn AudioSource>>;

    /// True if [`create_opus_encoder`](Self::create_opus_encoder) can work (default: no).
    fn opus_available(&self) -> bool {
        false
    }

    /// An Opus encoder for `channels` (1 or 2) at 48 kHz with 10 ms frames. If it fails the
    /// session falls back to PCM when the client allows it.
    fn create_opus_encoder(&self, _channels: u8) -> Result<Box<dyn AudioEncoder>, SourceError> {
        Err(SourceError::msg("no Opus encoder on this host"))
    }
}

impl std::fmt::Debug for dyn AudioBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AudioBackend")
    }
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
