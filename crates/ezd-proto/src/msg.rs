//! Message definitions. Each message is the postcard encoding of a struct,
//! carried in a frame addressed by `(channel, kind)`; see [`Message::channel`]
//! and [`Message::kind`] for the table.

use crate::frame::Channel;
use crate::{ProtoError, Result};
use serde::{Deserialize, Serialize};

/// Which side of the connection a peer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// The PC side: captures, encodes, sends video.
    Host,
    /// The tablet side: decodes and displays.
    Client,
}

/// Video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Codec {
    /// H.264 / AVC (Annex-B on the wire).
    H264,
    /// H.265 / HEVC (Annex-B on the wire).
    Hevc,
    /// AV1 (low-overhead OBU stream).
    Av1,
}

/// First message in each direction. Handshake fails on a major version mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Sender's [`PROTOCOL_MAJOR`](crate::PROTOCOL_MAJOR).
    pub proto_major: u16,
    /// Sender's [`PROTOCOL_MINOR`](crate::PROTOCOL_MINOR).
    pub proto_minor: u16,
    /// Sender role.
    pub role: Role,
    /// Application version string, informational.
    pub app_version: String,
    /// Human readable device name.
    pub device_name: String,
    /// Optional pairing token (reserved for later phases).
    pub token: Option<[u8; 32]>,
}

impl Hello {
    /// Builds a Hello carrying this build's protocol version.
    pub fn new(role: Role, app_version: impl Into<String>, device_name: impl Into<String>) -> Self {
        Hello {
            proto_major: crate::PROTOCOL_MAJOR,
            proto_minor: crate::PROTOCOL_MINOR,
            role,
            app_version: app_version.into(),
            device_name: device_name.into(),
            token: None,
        }
    }

    /// Fails with [`ProtoError::VersionMismatch`] if the peer's major differs from ours.
    pub fn check_compatible(&self) -> Result<()> {
        if self.proto_major != crate::PROTOCOL_MAJOR {
            return Err(ProtoError::VersionMismatch {
                local: crate::PROTOCOL_MAJOR,
                peer: self.proto_major,
            });
        }
        Ok(())
    }
}

/// Client display description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// Native width in pixels.
    pub width_px: u32,
    /// Native height in pixels.
    pub height_px: u32,
    /// Density in dots per inch.
    pub density_dpi: u32,
    /// Supported refresh rates in millihertz (60 Hz = 60000).
    pub refresh_rates_mhz: Vec<u32>,
    /// Currently active refresh rate in millihertz.
    pub current_refresh_mhz: u32,
}

/// One client-side decoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecoderCaps {
    /// Codec handled.
    pub codec: Codec,
    /// Implementation name (e.g. MediaCodec component name).
    pub name: String,
    /// Hardware accelerated.
    pub hardware: bool,
    /// Supports a low-latency mode (no frame reordering / buffering).
    pub low_latency: bool,
    /// Maximum width in pixels, 0 = unknown/unlimited.
    pub max_width: u32,
    /// Maximum height in pixels, 0 = unknown/unlimited.
    pub max_height: u32,
    /// Maximum frame rate at max resolution, 0 = unknown/unlimited.
    pub max_fps: u32,
}

/// Client capabilities (client to host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCaps {
    /// Display description.
    pub display: DisplayInfo,
    /// Available decoders.
    pub decoders: Vec<DecoderCaps>,
    /// Stylus supported.
    pub stylus: bool,
    /// Maximum simultaneous touch points.
    pub max_touch_points: u8,
}

/// One host-side encoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncoderCaps {
    /// Codec produced.
    pub codec: Codec,
    /// Encoder name (e.g. `h264_nvenc`).
    pub name: String,
    /// Hardware accelerated.
    pub hardware: bool,
}

/// Host capabilities (host to client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCaps {
    /// Operating system description.
    pub os: String,
    /// GPU description.
    pub gpu: String,
    /// Available encoders.
    pub encoders: Vec<EncoderCaps>,
}

/// Negotiated stream parameters (host to client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Stream width in pixels.
    pub width: u32,
    /// Stream height in pixels.
    pub height: u32,
    /// Frame rate in millihertz.
    pub refresh_mhz: u32,
    /// Codec.
    pub codec: Codec,
    /// Target bitrate in kbit/s.
    pub bitrate_kbps: u32,
    /// Name of the chosen host encoder.
    pub encoder_name: String,
}

/// Echo request. Either side may send it; the peer answers with [`Pong`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    /// Sequence number, echoed.
    pub seq: u32,
    /// Sender's monotonic clock at send (t0), microseconds.
    pub t0_us: u64,
}

