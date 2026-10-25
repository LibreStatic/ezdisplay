//! Transport layer for ezdisplay.
//!
//! * [`Link`]: a reliable ordered byte stream (TCP, in-memory duplex, blocking halves such as USB).
//! * [`mux`]: [`MuxSender`] / [`MuxReceiver`], the prioritized message mux on top of a link.
//! * [`adb`] (not on Android): locate `adb`, list devices, manage `adb reverse`.
//! * [`quic`], [`pair`], [`identity`] (`quic` feature): Wi-Fi links over QUIC with PIN pairing.

#[cfg(not(target_os = "android"))]
pub mod adb;
pub mod blocking;
#[cfg(feature = "quic")]
pub mod identity;
pub mod link;
pub mod mux;
#[cfg(feature = "quic")]
pub mod pair;
#[cfg(feature = "quic")]
pub mod quic;

pub use blocking::BlockingOptions;
pub use link::{BoxRead, BoxWrite, Lanes, Link};
pub use mux::{
    MuxClosed, MuxError, MuxOptions, MuxReceiver, MuxSender, MuxStats, MuxTask, VideoCredit,
};

pub use ezd_proto::DEFAULT_PORT;
