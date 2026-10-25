//! ezdisplay wire protocol.
//!
//! This crate is pure (no IO, no clocks, no OS dependencies) so it compiles for
//! Android, Windows and Linux alike. See `docs/protocol.md` for the normative
//! description.
//!
//! * [`frame`]: 8-byte framed transport, fragmentation and the IO-free
//!   [`FrameDecoder`].
//! * [`msg`]: all message structs, the [`Message`] enum and
//!   [`encode_message`] / [`decode_message`].
//! * [`negotiate`]: [`negotiate()`] turns client + host capabilities into a
//!   [`StreamConfig`].
//! * [`clock`]: NTP-style [`ClockSync`].

pub mod clock;
pub mod frame;
pub mod msg;
pub mod negotiate;

pub use clock::{ClockSync, Sample};
pub use frame::{
    Channel, DEFAULT_CHUNK_LEN, FrameDecoder, FrameHeader, FrameWriter, HEADER_LEN, Incoming,
    MAX_FRAME_LEN, MAX_MESSAGE_LEN, RawMessage, fragments,
};
pub use msg::*;
pub use negotiate::{
    DynamicRange, HdrMode, LinkKind, Prefs, SdrReason, negotiate, negotiate_hdr, pick_dynamic_range,
};

/// Wire protocol major version. A mismatch is fatal at handshake.
pub const PROTOCOL_MAJOR: u16 = 0;
/// Wire protocol minor version. Newer minors may add new `(channel, kind)` pairs.
pub const PROTOCOL_MINOR: u16 = 5;
/// First protocol minor with the input messages, `InputStatus` and `ClientOptions`.
pub const MINOR_INPUT: u16 = 3;
/// First protocol minor with `ClientHdr` and `StreamColor` (HDR10 streaming).
pub const MINOR_HDR: u16 = 5;
/// Default TCP port used for the host listener and `adb reverse`.
pub const DEFAULT_PORT: u16 = 27183;

/// Errors produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// A frame header announced a payload larger than [`MAX_FRAME_LEN`].
    #[error("frame payload of {0} bytes exceeds the {MAX_FRAME_LEN} byte limit")]
    FrameTooLarge(u32),
    /// A reassembled message exceeds [`MAX_MESSAGE_LEN`].
    #[error("message exceeds the {MAX_MESSAGE_LEN} byte limit")]
    MessageTooLarge,
    /// A message payload could not be encoded.
    #[error("encode error: {0}")]
    Encode(postcard::Error),
    /// A payload of a known `(channel, kind)` could not be decoded.
    #[error("malformed payload for channel {channel} kind {kind}: {source}")]
    Decode {
        /// Wire channel id.
        channel: u8,
        /// Wire kind id.
        kind: u8,
        /// Underlying postcard error.
        source: postcard::Error,
    },
    /// Peer speaks an incompatible protocol major version.
    #[error("protocol major version mismatch: local {local}, peer {peer}")]
    VersionMismatch {
        /// Our major version.
        local: u16,
        /// The peer's major version.
        peer: u16,
    },
    /// No codec is supported by both the client decoders and the host encoders.
    #[error("no codec supported by both client decoders and host encoders")]
    NoCommonCodec,
    /// The client display or capabilities are unusable.
    #[error("invalid capabilities: {0}")]
    InvalidCaps(&'static str),
}

/// Result alias using [`ProtoError`].
pub type Result<T> = std::result::Result<T, ProtoError>;