/// Echo response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    /// Echoed sequence number.
    pub seq: u32,
    /// Echoed `t0_us` from the ping (pinger's clock).
    pub t0_us: u64,
    /// Responder's clock when the ping was received.
    pub t1_us: u64,
    /// Responder's clock when the pong was sent.
    pub t2_us: u64,
}

/// Periodic client statistics (client to host).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientStats {
    /// Length of the measurement window in milliseconds.
    pub window_ms: u32,
    /// Frames received in the window.
    pub frames_received: u32,
    /// Frames successfully decoded.
    pub frames_decoded: u32,
    /// Frames dropped by the client.
    pub frames_dropped: u32,
    /// Median decode time, microseconds.
    pub decode_us_p50: u32,
    /// 95th percentile decode time, microseconds.
    pub decode_us_p95: u32,
    /// Median end-to-end (capture to decoded) latency, microseconds.
    pub e2e_us_p50: u32,
    /// 95th percentile end-to-end latency, microseconds.
    pub e2e_us_p95: u32,
}

/// Acknowledges that a video frame was fully received (client to host).
///
/// Sent as soon as a non-config `VideoFrame` is reassembled, before decoding.
/// `CODEC_CONFIG` frames are never acked. It releases the host's video credit
/// and lets the host measure link transfer time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameAck {
    /// `frame_id` of the received frame (releases all outstanding ids up to it).
    pub frame_id: u64,
    /// Client monotonic clock when the frame was fully received, microseconds.
    pub received_us: u64,
}

/// Connection refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reject {
    /// Human readable reason.
    pub reason: String,
}

/// Orderly shutdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bye {
    /// Human readable reason.
    pub reason: String,
}

/// One encoded video access unit (video channel).
///
/// `data` is Annex-B for H.264/HEVC. Frames flagged [`VideoFrame::CODEC_CONFIG`]
/// carry only parameter sets (SPS/PPS, plus VPS for HEVC) and are sent before
/// the first keyframe and whenever the parameter sets change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFrame {
    /// Monotonic frame counter (gaps allowed when the host coalesces).
    pub frame_id: u64,
    /// Bit set of [`VideoFrame::KEYFRAME`] and [`VideoFrame::CODEC_CONFIG`].
    pub flags: u32,
    /// Host monotonic time of capture, microseconds.
    pub capture_us: u64,
    /// Host monotonic time when encoding finished, microseconds.
    pub encoded_us: u64,
    /// Encoded bitstream.
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

impl VideoFrame {
    /// Flag: independently decodable frame.
    pub const KEYFRAME: u32 = 1;
    /// Flag: `data` holds codec configuration (parameter sets) only.
    pub const CODEC_CONFIG: u32 = 2;

    /// True if [`Self::KEYFRAME`] is set.
    pub fn is_keyframe(&self) -> bool {
        self.flags & Self::KEYFRAME != 0
    }

    /// True if [`Self::CODEC_CONFIG`] is set.
    pub fn is_codec_config(&self) -> bool {
        self.flags & Self::CODEC_CONFIG != 0
    }
}

/// Cursor bitmap (cursor channel). Sent once per `shape_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorShape {
    /// Identifier referenced by [`CursorState::shape_id`].
    pub shape_id: u32,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// Hotspot x offset in pixels.
    pub hot_x: u16,
    /// Hotspot y offset in pixels.
    pub hot_y: u16,
    /// Straight (non-premultiplied) RGBA8, row-major, no padding: `width*height*4` bytes.
    #[serde(with = "serde_bytes")]
    pub rgba: Vec<u8>,
}

/// Cursor position (cursor channel). Latest wins; may be coalesced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    /// X in virtual-display pixels; may be outside the display.
    pub x: i32,
    /// Y in virtual-display pixels; may be outside the display.
    pub y: i32,
    /// Cursor visible.
    pub visible: bool,
    /// Shape to draw.
    pub shape_id: u32,
    /// Host monotonic timestamp, microseconds.
    pub ts_us: u64,
}

/// Any message of protocol 0.1.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    /// Control/1.
    Hello(Hello),
    /// Control/2.
    ClientCaps(ClientCaps),
    /// Control/3.
    HostCaps(HostCaps),
    /// Control/4.
    StreamConfig(StreamConfig),
    /// Control/5, empty payload: decoder is ready.
    ConfigAck,
    /// Control/6.
    Ping(Ping),
    /// Control/7.
    Pong(Pong),
    /// Control/8, empty payload.
    RequestKeyframe,
    /// Control/9.
    ClientStats(ClientStats),
    /// Control/10.
    Reject(Reject),
    /// Control/11.
    Bye(Bye),
    /// Control/12.
    FrameAck(FrameAck),
    /// Cursor/1.
    CursorShape(CursorShape),
    /// Cursor/2.
    CursorState(CursorState),
    /// Video/1.
    VideoFrame(VideoFrame),
}

