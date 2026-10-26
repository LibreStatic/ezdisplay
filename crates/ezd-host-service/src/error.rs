//! Errors of the host service. `Display` is English, for logs; frontends localize by variant.

use std::net::SocketAddr;

/// Why the service cannot do what it was asked to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// The listener could not be bound.
    #[error("cannot listen on {addr}: {reason}")]
    Bind {
        /// Address that was requested.
        addr: SocketAddr,
        /// The OS error.
        reason: String,
    },
    /// adb is needed (`AdbMode::Auto` or `Serial`) but no binary was found.
    #[error("adb not found")]
    AdbNotFound,
    /// `adb reverse` failed for a device.
    #[error("adb reverse failed for {serial}: {reason}")]
    AdbReverse {
        /// Device serial.
        serial: String,
        /// What adb said.
        reason: String,
    },
    /// There is no virtual display backend for this platform.
    #[error("no virtual display backend for this platform")]
    UnsupportedPlatform,
    /// The Wi-Fi transport cannot start (identity or paired-devices file unusable, ...).
    #[error("Wi-Fi: {0}")]
    Wifi(String),
    /// The video source could not be created.
    #[error("cannot create the video source: {0}")]
    Source(String),
}
