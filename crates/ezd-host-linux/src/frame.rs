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

/// `SPA_VIDEO_FORMAT_*` ids of the packed 10-bit formats (2 bits of padding or alpha, 10 bits per
/// colour channel, little-endian 32-bit word), see `spa/param/video/raw.h`.
pub mod spa_fmt10 {
    /// x R G B, 2:10:10:10
    pub const XRGB_210LE: u32 = 80;
    /// x B G R, 2:10:10:10
    pub const XBGR_210LE: u32 = 81;
    /// A R G B, 2:10:10:10
    pub const ARGB_210LE: u32 = 84;
    /// A B G R, 2:10:10:10
    pub const ABGR_210LE: u32 = 85;
}

/// Formats offered to the compositor, in order of preference.
pub const OFFERED_FORMATS: [u32; 4] = [spa_fmt::BGRX, spa_fmt::BGRA, spa_fmt::RGBX, spa_fmt::RGBA];

/// 10-bit formats offered after [`OFFERED_FORMATS`], so a compositor that only knows the 8-bit
/// ones negotiates exactly what it always did. They carry HDR10 once a compositor renders it.
pub const OFFERED_FORMATS_10: [u32; 4] = [
    spa_fmt10::XRGB_210LE,
    spa_fmt10::XBGR_210LE,
    spa_fmt10::ARGB_210LE,
    spa_fmt10::ABGR_210LE,
];

/// True for the packed 10-bit formats.
pub fn is_ten_bit(spa: u32) -> bool {
    OFFERED_FORMATS_10.contains(&spa)
}

/// `SPA_VIDEO_COLOR_PRIMARIES_BT2020`.
pub const SPA_PRIMARIES_BT2020: u32 = 7;
/// `SPA_VIDEO_TRANSFER_SMPTE2084` (PQ).
pub const SPA_TRANSFER_PQ: u32 = 14;

/// Colour description of the negotiated format (`SPA_FORMAT_VIDEO_colorPrimaries` and friends),
/// `None` where the producer did not state it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpaColor {
    /// `SPA_VIDEO_COLOR_PRIMARIES_*`.
    pub primaries: Option<u32>,
    /// `SPA_VIDEO_TRANSFER_*`.
    pub transfer: Option<u32>,
    /// `SPA_VIDEO_COLOR_MATRIX_*`.
    pub matrix: Option<u32>,
}

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
        // DRM_FORMAT_XRGB2101010, XBGR2101010, ARGB2101010, ABGR2101010
        spa_fmt10::XRGB_210LE => fourcc(b"XR30"),
        spa_fmt10::XBGR_210LE => fourcc(b"XB30"),
        spa_fmt10::ARGB_210LE => fourcc(b"AR30"),
        spa_fmt10::ABGR_210LE => fourcc(b"AB30"),
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
        // FFmpeg has no 10-bit format with alpha: the 2 alpha bits of the A variants sit where the
        // padding of the x variants is, and are ignored.
        spa_fmt10::XRGB_210LE | spa_fmt10::ARGB_210LE => "x2rgb10le",
        spa_fmt10::XBGR_210LE | spa_fmt10::ABGR_210LE => "x2bgr10le",
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
    /// Colour description, when the producer stated it.
    pub color: SpaColor,
}

impl NegotiatedFormat {
    /// The stream is HDR10: a 10-bit format with BT.2020 primaries and the PQ transfer function.
    pub fn is_hdr10(&self) -> bool {
        is_ten_bit(self.spa_format)
            && self.color.primaries == Some(SPA_PRIMARIES_BT2020)
            && self.color.transfer == Some(SPA_TRANSFER_PQ)
    }
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

/// Picks the modifier to fixate: the first of `supported` (most preferred first) that was offered.
pub fn choose_modifier(offered: &[u64], supported: &[u64]) -> Option<u64> {
    supported.iter().copied().find(|m| offered.contains(m))
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
        assert_eq!(spa_fmt10::XRGB_210LE, s::SPA_VIDEO_FORMAT_xRGB_210LE);
        assert_eq!(spa_fmt10::XBGR_210LE, s::SPA_VIDEO_FORMAT_xBGR_210LE);
        assert_eq!(spa_fmt10::ARGB_210LE, s::SPA_VIDEO_FORMAT_ARGB_210LE);
        assert_eq!(spa_fmt10::ABGR_210LE, s::SPA_VIDEO_FORMAT_ABGR_210LE);
        assert_eq!(SPA_PRIMARIES_BT2020, s::SPA_VIDEO_COLOR_PRIMARIES_BT2020);
        assert_eq!(SPA_TRANSFER_PQ, s::SPA_VIDEO_TRANSFER_SMPTE2084);
    }

    #[test]
    fn ten_bit_formats_are_mapped() {
        // DRM_FORMAT_XRGB2101010 / XBGR2101010 / ARGB2101010 / ABGR2101010
        assert_eq!(drm_fourcc(spa_fmt10::XRGB_210LE), Some(0x3033_5258));
        assert_eq!(drm_fourcc(spa_fmt10::XBGR_210LE), Some(0x3033_4258));
        assert_eq!(drm_fourcc(spa_fmt10::ARGB_210LE), Some(0x3033_5241));
        assert_eq!(drm_fourcc(spa_fmt10::ABGR_210LE), Some(0x3033_4241));
        for f in OFFERED_FORMATS_10 {
            assert!(is_ten_bit(f) && av_pix_fmt_name(f).is_some());
            assert!(!OFFERED_FORMATS.contains(&f));
        }
        assert!(OFFERED_FORMATS.iter().all(|f| !is_ten_bit(*f)));
    }

    fn negotiated(
        spa_format: u32,
        primaries: Option<u32>,
        transfer: Option<u32>,
    ) -> NegotiatedFormat {
        NegotiatedFormat {
            width: 8,
            height: 8,
            spa_format,
            modifier: None,
            color: SpaColor {
                primaries,
                transfer,
                matrix: None,
            },
        }
    }

    #[test]
    fn hdr10_needs_ten_bits_bt2020_and_pq() {
        let (p, t) = (Some(SPA_PRIMARIES_BT2020), Some(SPA_TRANSFER_PQ));
        assert!(negotiated(spa_fmt10::XRGB_210LE, p, t).is_hdr10());
        assert!(negotiated(spa_fmt10::ABGR_210LE, p, t).is_hdr10());
        // 8 bits, BT.709 primaries (1), sRGB transfer (13), HLG (18), nothing stated.
        assert!(!negotiated(spa_fmt::BGRX, p, t).is_hdr10());
        assert!(!negotiated(spa_fmt10::XRGB_210LE, Some(1), t).is_hdr10());
        assert!(!negotiated(spa_fmt10::XRGB_210LE, p, Some(13)).is_hdr10());
        assert!(!negotiated(spa_fmt10::XRGB_210LE, p, Some(18)).is_hdr10());
        assert!(!negotiated(spa_fmt10::XRGB_210LE, None, None).is_hdr10());
        assert!(!negotiated(spa_fmt10::XRGB_210LE, p, None).is_hdr10());
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
        let sup = [0x0100_0000_0000_0002, 0];
        assert_eq!(
            choose_modifier(&[5, 0, 0x0100_0000_0000_0002], &sup),
            Some(0x0100_0000_0000_0002)
        );
        assert_eq!(choose_modifier(&[5, 0], &sup), Some(0));
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
