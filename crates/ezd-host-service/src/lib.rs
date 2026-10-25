//! The ezdisplay host service: listens for the tablet, runs one session at a time and exposes
//! its state, shared by the `ezd-host` command line and the tray app.
//!
//! * [`HostService::start`] binds the listener and spawns the accept loop and the adb watcher.
//! * [`HostService::status`] is a `watch` channel with the current [`HostStatus`]; one-off
//!   notifications (a session ended, ...) come from [`HostService::events`].
//! * With the `wifi` feature, [`ServiceConfig::wifi`] adds a QUIC listener for paired tablets
//!   next to the plaintext TCP one; only one session streams at a time across both.
//! * [`HostService::apply`] changes the [`ServiceConfig`] while running; [`HostService::stop`]
//!   ends the session cleanly and removes the adb reverses.
//!
//! The library never localizes text: errors are typed ([`ServiceError`]) and `Display` is
//! English for logs.

pub mod adb;
pub mod config;
pub mod error;
#[cfg(feature = "wifi")]
mod mdns;
pub mod platform;
mod service;
pub mod status;
#[cfg(feature = "wifi")]
pub mod store;
#[cfg(feature = "wifi")]
mod wifi;

pub use adb::{AdbClient, AdbLocator, is_network_serial};
pub use config::{AdbMode, HdrChoice, LinkChoice, PenChoice, ServiceConfig, SourceKind};
#[cfg(feature = "wifi")]
pub use config::{CongestionChoice, WifiConfig};
pub use error::ServiceError;
pub use service::{HostService, ServiceHooks, SourceFactory};
pub use status::{
    DeviceInfo, DeviceState, HostEvent, HostStatus, LiveSession, PairingCloseReason, RefusedWhy,
    Transport, WifiInfo,
};
