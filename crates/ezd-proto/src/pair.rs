//! Wi-Fi link preamble and PIN pairing messages.
//!
//! A Wi-Fi link is a QUIC connection with mutual TLS (self-signed certificates, pinned by
//! fingerprint after pairing). The client opens the control stream (bidirectional) and sends a
//! [`LinkOpen`]; the host answers with a [`LinkReply`]. For a session the host then opens the
//! video stream (unidirectional), starting with [`VIDEO_STREAM_TAG`], and both sides run the
//! normal framed protocol. Pairing is a SPAKE2 exchange keyed by a 6-digit PIN, see
//! `docs/protocol.md`.
//!
//! Preamble messages are postcard encoded with a little-endian `u16` length prefix
//! ([`encode_preamble`] / [`decode_preamble`]); they never exceed [`MAX_PREAMBLE_LEN`].

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// ALPN protocol id of a Wi-Fi link.
pub const ALPN: &[u8] = b"ezd/0";
/// First byte of the host's video stream.
pub const VIDEO_STREAM_TAG: u8 = b'V';
/// Largest preamble message payload.
pub const MAX_PREAMBLE_LEN: usize = 1024;
/// Number of digits in a pairing PIN.
pub const PIN_DIGITS: usize = 6;

/// SHA-256 of a peer's certificate (DER).
pub type Fingerprint = [u8; 32];

/// First message of the client on the control stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkOpen {
    /// Start a session; the client certificate must already be paired.
    Session,
    /// Pair this client using the host's current PIN.
    Pair {
        /// Human readable client name.
        device_name: String,
        /// SPAKE2 message of the client (side A).
        #[serde(with = "serde_bytes")]
        spake: Vec<u8>,
    },
}

/// Why the host refused a [`LinkOpen`] or a pairing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// The client certificate is not paired with this host.
    NotPaired,
    /// No pairing window is open on the host.
    PairingClosed,
    /// The pairing window closed after too many failed attempts.
    TooManyAttempts,
    /// The PIN did not match.
    WrongPin,
    /// Another client is streaming.
    Busy,
}

/// Host answer to a [`LinkOpen`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkReply {
    /// Session accepted; the video stream follows.
    Accepted,
    /// Refused; the host closes the connection.
    Refused(Refusal),
    /// Pairing answer of the host.
    PairChallenge {
        /// Human readable host name.
        host_name: String,
        /// SPAKE2 message of the host (side B).
        #[serde(with = "serde_bytes")]
        spake: Vec<u8>,
        /// Host key confirmation (HMAC over the transcript).
        confirm: [u8; 32],
    },
}

/// Client key confirmation, sent after checking the host's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairConfirm {
    /// Client key confirmation (HMAC over the transcript).
    pub confirm: [u8; 32],
}

/// Final host message of a pairing exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairOutcome {
    /// Both sides pinned each other's certificate.
    Paired,
    /// Refused; nothing was stored.
    Refused(Refusal),
}

/// Encodes a preamble message with its length prefix.
pub fn encode_preamble<T: Serialize>(msg: &T) -> crate::Result<Vec<u8>> {
    let body = postcard::to_allocvec(msg).map_err(crate::ProtoError::Encode)?;
    if body.len() > MAX_PREAMBLE_LEN {
        return Err(crate::ProtoError::MessageTooLarge);
    }
    let mut out = Vec::with_capacity(2 + body.len());
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decodes a preamble payload (without its length prefix).
pub fn decode_preamble<T: DeserializeOwned>(body: &[u8]) -> crate::Result<T> {
    postcard::from_bytes(body).map_err(crate::ProtoError::Preamble)
}

/// True for a well-formed PIN: exactly [`PIN_DIGITS`] ASCII digits.
pub fn is_valid_pin(pin: &str) -> bool {
    pin.len() == PIN_DIGITS && pin.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preamble_round_trip() {
        let msgs = [
            LinkReply::Accepted,
            LinkReply::Refused(Refusal::Busy),
            LinkReply::PairChallenge {
                host_name: "pc".into(),
                spake: vec![7; 33],
                confirm: [9; 32],
            },
        ];
        for m in msgs {
            let enc = encode_preamble(&m).unwrap();
            let len = u16::from_le_bytes([enc[0], enc[1]]) as usize;
            assert_eq!(len, enc.len() - 2);
            assert_eq!(decode_preamble::<LinkReply>(&enc[2..]).unwrap(), m);
        }
    }

    #[test]
    fn oversized_preamble_is_rejected() {
        let m = LinkOpen::Pair {
            device_name: "x".repeat(MAX_PREAMBLE_LEN),
            spake: vec![],
        };
        assert!(encode_preamble(&m).is_err());
    }

    #[test]
    fn pin_format() {
        assert!(is_valid_pin("012345"));
        assert!(!is_valid_pin("12345"));
        assert!(!is_valid_pin("1234567"));
        assert!(!is_valid_pin("12a456"));
    }
}
