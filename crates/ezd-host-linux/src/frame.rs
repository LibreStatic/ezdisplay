//! Captured frame types handed from the PipeWire thread to the encoder, and pixel format tables.

use crate::cursor::PixelOrder;
use std::os::fd::OwnedFd;

/// `SPA_VIDEO_FORMAT_*` ids for the packed 32-bit formats we negotiate.
pub mod spa_fmt {
    /// R G B x
    pub const RGBX: u32 = 7;
    /// B G R x
    pub const BGRX: u32 = 8;
    /// x R G B
    pub const XRGB: u32 = 9;
    /// x B G R
    pub const XBGR: u32 = 10;
    /// R G B A
    pub const RGBA: u32 = 11;
    /// B G R A
    pub const BGRA: u32 = 12;
    /// A R G B
    pub const ARGB: u32 = 13;
    /// A B G R
    pub const ABGR: u32 = 14;
}

/// Formats offered to the compositor, in order of preference.
pub const OFFERED_FORMATS: [u32; 4] = [spa_fmt::BGRX, spa_fmt::BGRA, spa_fmt::RGBX, spa_fmt::RGBA];

/// `DRM_FORMAT_MOD_LINEAR`.
pub const DRM_MOD_LINEAR: u64 = 0;

const fn fourcc(s: &[u8; 4]) -> u32 {
    (s[0] as u32) | ((s[1] as u32) << 8) | ((s[2] as u32) << 16) | ((s[3] as u32) << 24)
}

/// DRM fourcc for a SPA video format (memory byte order -> little-endian word).
pub fn drm_fourcc(spa: u32) -> Option<u32> {
    Some(match spa {
        spa_fmt::BGRX => fourcc(b"XR24"),
        spa_fmt::BGRA => fourcc(b"AR24"),
        spa_fmt::RGBX => fourcc(b"XB24"),
        spa_fmt::RGBA => fourcc(b"AB24"),
        spa_fmt::XRGB => fourcc(b"BX24"),
        spa_fmt::XBGR => fourcc(b"RX24"),
        spa_fmt::ARGB => fourcc(b"BA24"),
        spa_fmt::ABGR => fourcc(b"RA24"),
        _ => return None,
    })
}

/// Byte order of a SPA video format (for cursor bitmaps and software frames).
pub fn pixel_order(spa: u32) -> Option<PixelOrder> {
    Some(match spa {
        spa_fmt::BGRX => PixelOrder::Bgrx,
        spa_fmt::BGRA => PixelOrder::Bgra,
        spa_fmt::RGBX => PixelOrder::Rgbx,
        spa_fmt::RGBA => PixelOrder::Rgba,
        spa_fmt::ARGB => PixelOrder::Argb,
        spa_fmt::ABGR => PixelOrder::Abgr,
        _ => return None,
    })
}

/// FFmpeg pixel format name (for filter graph arguments) of a SPA video format.
pub fn av_pix_fmt_name(spa: u32) -> Option<&'static str> {
    Some(match spa {
        spa_fmt::BGRX => "bgr0",
        spa_fmt::BGRA => "bgra",
        spa_fmt::RGBX => "rgb0",
        spa_fmt::RGBA => "rgba",
        spa_fmt::XRGB => "0rgb",
        spa_fmt::XBGR => "0bgr",
        spa_fmt::ARGB => "argb",
        spa_fmt::ABGR => "abgr",
        _ => return None,
    })
}

/// One plane of a DMA-BUF.
#[derive(Debug)]
pub struct Plane {
    /// Own duplicate of the buffer fd (closed when the frame is dropped or consumed).
    pub fd: OwnedFd,
    /// Total size of the dma-buf object.
    pub size: u64,
    /// Offset of the plane in the buffer.
    pub offset: u32,
    /// Row pitch in bytes.
    pub pitch: u32,
}

/// Held PipeWire buffer: dropping it gives the buffer back to the stream.
pub struct Release(pub Option<Box<dyn FnOnce() + Send>>);

impl Drop for Release {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

impl std::fmt::Debug for Release {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Release")
    }
}

/// A DMA-BUF frame.
#[derive(Debug)]
pub struct DmaBuf {
    /// SPA video format.
    pub spa_format: u32,
    /// DRM format modifier.
    pub modifier: u64,
    /// Planes (usually one).
    pub planes: Vec<Plane>,
    /// Releases the PipeWire buffer when this frame is dropped.
    pub release: Release,
}

/// A shared-memory frame, copied out of the PipeWire buffer.
#[derive(Debug)]
pub struct ShmBuf {
    /// SPA video format.
    pub spa_format: u32,
    /// Row stride in bytes.
    pub stride: usize,
    /// Pixels (`stride * (height - 1) + width * 4` bytes at least).
    pub data: Vec<u8>,
}

/// Pixel storage.
#[derive(Debug)]
pub enum Pixels {
    /// DMA-BUF import.
    Dma(DmaBuf),
    /// Copied bytes.
    Shm(ShmBuf),
}

