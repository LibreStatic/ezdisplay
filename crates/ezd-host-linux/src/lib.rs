//! Linux (KDE Plasma and GNOME, Wayland) host backend: a virtual output from the compositor (KWin
//! through the ScreenCast portal, Mutter through its own ScreenCast service), PipeWire capture
//! (DMA-BUF preferred) and VAAPI / NVENC / x264 encoding.
//!
//! Exposes the platform contract used by `ezd-host`:
//! `create_virtual_source(&PlatformOptions)` and `diag_section()`.

#![cfg(target_os = "linux")]

mod adapter;
mod ahead;
mod backend;
mod cursor;
mod cursor_meta;
mod diag;
mod encode;
mod frame;
#[cfg(test)]
mod gbm_test;
mod gnome;
mod gnome_dc;
mod gnome_input;
mod input;
mod kde;
mod kscreen;
mod kwin_input;
mod pod;
mod portal;
mod provider;
mod pw;
mod source;
mod uinput_sink;
mod vkdma;

use backend::LinuxBackend;
pub use ezd_host_core::PlatformOptions;
use ezd_host_core::{InputBackend, PlatformError, UnavailableBackend, VideoSource};
use std::sync::Arc;

/// Creates the capture + encode source for one client connection, for the detected desktop.
pub fn create_virtual_source(
    opts: &PlatformOptions,
) -> Result<Box<dyn VideoSource>, PlatformError> {
    let backend = backend::current().map_err(PlatformError::Other)?;
    let provider: Box<dyn provider::VirtualOutputProvider> = match backend {
        LinuxBackend::Kde => Box::new(kde::KdeProvider),
        LinuxBackend::Gnome => Box::new(gnome::GnomeProvider::new()),
        LinuxBackend::Sway | LinuxBackend::Hyprland => {
            return Err(PlatformError::Other(backend.unsupported_error()));
        }
    };
    tracing::info!(backend = backend.name(), "Linux backend");
    let src = source::LinuxSource::new(opts, provider);
    if src.encoders().is_empty() {
        return Err(PlatformError::Other(
            "no working H.264 encoder (VAAPI, NVENC or libx264) was found".into(),
        ));
    }
    Ok(Box::new(src))
}

/// Input injection backend for the detected desktop (see [`backend`]): KDE through KWin's input
/// device interface, GNOME through gsettings. Elsewhere the backend is unavailable with a note
/// for the tablet.
pub fn input_backend() -> Option<Arc<dyn InputBackend>> {
    let backend = match backend::current() {
        Ok(b) => b,
        Err(e) => return Some(Arc::new(UnavailableBackend(e))),
    };
    match backend {
        LinuxBackend::Kde => match tokio::runtime::Handle::try_current() {
            Ok(handle) => Some(Arc::new(uinput_sink::UinputBackend::kde(handle))),
            Err(_) => {
                tracing::warn!("input injection needs a tokio runtime; it is disabled");
                None
            }
        },
        LinuxBackend::Gnome => Some(Arc::new(uinput_sink::UinputBackend::new(Arc::new(
            gnome_input::GnomeDesktop::new(),
        )))),
        LinuxBackend::Sway | LinuxBackend::Hyprland => {
            tracing::info!(
                backend = backend.name(),
                "input injection is not implemented for this desktop"
            );
            Some(Arc::new(UnavailableBackend(format!(
                "input injection: {}",
                backend.unsupported_error()
            ))))
        }
    }
}

/// Platform section of `ezd-host diag`.
pub fn diag_section() -> serde_json::Value {
    diag::section()
}
