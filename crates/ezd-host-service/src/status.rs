//! Observable state of the service.

use crate::error::ServiceError;
use ezd_host_core::{LiveStats, SessionSummary};
use ezd_proto::{DynamicRange, InputStatus, StreamConfig};
use std::net::SocketAddr;
use std::time::Instant;

/// What the service is doing, see [`HostService::status`](crate::HostService::status).
// One live session at a time, so the size of the variant does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum HostStatus {
    /// Not running (after `stop`).
    Stopped,
    /// Listening; no client is streaming. `devices` are the adb devices this service handles.
    Waiting {
        /// Devices seen by the adb watcher (empty without adb).
        devices: Vec<DeviceInfo>,
    },
    /// A client is connected and the stream is configured.
    Streaming(LiveSession),
    /// Something needs attention. While the listener is up (adb or source problems) a client can
    /// still connect, which switches the status to `Streaming`.
    Error(ServiceError),
}

/// The running session.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSession {
    /// Name the client announced.
    pub device_name: String,
    /// Negotiated stream configuration.
    pub stream: StreamConfig,
    /// Dynamic range of the stream (HDR10, converted SDR with the reason, or plain SDR).
    pub dynamic_range: DynamicRange,
    /// Devices seen by the adb watcher.
    pub devices: Vec<DeviceInfo>,
    /// Latest live measurements (once per second), `None` until the first window ends.
    pub stats: Option<LiveStats>,
    /// Latest input status reported to the client.
    pub input: Option<InputStatus>,
    /// How the client reached the host.
    pub transport: Transport,
}

/// How a client reached the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    /// The plaintext TCP listener (USB through `adb reverse`, or loopback).
    #[default]
    Tcp,
    /// Wi-Fi: QUIC with a paired device.
    Wifi,
}

/// The Wi-Fi listener, see [`HostService::wifi`](crate::HostService::wifi).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiInfo {
    /// UDP address being listened on.
    pub addr: SocketAddr,
    /// Host certificate fingerprint, lower-case hex.
    pub fingerprint: String,
    /// Host certificate fingerprint in the short form people compare.
    pub fingerprint_short: String,
    /// Name announced to clients and on the network.
    pub host_name: String,
    /// Whether a pairing window is open.
    pub pairing_open: bool,
}

/// An adb device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// adb serial.
    pub serial: String,
    /// Connection state.
    pub state: DeviceState,
    /// Product model, when known.
    pub model: Option<String>,
}

/// Connection state of an adb device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    /// Usable (`device`).
    Ready,
    /// USB debugging not accepted on the device yet.
    Unauthorized,
    /// `offline`.
    Offline,
    /// Any other adb state.
    Other(String),
}

/// One-off notifications, see [`HostService::events`](crate::HostService::events).
// Events are rare, so the size of the summary does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// The listener is bound (also after a rebind).
    Listening(SocketAddr),
    /// A client connected; the handshake starts.
    ClientConnected(SocketAddr),
    /// The video source could not be created for a client.
    SourceFailed(ServiceError),
    /// A session ended; `Err` carries the failure text.
    SessionEnded(Result<SessionSummary, String>),
    /// A pairing window opened (also when it is reopened with a new PIN).
    PairingOpen {
        /// The PIN to type on the tablet.
        pin: String,
        /// The `ezd://pair?...` URI for the QR code (it carries the PIN).
        uri: String,
        /// Short form of the host fingerprint, for people to compare.
        fingerprint: String,
        /// When the window closes by itself.
        expires: Instant,
    },
    /// The pairing window closed.
    PairingClosed {
        /// Why.
        reason: PairingCloseReason,
    },
    /// A device paired.
    Paired {
        /// Name the device announced.
        device: String,
    },
    /// A pairing attempt failed (wrong PIN, window closed, broken connection).
    PairingFailed {
        /// The client.
        peer: SocketAddr,
        /// English description, for logs.
        reason: String,
    },
    /// A Wi-Fi client was turned away.
    SessionRefused {
        /// The client.
        peer: SocketAddr,
        /// Why.
        why: RefusedWhy,
    },
}

/// Why a pairing window closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingCloseReason {
    /// Closed on request.
    Closed,
    /// A device paired.
    Paired,
    /// Too many wrong PINs.
    TooManyAttempts,
    /// The time ran out.
    Expired,
}

/// Why a Wi-Fi session request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusedWhy {
    /// The device is not paired.
    NotPaired,
    /// Another session is running.
    Busy,
}
