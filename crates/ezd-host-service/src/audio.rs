//! The audio parts of a session on this host: platform capture and the optional Opus encoder.

use ezd_host_core::{AudioBackend, AudioEncoder, AudioSource, SourceError};

/// Platform capture plus Opus through FFmpeg when the `ffmpeg` feature is on.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostAudioBackend;

impl AudioBackend for HostAudioBackend {
    fn create_source(&self) -> Option<Box<dyn AudioSource>> {
        crate::platform::audio_source()
    }

    fn opus_available(&self) -> bool {
        opus_available()
    }

    fn create_opus_encoder(&self, channels: u8) -> Result<Box<dyn AudioEncoder>, SourceError> {
        #[cfg(feature = "ffmpeg")]
        {
            ezd_ffmpeg::create_opus_encoder(channels)
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            let _ = channels;
            Err(SourceError::msg("built without ffmpeg"))
        }
    }
}

/// True if this build can encode Opus.
pub fn opus_available() -> bool {
    #[cfg(feature = "ffmpeg")]
    {
        ezd_ffmpeg::opus_available()
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        false
    }
}

/// The audio part of the diagnostics report.
pub fn diag_section() -> serde_json::Value {
    serde_json::json!({
        "capture": crate::platform::audio_source().is_some(),
        "opus": opus_available(),
    })
}
