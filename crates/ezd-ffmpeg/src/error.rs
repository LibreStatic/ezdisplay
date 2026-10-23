use ffmpeg_sys_next as ffi;
use std::ffi::CStr;

/// Errors from the FFmpeg wrapper.
#[derive(Debug, thiserror::Error)]
pub enum FfmpegError {
    /// An FFmpeg call returned an error code.
    #[error("{what}: {text} (averror {code})")]
    Av {
        /// The failing operation.
        what: String,
        /// `av_strerror` text.
        text: String,
        /// Raw error code.
        code: i32,
    },
    /// Anything else (missing encoder, bad parameters, ...).
    #[error("{0}")]
    Msg(String),
}

impl FfmpegError {
    pub(crate) fn msg(s: impl Into<String>) -> Self {
        FfmpegError::Msg(s.into())
    }

    pub(crate) fn av(what: impl Into<String>, code: i32) -> Self {
        let mut buf = [0 as std::os::raw::c_char; 128];
        // SAFETY: `buf` is a valid writable buffer of the stated size; av_strerror NUL-terminates it.
        let text = unsafe {
            ffi::av_strerror(code, buf.as_mut_ptr(), buf.len());
            CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
        };
        FfmpegError::Av {
            what: what.into(),
            text,
            code,
        }
    }
}

/// `AVERROR(EAGAIN)`.
pub(crate) const EAGAIN: i32 = -11;
/// `AVERROR_EOF`.
pub(crate) fn averror_eof() -> i32 {
    // FFERRTAG('E','O','F',' ') negated.
    -(i32::from_le_bytes(*b"EOF "))
}

pub(crate) fn init_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: plain setter of a global log level, no pointers involved.
        unsafe { ffi::av_log_set_level(ffi::AV_LOG_ERROR as i32) };
    });
}
