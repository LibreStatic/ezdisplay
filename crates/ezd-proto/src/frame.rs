//! Framing: `[payload_len: u32 LE][channel: u8][kind: u8][flags: u16 LE]` + payload.
//!
//! Large messages are split into fragments: every fragment except the last has
//! the [`FLAG_MORE`] bit set. Fragments belonging to one message share the same
//! `(channel, kind)`. The mux may interleave *other* complete frames (e.g. a
//! cursor update) between the fragments of a video frame, so reassembly is keyed
//! by `(channel, kind)`; fragments of one key must stay in order.

use crate::msg::{Message, decode_message, encode_message};
use crate::{ProtoError, Result};
use std::collections::HashMap;

/// Size of the frame header in bytes.
pub const HEADER_LEN: usize = 8;
/// Maximum payload of a single frame.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;
/// Maximum size of a reassembled message.
pub const MAX_MESSAGE_LEN: usize = 32 * 1024 * 1024;
/// Default fragment size used by [`FrameWriter`] and the mux (64 KiB).
pub const DEFAULT_CHUNK_LEN: usize = 64 * 1024;
/// Flag bit 0: this frame is a fragment and the message continues in a later frame.
pub const FLAG_MORE: u16 = 1;

/// Logical channel of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Channel {
    /// Handshake, capabilities, keepalive, statistics.
    Control = 0,
    /// Input events (pen, touch, buttons, scroll; protocol 0.3).
    Input = 1,
    /// Cursor shape and position.
    Cursor = 2,
    /// Encoded video frames.
    Video = 3,
    /// Audio packets (protocol 0.6).
    Audio = 4,
}

impl Channel {
    /// Maps a wire value to a channel; `None` for unknown channels.
    pub fn from_u8(v: u8) -> Option<Channel> {
        Some(match v {
            0 => Channel::Control,
            1 => Channel::Input,
            2 => Channel::Cursor,
            3 => Channel::Video,
            4 => Channel::Audio,
            _ => return None,
        })
    }
}

/// Parsed 8-byte frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Payload length in bytes (excluding the header).
    pub payload_len: u32,
    /// Raw channel id (may be unknown to this version).
    pub channel: u8,
    /// Raw kind id.
    pub kind: u8,
    /// Flag bits; see [`FLAG_MORE`]. Unknown bits are ignored.
    pub flags: u16,
}

impl FrameHeader {
    /// Serializes the header.
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&self.payload_len.to_le_bytes());
        b[4] = self.channel;
        b[5] = self.kind;
        b[6..8].copy_from_slice(&self.flags.to_le_bytes());
        b
    }

    /// Parses a header from the first [`HEADER_LEN`] bytes of `b`.
    ///
    /// Returns `None` when `b` is shorter than a header. Does not validate the length.
    pub fn parse(b: &[u8]) -> Option<FrameHeader> {
        if b.len() < HEADER_LEN {
            return None;
        }
        Some(FrameHeader {
            payload_len: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            channel: b[4],
            kind: b[5],
            flags: u16::from_le_bytes([b[6], b[7]]),
        })
    }

    /// True if the `MORE` flag is set.
    pub fn more(&self) -> bool {
        self.flags & FLAG_MORE != 0
    }
}

/// Splits `payload` into `(flags, chunk)` pairs of at most `chunk_len` bytes.
///
/// All but the last item carry [`FLAG_MORE`]. An empty payload yields a single
/// empty fragment. `chunk_len` is clamped to `1..=MAX_FRAME_LEN`.
pub fn fragments(payload: &[u8], chunk_len: usize) -> impl Iterator<Item = (u16, &[u8])> {
    let chunk_len = chunk_len.clamp(1, MAX_FRAME_LEN as usize);
    let n = payload.len().div_ceil(chunk_len).max(1);
    (0..n).map(move |i| {
        let start = i * chunk_len;
        let end = (start + chunk_len).min(payload.len());
        let flags = if i + 1 < n { FLAG_MORE } else { 0 };
        (flags, &payload[start..end])
    })
}

/// Appends one frame (header + chunk) to `out`.
pub fn put_frame(out: &mut Vec<u8>, channel: u8, kind: u8, flags: u16, chunk: &[u8]) {
    debug_assert!(chunk.len() <= MAX_FRAME_LEN as usize);
    let h = FrameHeader {
        payload_len: chunk.len() as u32,
        channel,
        kind,
        flags,
    };
    out.extend_from_slice(&h.to_bytes());
    out.extend_from_slice(chunk);
}

/// Serializes messages into (possibly fragmented) frames.
#[derive(Debug, Clone, Copy)]
pub struct FrameWriter {
    chunk_len: usize,
}

impl Default for FrameWriter {
    fn default() -> Self {
        Self::new(DEFAULT_CHUNK_LEN)
    }
}

impl FrameWriter {
    /// Creates a writer that fragments payloads into chunks of `chunk_len` bytes.
    pub fn new(chunk_len: usize) -> Self {
        Self {
            chunk_len: chunk_len.clamp(1, MAX_FRAME_LEN as usize),
        }
    }

