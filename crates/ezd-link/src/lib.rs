//! Transport layer for ezdisplay.
//!
//! * [`Link`]: a reliable ordered byte stream (TCP, in-memory duplex, later AOA).
//! * [`mux`]: [`MuxSender`] / [`MuxReceiver`], the prioritized message mux on top of a link.
//! * [`adb`] (not on Android): locate `adb`, list devices, manage `adb reverse`.

#[cfg(not(target_os = "android"))]
pub mod adb;
pub mod link;
pub mod mux;

pub use link::{BoxRead, BoxWrite, Link};
pub use mux::{
    MuxClosed, MuxError, MuxOptions, MuxReceiver, MuxSender, MuxStats, MuxTask, VideoCredit,
};

pub use ezd_proto::DEFAULT_PORT;