impl Message {
    /// Channel this message travels on.
    pub fn channel(&self) -> Channel {
        match self {
            Message::CursorShape(_) | Message::CursorState(_) => Channel::Cursor,
            Message::VideoFrame(_) => Channel::Video,
            _ => Channel::Control,
        }
    }

    /// Kind id within the channel.
    pub fn kind(&self) -> u8 {
        match self {
            Message::Hello(_) => 1,
            Message::ClientCaps(_) => 2,
            Message::HostCaps(_) => 3,
            Message::StreamConfig(_) => 4,
            Message::ConfigAck => 5,
            Message::Ping(_) => 6,
            Message::Pong(_) => 7,
            Message::RequestKeyframe => 8,
            Message::ClientStats(_) => 9,
            Message::Reject(_) => 10,
            Message::Bye(_) => 11,
            Message::FrameAck(_) => 12,
            Message::CursorShape(_) => 1,
            Message::CursorState(_) => 2,
            Message::VideoFrame(_) => 1,
        }
    }
}

fn enc<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    postcard::to_allocvec(v).map_err(ProtoError::Encode)
}

/// Encodes a message into `(channel, kind, payload)`; the payload is the bare
/// postcard encoding, to be wrapped by [`FrameWriter`](crate::FrameWriter).
pub fn encode_message(msg: &Message) -> Result<(Channel, u8, Vec<u8>)> {
    let payload = match msg {
        Message::Hello(m) => enc(m)?,
        Message::ClientCaps(m) => enc(m)?,
        Message::HostCaps(m) => enc(m)?,
        Message::StreamConfig(m) => enc(m)?,
        Message::ConfigAck | Message::RequestKeyframe => Vec::new(),
        Message::Ping(m) => enc(m)?,
        Message::Pong(m) => enc(m)?,
        Message::ClientStats(m) => enc(m)?,
        Message::Reject(m) => enc(m)?,
        Message::Bye(m) => enc(m)?,
        Message::FrameAck(m) => enc(m)?,
        Message::CursorShape(m) => enc(m)?,
        Message::CursorState(m) => enc(m)?,
        Message::VideoFrame(m) => enc(m)?,
    };
    Ok((msg.channel(), msg.kind(), payload))
}

