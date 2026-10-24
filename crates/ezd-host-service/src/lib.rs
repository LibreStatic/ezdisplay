//! The ezdisplay host service: listens for the tablet, runs one session at a time and exposes
//! its state, shared by the `ezd-host` command line and the tray app.
//!
//! * [`HostService::start`] binds the listener and spawns the accept loop and the adb watcher.
//! * [`HostService::status`] is a `watch` channel with the current [`HostStatus`]; one-off
//!   notifications (a session ended, ...) come from [`HostService::events`].
//! * [`HostService::apply`] changes the [`ServiceConfig`] while running; [`HostService::stop`]
//!   ends the session cleanly and removes the adb reverses.
//!
//! The library never localizes text: errors are typed ([`ServiceError`]) and `Display` is
//! English for logs.

pub mod adb;
pub mod config;
pub mod error;
pub mod platform;
mod service;
pub mod status;

pub use adb::{AdbClient, AdbLocator, is_network_serial};
pub use config::{AdbMode, LinkChoice, PenChoice, ServiceConfig, SourceKind};
pub use error::ServiceError;
pub use service::{HostService, ServiceHooks, SourceFactory};
pub use status::{DeviceInfo, DeviceState, HostEvent, HostStatus, LiveSession};
