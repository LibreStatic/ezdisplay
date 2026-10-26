//! The compositor-specific half of a Linux capture source: creating (and later destroying) the
//! virtual output, and saying where its frames come from.
//!
//! [`crate::source::LinuxSource`] owns the shared half (encoder selection, capture negotiation,
//! encode pipeline) and talks to one [`VirtualOutputProvider`]: [`crate::kde::KdeProvider`] or
//! [`crate::gnome::GnomeProvider`]. A provider returns a [`FrameSource`]; PipeWire is the only
//! one today, and a producer that does not use PipeWire (the wlroots backends capture through
//! Wayland protocols) becomes another variant there.

use crate::portal::SourceKind;
use crate::pw::Target;
use ezd_host_core::{OutputInfo, SourceError};
use std::os::fd::OwnedFd;

/// What the source asks a provider for.
#[derive(Debug, Clone)]
pub struct OutputRequest {
    /// A new virtual output, or an existing monitor.
    pub kind: SourceKind,
    /// The monitor to capture, for [`SourceKind::Monitor`].
    pub output_name: Option<String>,
    /// Negotiated size and refresh (millihertz) of the stream.
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

/// Where the frames of the opened output come from.
pub enum FrameSource {
    /// A PipeWire node, on the remote behind `fd` or, without one, on the user's own daemon.
    PipeWire { fd: Option<OwnedFd>, target: Target },
}

/// An opened output.
pub struct OpenedOutput {
    /// How to get its frames.
    pub frames: FrameSource,
    /// The output as the compositor reports it after the mode was applied, when known.
    pub info: Option<OutputInfo>,
    /// Keeps the output alive: dropping it destroys the virtual output. Dropped after capture.
    pub guard: Box<dyn Send>,
}

/// Creates virtual outputs on one compositor.
pub trait VirtualOutputProvider: Send {
    /// Short name for logs.
    fn name(&self) -> &'static str;

    /// Creates the output (or selects the monitor), applies the mode on a best-effort basis and
    /// returns how to capture it. May block; called from the session's video thread, never from
    /// a tokio worker.
    fn open(&mut self, req: &OutputRequest) -> Result<OpenedOutput, SourceError>;

    /// Called once the stream's format is known, with its size. A compositor that sizes the
    /// output from the negotiation (Mutter) reports the final output here; the default keeps what
    /// [`open`](Self::open) returned.
    fn settle(&mut self, _negotiated: (u32, u32)) -> Option<OutputInfo> {
        None
    }
}