    /// Configured fragment size.
    pub fn chunk_len(&self) -> usize {
        self.chunk_len
    }

    /// Appends the framed encoding of `msg` to `out`.
    pub fn write_message(&self, msg: &Message, out: &mut Vec<u8>) -> Result<()> {
        let (ch, kind, payload) = encode_message(msg)?;
        self.write_raw(ch as u8, kind, &payload, out)
    }

    /// Appends already-encoded `payload` as frames to `out`.
    pub fn write_raw(
        &self,
        channel: u8,
        kind: u8,
        payload: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<()> {
        if payload.len() > MAX_MESSAGE_LEN {
            return Err(ProtoError::MessageTooLarge);
        }
        for (flags, chunk) in fragments(payload, self.chunk_len) {
            put_frame(out, channel, kind, flags, chunk);
        }
        Ok(())
    }
}

/// A reassembled message payload that has not been interpreted yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    /// Raw channel id.
    pub channel: u8,
    /// Raw kind id.
    pub kind: u8,
    /// Complete payload (fragments joined).
    pub payload: Vec<u8>,
}

/// Result of decoding one complete message.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A known message.
    Message(Message),
    /// A `(channel, kind)` this version does not know; skipped for forward compatibility.
    Unknown {
        /// Raw channel id.
        channel: u8,
        /// Raw kind id.
        kind: u8,
        /// Payload length in bytes.
        len: usize,
    },
}

/// IO-free incremental frame parser and message reassembler.
///
/// Feed arbitrary byte slices with [`push`](Self::push) (1 byte or many
/// frames at a time) and drain with [`pop`](Self::pop). After an `Err` the
/// stream is desynchronized and the connection should be closed.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    pos: usize,
    partial: HashMap<(u8, u8), Vec<u8>>,
}

impl FrameDecoder {
    /// Creates an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.pos > 0 && self.pos >= self.buf.len() / 2 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// Number of buffered bytes not yet consumed.
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Returns the next complete reassembled message payload, if any.
    pub fn pop_raw(&mut self) -> Result<Option<RawMessage>> {
        loop {
            let avail = &self.buf[self.pos..];
            let Some(h) = FrameHeader::parse(avail) else {
                return Ok(None);
            };
            if h.payload_len > MAX_FRAME_LEN {
                return Err(ProtoError::FrameTooLarge(h.payload_len));
            }
            let total = HEADER_LEN + h.payload_len as usize;
            if avail.len() < total {
                return Ok(None);
            }
            let chunk = &avail[HEADER_LEN..total];
            let key = (h.channel, h.kind);
            let msg = if h.more() {
                let entry = self.partial.entry(key).or_default();
                if entry.len() + chunk.len() > MAX_MESSAGE_LEN {
                    return Err(ProtoError::MessageTooLarge);
                }
                entry.extend_from_slice(chunk);
                None
            } else if let Some(mut acc) = self.partial.remove(&key) {
                if acc.len() + chunk.len() > MAX_MESSAGE_LEN {
                    return Err(ProtoError::MessageTooLarge);
                }
                acc.extend_from_slice(chunk);
                Some(acc)
            } else {
                Some(chunk.to_vec())
            };
            self.pos += total;
            if let Some(payload) = msg {
                return Ok(Some(RawMessage {
                    channel: h.channel,
                    kind: h.kind,
                    payload,
                }));
            }
        }
    }

