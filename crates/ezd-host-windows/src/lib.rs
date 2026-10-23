//! windows host backend (skeleton). Everything is behind `cfg(windows)`, so this
//! crate is empty on other platforms.
//!
//! Contract (see `ezd_host_core::platform`): `create_virtual_source` builds
//! the capture + encode pipeline for a virtual display and `diag_section`
//! feeds the `platform` part of `ezd-host diag`.
#![cfg(windows)]

pub use ezd_host_core::PlatformOptions;
use ezd_host_core::{PlatformError, VideoSource};

/// Creates the virtual display + capture + encode pipeline.
pub fn create_virtual_source(
    _opts: &PlatformOptions,
) -> Result<Box<dyn VideoSource>, PlatformError> {
    Err(PlatformError::Unimplemented("windows"))
}

/// Platform-specific diagnostics for `ezd-host diag`.
pub fn diag_section() -> serde_json::Value {
    serde_json::json!({ "status": "not implemented" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_reports_unimplemented() {
        assert!(matches!(
            create_virtual_source(&PlatformOptions::default()),
            Err(PlatformError::Unimplemented(_))
        ));
        assert_eq!(diag_section()["status"], "not implemented");
    }
}
