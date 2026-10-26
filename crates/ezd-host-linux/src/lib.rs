//! Linux (KDE Plasma, GNOME, sway and Hyprland on Wayland, any desktop on X11) host backend: a
//! virtual output from the compositor (KWin through the ScreenCast portal, Mutter through its own
//! ScreenCast service, sway and Hyprland through their IPC, an X server as a RandR monitor),
//! PipeWire capture (DMA-BUF preferred), direct Wayland capture on the wlroots compositors or
//! MIT-SHM capture on X11, and VAAPI / NVENC / x264 encoding.
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
mod hyprland;
mod input;
mod kde;
mod kscreen;
mod kwin_input;
mod pod;
mod portal;
mod provider;
mod pw;
mod source;
mod sway;
mod uinput_sink;
mod vkdma;
mod wl_capture;
#[cfg(test)]
mod wlroots_test;
mod x11_capture;
mod x11_input;
mod x11_output;
#[cfg(test)]
mod x11_test;

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
        LinuxBackend::Sway => Box::new(sway::SwayProvider),
        LinuxBackend::Hyprland => Box::new(hyprland::HyprlandProvider),
        LinuxBackend::X11 => Box::new(x11_output::X11Provider),
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
/// device interface, GNOME through gsettings, sway and Hyprland through their per-device output
/// mapping, X11 through XInput2 transformation matrices.
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
        LinuxBackend::Sway => Some(Arc::new(uinput_sink::UinputBackend::new(Arc::new(
            sway::SwayDesktop,
        )))),
        LinuxBackend::Hyprland => Some(Arc::new(uinput_sink::UinputBackend::new(Arc::new(
            hyprland::HyprlandDesktop,
        )))),
        LinuxBackend::X11 => Some(Arc::new(uinput_sink::UinputBackend::new(Arc::new(
            x11_input::X11Desktop,
        )))),
    }
}

/// Platform section of `ezd-host diag`.
pub fn diag_section() -> serde_json::Value {
    diag::section()
}
