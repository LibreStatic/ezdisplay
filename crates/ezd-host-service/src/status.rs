//! Observable state of the service.

use crate::error::ServiceError;
use ezd_host_core::{LiveStats, SessionSummary};
use ezd_proto::{DynamicRange, InputStatus, StreamConfig};
use std::net::SocketAddr;

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
}
