//! Linux (KDE Plasma / Wayland) host backend: a KWin virtual output via the ScreenCast portal,
//! PipeWire capture (DMA-BUF preferred) and VAAPI / NVENC / x264 encoding.
//!
//! Exposes the platform contract used by `ezd-host`:
//! `create_virtual_source(&PlatformOptions)` and `diag_section()`.

#![cfg(target_os = "linux")]

mod adapter;
mod ahead;
mod cursor;
mod cursor_meta;
mod diag;
mod encode;
mod frame;
#[cfg(test)]
mod gbm_test;
mod kscreen;
mod pod;
mod portal;
mod pw;
mod source;
mod vkdma;

pub use ezd_host_core::PlatformOptions;
use ezd_host_core::{PlatformError, VideoSource};

/// Creates the capture + encode source for one client connection.
pub fn create_virtual_source(
    opts: &PlatformOptions,
) -> Result<Box<dyn VideoSource>, PlatformError> {
    let src = source::KdeVirtualSource::new(opts);
    if src.encoders().is_empty() {
        return Err(PlatformError::Other(
            "no working H.264 encoder (VAAPI, NVENC or libx264) was found".into(),
        ));
    }
    Ok(Box::new(src))
}

/// Platform section of `ezd-host diag`.
pub fn diag_section() -> serde_json::Value {
    diag::section()
}
