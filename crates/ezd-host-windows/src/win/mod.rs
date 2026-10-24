//! Windows backend: GPU policy, VDD control, Desktop Duplication, encode, diagnostics.

// Without FFmpeg only `diag` uses the capture side; the streaming pipeline that drives the
// rest is compiled out.
#![cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]

pub mod capture;
pub mod diag;
pub mod display;
pub mod dxgi;
#[cfg(feature = "ffmpeg")]
pub mod encode;
#[cfg(feature = "ffmpeg")]
pub mod ffenc;
pub mod input;
pub mod source;
pub mod util;
pub mod vdd;
pub mod vdd_install;

pub use source::create_virtual_source;
