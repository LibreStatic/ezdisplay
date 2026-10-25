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

/// The `va_list` parameter type of FFmpeg's log callback as bindgen emits it: a pointer to the
/// tag struct on the x86_64 System V ABI, the `va_list` alias everywhere else (MSVC, aarch64).
#[cfg(all(target_arch = "x86_64", not(windows)))]
type VaList = *mut ffi::__va_list_tag;
#[cfg(not(all(target_arch = "x86_64", not(windows))))]
type VaList = ffi::va_list;

/// Forwards FFmpeg's log lines to `tracing`: stderr is lost in the windowed tray app, and hardware
/// encoders report why they fail to open only through `av_log` (often as warnings).
unsafe extern "C" fn log_callback(
    avcl: *mut std::os::raw::c_void,
    level: std::os::raw::c_int,
    fmt: *const std::os::raw::c_char,
    vl: VaList,
) {
    // SAFETY: plain getter of the global log level.
    if level > unsafe { ffi::av_log_get_level() } {
        return;
    }
    // Called from FFmpeg threads: format into the stack, no allocation unless a line is emitted.
    let mut buf = [0 as std::os::raw::c_char; 1024];
    let mut prefix: std::os::raw::c_int = 1;
    // SAFETY: arguments come straight from av_log; `buf` is writable for its full length and
    // av_log_format_line2 NUL-terminates it (truncating long lines).
    unsafe {
        ffi::av_log_format_line2(
            avcl,
            level,
            fmt,
            vl,
            buf.as_mut_ptr(),
            buf.len() as std::os::raw::c_int,
            &mut prefix,
        );
    }
    // SAFETY: NUL-terminated above.
    let line = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    let line = line.trim_end();
    if line.is_empty() {
        return;
    }
    if level <= ffi::AV_LOG_ERROR as i32 {
        tracing::error!(target: "ffmpeg", "{line}");
    } else if level <= ffi::AV_LOG_WARNING as i32 {
        tracing::warn!(target: "ffmpeg", "{line}");
    } else {
        tracing::debug!(target: "ffmpeg", "{line}");
    }
}

pub(crate) fn init_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: global setters; the callback is a 'static function that is safe to call from any
        // thread.
        unsafe {
            ffi::av_log_set_level(ffi::AV_LOG_VERBOSE as i32);
            ffi::av_log_set_callback(Some(log_callback));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_callback_formats_varargs_and_long_lines() {
        init_logging();
        let long = std::ffi::CString::new("x".repeat(4096)).unwrap();
        // SAFETY: format strings match their arguments; a null context is allowed by av_log.
        unsafe {
            ffi::av_log(
                ptr_null(),
                ffi::AV_LOG_WARNING as i32,
                c"value %d of %s\n".as_ptr(),
                42,
                c"test".as_ptr(),
            );
            ffi::av_log(
                ptr_null(),
                ffi::AV_LOG_ERROR as i32,
                c"%s\n".as_ptr(),
                long.as_ptr(),
            );
            ffi::av_log(
                ptr_null(),
                ffi::AV_LOG_TRACE as i32,
                c"filtered %d\n".as_ptr(),
                1,
            );
        }
    }

    fn ptr_null() -> *mut std::os::raw::c_void {
        std::ptr::null_mut()
    }
}