/// Decodes a payload for the raw `(channel, kind)`.
///
/// Returns `Ok(None)` for pairs unknown to this version (forward compatibility)
/// and `Err` only when a known pair has a malformed payload.
pub fn decode_message(channel: u8, kind: u8, payload: &[u8]) -> Result<Option<Message>> {
    fn dec<'a, T: Deserialize<'a>>(channel: u8, kind: u8, p: &'a [u8]) -> Result<T> {
        postcard::from_bytes(p).map_err(|source| ProtoError::Decode {
            channel,
            kind,
            source,
        })
    }
    let (c, k) = (channel, kind);
    let Some(ch) = Channel::from_u8(channel) else {
        return Ok(None);
    };
    Ok(Some(match (ch, kind) {
        (Channel::Control, 1) => Message::Hello(dec(c, k, payload)?),
        (Channel::Control, 2) => Message::ClientCaps(dec(c, k, payload)?),
        (Channel::Control, 3) => Message::HostCaps(dec(c, k, payload)?),
        (Channel::Control, 4) => Message::StreamConfig(dec(c, k, payload)?),
        (Channel::Control, 5) => Message::ConfigAck,
        (Channel::Control, 6) => Message::Ping(dec(c, k, payload)?),
        (Channel::Control, 7) => Message::Pong(dec(c, k, payload)?),
        (Channel::Control, 8) => Message::RequestKeyframe,
        (Channel::Control, 9) => Message::ClientStats(dec(c, k, payload)?),
        (Channel::Control, 10) => Message::Reject(dec(c, k, payload)?),
        (Channel::Control, 11) => Message::Bye(dec(c, k, payload)?),
        (Channel::Control, 12) => Message::FrameAck(dec(c, k, payload)?),
        (Channel::Cursor, 1) => Message::CursorShape(dec(c, k, payload)?),
        (Channel::Cursor, 2) => Message::CursorState(dec(c, k, payload)?),
        (Channel::Video, 1) => Message::VideoFrame(dec(c, k, payload)?),
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sample of every message.
    pub(crate) fn samples() -> Vec<Message> {
        vec![
            Message::Hello(Hello {
                token: Some([7; 32]),
                ..Hello::new(Role::Client, "0.1.0", "Pixel Tablet")
            }),
            Message::Hello(Hello::new(Role::Host, "0.1.0", "pc")),
            Message::ClientCaps(ClientCaps {
                display: DisplayInfo {
                    width_px: 2560,
                    height_px: 1600,
                    density_dpi: 320,
                    refresh_rates_mhz: vec![60000, 120000],
                    current_refresh_mhz: 60000,
                },
                decoders: vec![DecoderCaps {
                    codec: Codec::Hevc,
                    name: "c2.qti.hevc.decoder".into(),
                    hardware: true,
                    low_latency: true,
                    max_width: 4096,
                    max_height: 4096,
                    max_fps: 120,
                }],
                stylus: true,
                max_touch_points: 10,
            }),
            Message::HostCaps(HostCaps {
                os: "linux".into(),
                gpu: "NVIDIA GTX 1650".into(),
                encoders: vec![EncoderCaps {
                    codec: Codec::H264,
                    name: "h264_nvenc".into(),
                    hardware: true,
                }],
            }),
            Message::StreamConfig(StreamConfig {
                width: 1920,
                height: 1080,
                refresh_mhz: 120000,
                codec: Codec::Av1,
                bitrate_kbps: 20000,
                encoder_name: "x".into(),
            }),
            Message::ConfigAck,
            Message::Ping(Ping {
                seq: 1,
                t0_us: u64::MAX,
            }),
            Message::Pong(Pong {
                seq: 1,
                t0_us: 2,
                t1_us: 3,
                t2_us: 4,
            }),
            Message::RequestKeyframe,
            Message::ClientStats(ClientStats {
                window_ms: 1000,
                frames_received: 60,
                frames_decoded: 59,
                frames_dropped: 1,
                decode_us_p50: 2000,
                decode_us_p95: 4000,
                e2e_us_p50: 15000,
                e2e_us_p95: 30000,
            }),
            Message::Reject(Reject {
                reason: "no".into(),
            }),
            Message::Bye(Bye {
                reason: "bye".into(),
            }),
            Message::FrameAck(FrameAck {
                frame_id: 1 << 33,
                received_us: 99,
            }),
            Message::CursorShape(CursorShape {
                shape_id: 9,
                width: 2,
                height: 1,
                hot_x: 0,
                hot_y: 0,
                rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
            }),
            Message::CursorState(CursorState {
                x: -5,
                y: 3000,
                visible: false,
                shape_id: 9,
                ts_us: 11,
            }),
            Message::VideoFrame(VideoFrame {
                frame_id: 1 << 40,
                flags: VideoFrame::KEYFRAME | VideoFrame::CODEC_CONFIG,
                capture_us: 5,
                encoded_us: 6,
                data: vec![0, 0, 0, 1, 0x67, 1, 2, 3],
            }),
        ]
    }

    #[test]
    fn every_message_roundtrips() {
        for m in samples() {
            let (ch, kind, payload) = encode_message(&m).unwrap();
            assert_eq!((ch, kind), (m.channel(), m.kind()));
            let back = decode_message(ch as u8, kind, &payload).unwrap().unwrap();
            assert_eq!(back, m);
        }
    }

    #[test]
    fn kinds_are_unique_per_channel() {
        let mut seen = std::collections::HashSet::new();
        for m in samples() {
            let key = (m.channel() as u8, m.kind());
            // Samples contain a duplicate Hello; everything else must be unique.
            if !matches!(m, Message::Hello(_)) {
                assert!(seen.insert(key), "duplicate {key:?}");
            }
        }
    }

    #[test]
    fn unknown_pairs_decode_to_none() {
        assert_eq!(decode_message(0, 99, b"x").unwrap(), None);
        assert_eq!(decode_message(77, 1, b"x").unwrap(), None);
        assert_eq!(decode_message(Channel::Input as u8, 1, b"x").unwrap(), None);
    }

    #[test]
    fn video_data_is_bytes_on_the_wire() {
        // serde_bytes keeps the encoding compact: varint length + raw bytes.
        let m = VideoFrame {
            frame_id: 0,
            flags: 0,
            capture_us: 0,
            encoded_us: 0,
            data: vec![255; 200],
        };
        let (_, _, p) = encode_message(&Message::VideoFrame(m)).unwrap();
        assert_eq!(p.len(), 4 + 2 + 200);
    }

    #[test]
    fn version_check() {
        let mut h = Hello::new(Role::Host, "x", "y");
        assert!(h.check_compatible().is_ok());
        h.proto_major += 1;
        assert!(matches!(
            h.check_compatible(),
            Err(ProtoError::VersionMismatch { .. })
        ));
    }
}