    /// Returns the next complete message, decoded. Unknown `(channel, kind)`
    /// pairs yield [`Incoming::Unknown`], never an error.
    pub fn pop(&mut self) -> Result<Option<Incoming>> {
        let Some(raw) = self.pop_raw()? else {
            return Ok(None);
        };
        Ok(Some(
            match decode_message(raw.channel, raw.kind, &raw.payload)? {
                Some(m) => Incoming::Message(m),
                None => Incoming::Unknown {
                    channel: raw.channel,
                    kind: raw.kind,
                    len: raw.payload.len(),
                },
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::*;

    fn video(n: usize) -> Message {
        Message::VideoFrame(VideoFrame {
            frame_id: 7,
            flags: VideoFrame::KEYFRAME,
            capture_us: 1,
            encoded_us: 2,
            data: (0..n).map(|i| (i % 251) as u8).collect(),
        })
    }

    fn pop_all(d: &mut FrameDecoder) -> Vec<Incoming> {
        let mut v = vec![];
        while let Some(m) = d.pop().unwrap() {
            v.push(m);
        }
        v
    }

    #[test]
    fn header_layout() {
        let h = FrameHeader {
            payload_len: 0x0102_0304,
            channel: 3,
            kind: 9,
            flags: 0x0a0b,
        };
        assert_eq!(h.to_bytes(), [4, 3, 2, 1, 3, 9, 0x0b, 0x0a]);
        assert_eq!(FrameHeader::parse(&h.to_bytes()), Some(h));
        assert_eq!(FrameHeader::parse(&[0; 7]), None);
    }

    #[test]
    fn fragment_split() {
        let data = [0u8; 10];
        let f: Vec<_> = fragments(&data, 4).collect();
        assert_eq!(
            f.iter().map(|x| (x.0, x.1.len())).collect::<Vec<_>>(),
            vec![(1, 4), (1, 4), (0, 2)]
        );
        let f: Vec<_> = fragments(&data, 5).collect();
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].0, 0);
        assert_eq!(fragments(&[], 5).count(), 1);
    }

    #[test]
    fn roundtrip_fragmented_whole_buffer() {
        let w = FrameWriter::new(100);
        let m = video(1000);
        let mut out = vec![];
        w.write_message(&m, &mut out).unwrap();
        let mut d = FrameDecoder::new();
        d.push(&out);
        assert_eq!(pop_all(&mut d), vec![Incoming::Message(m)]);
        assert_eq!(d.buffered(), 0);
    }

    #[test]
    fn one_byte_at_a_time() {
        let w = FrameWriter::new(37);
        let msgs = vec![
            video(500),
            Message::RequestKeyframe,
            video(0),
            Message::Bye(Bye { reason: "x".into() }),
        ];
        let mut out = vec![];
        for m in &msgs {
            w.write_message(m, &mut out).unwrap();
        }
        let mut d = FrameDecoder::new();
        let mut got = vec![];
        for b in &out {
            d.push(std::slice::from_ref(b));
            while let Some(m) = d.pop().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(
            got,
            msgs.into_iter().map(Incoming::Message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn interleaved_frames_between_fragments() {
        let m = video(300);
        let (ch, kind, payload) = encode_message(&m).unwrap();
        let cur = Message::CursorState(CursorState {
            x: 1,
            y: -2,
            visible: true,
            shape_id: 3,
            ts_us: 4,
        });
        let (cch, ckind, cpayload) = encode_message(&cur).unwrap();
        let mut out = vec![];
        for (i, (flags, chunk)) in fragments(&payload, 100).enumerate() {
            put_frame(&mut out, ch as u8, kind, flags, chunk);
            if i == 0 {
                put_frame(&mut out, cch as u8, ckind, 0, &cpayload);
            }
        }
        let mut d = FrameDecoder::new();
        d.push(&out);
        assert_eq!(
            pop_all(&mut d),
            vec![Incoming::Message(cur), Incoming::Message(m)]
        );
    }

    #[test]
    fn unknown_kinds_are_skipped() {
        let mut out = vec![];
        put_frame(&mut out, 0, 200, 0, b"future");
        put_frame(&mut out, 99, 1, FLAG_MORE, b"fu");
        put_frame(&mut out, 99, 1, 0, b"ture");
        FrameWriter::default()
            .write_message(&Message::RequestKeyframe, &mut out)
            .unwrap();
        let mut d = FrameDecoder::new();
        d.push(&out);
        assert_eq!(
            pop_all(&mut d),
            vec![
                Incoming::Unknown {
                    channel: 0,
                    kind: 200,
                    len: 6
                },
                Incoming::Unknown {
                    channel: 99,
                    kind: 1,
                    len: 6
                },
                Incoming::Message(Message::RequestKeyframe),
            ]
        );
    }

    #[test]
    fn unknown_flag_bits_ignored() {
        let mut out = vec![];
        put_frame(&mut out, 0, 8, 0xfffe, &[]);
        let mut d = FrameDecoder::new();
        d.push(&out);
        assert_eq!(
            pop_all(&mut d),
            vec![Incoming::Message(Message::RequestKeyframe)]
        );
    }

    #[test]
    fn oversize_frame_rejected_from_header_alone() {
        let h = FrameHeader {
            payload_len: MAX_FRAME_LEN + 1,
            channel: 3,
            kind: 1,
            flags: 0,
        };
        let mut d = FrameDecoder::new();
        d.push(&h.to_bytes());
        assert!(matches!(d.pop(), Err(ProtoError::FrameTooLarge(_))));
    }

    #[test]
    fn oversize_message_rejected() {
        let chunk = vec![0u8; MAX_FRAME_LEN as usize];
        let mut d = FrameDecoder::new();
        let mut err = None;
        for _ in 0..3 {
            let mut out = vec![];
            put_frame(&mut out, 3, 1, FLAG_MORE, &chunk);
            d.push(&out);
            if let Err(e) = d.pop() {
                err = Some(e);
                break;
            }
        }
        assert!(matches!(err, Some(ProtoError::MessageTooLarge)));
        let w = FrameWriter::default();
        let mut out = vec![];
        let big = vec![0u8; MAX_MESSAGE_LEN + 1];
        assert!(matches!(
            w.write_raw(3, 1, &big, &mut out),
            Err(ProtoError::MessageTooLarge)
        ));
    }

    #[test]
    fn malformed_known_payload_is_error() {
        let mut out = vec![];
        put_frame(&mut out, 3, 1, 0, &[1, 2]);
        let mut d = FrameDecoder::new();
        d.push(&out);
        assert!(matches!(d.pop(), Err(ProtoError::Decode { .. })));
    }
}
