//! Shared contract of the platform crates (`ezd-host-linux`, `ezd-host-windows`).
//!
//! Each platform crate exposes, behind its own `cfg`:
//!
//! ```ignore
//! pub fn create_virtual_source(opts: &PlatformOptions)
//!     -> Result<Box<dyn VideoSource>, PlatformError>;
//! pub fn create_audio_source(opts: &PlatformOptions) -> Option<Box<dyn AudioSource>>;
//! pub fn diag_section() -> serde_json::Value;
//! ```
//! so `ezd-host` can call them without knowing the platform. `create_audio_source` returns
//! `None` when the PC audio cannot be captured; the host wraps it in an
//! [`AudioBackend`](crate::AudioBackend) for the sessions.

use std::time::Duration;

/// Options chosen on the command line that platform backends interpret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformOptions {
    /// Force a GPU/adapter by name (render node path, adapter description or LUID).
    pub adapter: Option<String>,
    /// Prefer the discrete GPU when several adapters are available.
    pub prefer_dgpu: bool,
    /// Name of an existing output to capture instead of creating a virtual one.
    pub output_name: Option<String>,
    /// How long a virtual display that outlives its session (Windows) stays on the desktop
    /// after the client goes away, so a quick reconnect does not move its windows.
    pub detach_grace: Duration,
}

/// Default [`PlatformOptions::detach_grace`].
pub const DEFAULT_DETACH_GRACE: Duration = Duration::from_secs(2);

impl Default for PlatformOptions {
    fn default() -> Self {
        PlatformOptions {
            adapter: None,
            prefer_dgpu: false,
            output_name: None,
            detach_grace: DEFAULT_DETACH_GRACE,
        }
    }
}

/// Error from a platform backend.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The backend is a skeleton that has not been written yet.
    #[error("virtual display source is not implemented on {0} yet")]
    Unimplemented(&'static str),
    /// Any other failure.
    #[error("{0}")]
    Other(String),
}
