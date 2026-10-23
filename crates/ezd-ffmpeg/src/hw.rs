use crate::error::FfmpegError;
use ffmpeg_sys_next as ffi;
use std::ffi::CString;
use std::ptr;

/// Kind of FFmpeg hardware device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwKind {
    /// Linux VAAPI (`/dev/dri/renderD*`).
    Vaapi,
    /// NVIDIA CUDA.
    Cuda,
    /// Intel QSV.
    Qsv,
    /// Windows D3D11VA.
    D3d11va,
}

impl HwKind {
    pub(crate) fn device_type(self) -> ffi::AVHWDeviceType {
        match self {
            HwKind::Vaapi => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            HwKind::Cuda => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
            HwKind::Qsv => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_QSV,
            HwKind::D3d11va => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
        }
    }

    pub(crate) fn pix_fmt(self) -> ffi::AVPixelFormat {
        match self {
            HwKind::Vaapi => ffi::AVPixelFormat::AV_PIX_FMT_VAAPI,
            HwKind::Cuda => ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
            HwKind::Qsv => ffi::AVPixelFormat::AV_PIX_FMT_QSV,
            HwKind::D3d11va => ffi::AVPixelFormat::AV_PIX_FMT_D3D11,
        }
    }
}

/// A reference-counted FFmpeg hardware device context.
pub struct HwDevice {
    pub(crate) kind: HwKind,
    pub(crate) buf: *mut ffi::AVBufferRef,
}

// SAFETY: AVBufferRef is reference counted with atomic operations; the device
// context itself is only used from the thread owning the encoder.
unsafe impl Send for HwDevice {}

impl HwDevice {
    /// Creates a device of `kind`; `device` is e.g. `/dev/dri/renderD129` for VAAPI.
    pub fn open(kind: HwKind, device: Option<&str>) -> Result<HwDevice, FfmpegError> {
        crate::error::init_logging();
        let c = device
            .map(CString::new)
            .transpose()
            .map_err(|_| FfmpegError::msg("device path contains NUL"))?;
        let mut buf = ptr::null_mut();
        // SAFETY: `buf` is a valid out-pointer; the device string is NUL-terminated and
        // outlives the call; options/flags are null/0.
        let r = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut buf,
                kind.device_type(),
                c.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
                ptr::null_mut(),
                0,
            )
        };
        if r < 0 {
            return Err(FfmpegError::av(
                format!("create {kind:?} device {device:?}"),
                r,
            ));
        }
        Ok(HwDevice { kind, buf })
    }

    /// The kind of this device.
    pub fn kind(&self) -> HwKind {
        self.kind
    }

    /// Raw `AVBufferRef*` (still owned by `self`).
    pub fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.buf
    }
}

impl Clone for HwDevice {
    fn clone(&self) -> Self {
        // SAFETY: self.buf is a valid buffer ref; av_buffer_ref returns a new reference.
        let buf = unsafe { ffi::av_buffer_ref(self.buf) };
        HwDevice {
            kind: self.kind,
            buf,
        }
    }
}

impl Drop for HwDevice {
    fn drop(&mut self) {
        // SAFETY: we own exactly one reference, released here.
        unsafe { ffi::av_buffer_unref(&mut self.buf) };
    }
}

impl std::fmt::Debug for HwDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwDevice")
            .field("kind", &self.kind)
            .finish()
    }
}
