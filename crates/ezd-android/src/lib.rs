//! ezdisplay Android client native library.
//!
//! The portable parts ([`caps`], [`tracker`], [`status`]) build and test on the host. The JNI
//! bridge, session and MediaCodec decoder only exist for `target_os = "android"`.
//!
//! Kotlin class: `io.github.facum.ezdisplay.NativeBridge` (see `bridge.rs`).

// FFI with the JVM and the NDK needs unsafe; the workspace denies it by default.
#![allow(unsafe_code)]

pub mod caps;
pub mod status;
pub mod tracker;

#[cfg(target_os = "android")]
mod bridge;
#[cfg(target_os = "android")]
mod decoder;
#[cfg(target_os = "android")]
mod session;
