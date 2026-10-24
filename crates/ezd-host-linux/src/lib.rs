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
mod input;
mod kscreen;
mod kwin_input;
mod pod;
mod portal;
mod pw;
mod source;
mod uinput_sink;
mod vkdma;

pub use ezd_host_core::PlatformOptions;
use ezd_host_core::{InputBackend, PlatformError, VideoSource};
use std::sync::Arc;

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

/// Input injection backend (`None` = not available on this desktop).
pub fn input_backend() -> Option<Arc<dyn InputBackend>> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !desktop.to_ascii_uppercase().split(':').any(|d| d == "KDE") {
        tracing::info!(%desktop, "input injection is only implemented for KDE Plasma");
        return None;
    }
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => Some(Arc::new(uinput_sink::KdeInputBackend::new(handle))),
        Err(_) => {
            tracing::warn!("input injection needs a tokio runtime; it is disabled");
            None
        }
    }
}

/// Platform section of `ezd-host diag`.
pub fn diag_section() -> serde_json::Value {
    diag::section()
}