/// One captured frame.
#[derive(Debug)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// [`ezd_host_core::now_us`] clock time of the content (PipeWire buffer time when known).
    pub capture_us: u64,
    /// Capture counter (skips when frames were coalesced).
    pub seq: u64,
    /// Pixels.
    pub pixels: Pixels,
}

impl Frame {
    /// True for DMA-BUF frames.
    pub fn is_dma(&self) -> bool {
        matches!(self.pixels, Pixels::Dma(_))
    }
}

/// Negotiated stream format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedFormat {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// SPA video format id.
    pub spa_format: u32,
    /// DRM modifier when the buffers are DMA-BUFs.
    pub modifier: Option<u64>,
}

/// Converts a PipeWire header timestamp (CLOCK_MONOTONIC ns) to the `now_us` clock.
///
/// `mono_now_ns` is CLOCK_MONOTONIC at the time of the call and `now_us` the host clock at the
/// same instant. Implausible stamps (zero, in the future, older than one second) fall back to
/// `now_us`.
pub fn capture_us_from_pts(pts_ns: i64, mono_now_ns: i64, now_us: u64) -> u64 {
    if pts_ns <= 0 {
        return now_us;
    }
    let age_ns = mono_now_ns - pts_ns;
    if !(0..1_000_000_000).contains(&age_ns) {
        return now_us;
    }
    now_us.saturating_sub((age_ns / 1000) as u64)
}

/// Picks the modifier to fixate: linear if offered (always importable), otherwise the first
/// offered modifier we support.
pub fn choose_modifier(offered: &[u64], supported: &[u64]) -> Option<u64> {
    if offered.contains(&DRM_MOD_LINEAR) && supported.contains(&DRM_MOD_LINEAR) {
        return Some(DRM_MOD_LINEAR);
    }
    offered.iter().copied().find(|m| supported.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipewire::spa::sys as s;

    #[test]
    fn spa_ids_match_bindings() {
        assert_eq!(spa_fmt::RGBX, s::SPA_VIDEO_FORMAT_RGBx);
        assert_eq!(spa_fmt::BGRX, s::SPA_VIDEO_FORMAT_BGRx);
        assert_eq!(spa_fmt::XRGB, s::SPA_VIDEO_FORMAT_xRGB);
        assert_eq!(spa_fmt::XBGR, s::SPA_VIDEO_FORMAT_xBGR);
        assert_eq!(spa_fmt::RGBA, s::SPA_VIDEO_FORMAT_RGBA);
        assert_eq!(spa_fmt::BGRA, s::SPA_VIDEO_FORMAT_BGRA);
        assert_eq!(spa_fmt::ARGB, s::SPA_VIDEO_FORMAT_ARGB);
        assert_eq!(spa_fmt::ABGR, s::SPA_VIDEO_FORMAT_ABGR);
    }

    #[test]
    fn fourccs() {
        // DRM_FORMAT_XRGB8888 / ARGB8888 / XBGR8888 / ABGR8888
        assert_eq!(drm_fourcc(spa_fmt::BGRX), Some(0x3432_5258));
        assert_eq!(drm_fourcc(spa_fmt::BGRA), Some(0x3432_5241));
        assert_eq!(drm_fourcc(spa_fmt::RGBX), Some(0x3432_4258));
        assert_eq!(drm_fourcc(spa_fmt::RGBA), Some(0x3432_4241));
        assert_eq!(drm_fourcc(99), None);
    }

    #[test]
    fn all_offered_formats_are_mapped() {
        for f in OFFERED_FORMATS {
            assert!(drm_fourcc(f).is_some() && pixel_order(f).is_some());
            assert!(av_pix_fmt_name(f).is_some());
        }
    }

    #[test]
    fn capture_time_conversion() {
        let now = 10_000_000;
        assert_eq!(
            capture_us_from_pts(1_000_000_000, 1_004_000_000, now),
            now - 4_000
        );
        assert_eq!(capture_us_from_pts(0, 5, now), now);
        // In the future or too old: fall back.
        assert_eq!(capture_us_from_pts(2_000_000_000, 1_000_000_000, now), now);
        assert_eq!(capture_us_from_pts(1_000, 5_000_000_000, now), now);
    }

    #[test]
    fn modifier_choice() {
        let sup = [0, 0x0100_0000_0000_0002];
        assert_eq!(
            choose_modifier(&[5, 0, 0x0100_0000_0000_0002], &sup),
            Some(0)
        );
        assert_eq!(
            choose_modifier(&[5, 0x0100_0000_0000_0002], &sup),
            Some(0x0100_0000_0000_0002)
        );
        assert_eq!(choose_modifier(&[5, 6], &sup), None);
        assert_eq!(choose_modifier(&[], &sup), None);
    }

    #[test]
    fn release_runs_once_on_drop() {
        let n = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let n2 = n.clone();
        let r = Release(Some(Box::new(move || {
            n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })));
        drop(r);
        assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
