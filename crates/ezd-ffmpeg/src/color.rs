//! Colour description and HDR10 static metadata for the encoder.

use ezd_proto::{Mastering, StreamColor};
use ffmpeg_sys_next as ffi;

/// Colour signalling of the encoded stream, as ITU-T H.273 code points.
///
/// Written to the codec context (VUI) and to every frame sent to the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorInfo {
    /// Colour primaries (1 = BT.709, 9 = BT.2020).
    pub primaries: u8,
    /// Transfer characteristics (1 = BT.709, 16 = PQ, 18 = HLG).
    pub transfer: u8,
    /// Matrix coefficients (1 = BT.709, 9 = BT.2020 NCL).
    pub matrix: u8,
    /// Full range (JPEG) instead of limited range (MPEG) samples.
    pub full_range: bool,
}

impl ColorInfo {
    /// BT.709, limited range: what SDR streams have always been tagged with.
    pub const BT709_LIMITED: ColorInfo = ColorInfo {
        primaries: 1,
        transfer: 1,
        matrix: 1,
        full_range: false,
    };

    /// BT.2020 primaries, PQ transfer, BT.2020 NCL matrix, limited range (HDR10).
    pub const BT2020_PQ_LIMITED: ColorInfo = ColorInfo {
        primaries: 9,
        transfer: 16,
        matrix: 9,
        full_range: false,
    };

    pub(crate) fn av_primaries(self) -> ffi::AVColorPrimaries {
        use ffi::AVColorPrimaries as P;
        match self.primaries {
            1 => P::AVCOL_PRI_BT709,
            4 => P::AVCOL_PRI_BT470M,
            5 => P::AVCOL_PRI_BT470BG,
            6 => P::AVCOL_PRI_SMPTE170M,
            7 => P::AVCOL_PRI_SMPTE240M,
            8 => P::AVCOL_PRI_FILM,
            9 => P::AVCOL_PRI_BT2020,
            10 => P::AVCOL_PRI_SMPTE428,
            11 => P::AVCOL_PRI_SMPTE431,
            12 => P::AVCOL_PRI_SMPTE432,
            _ => P::AVCOL_PRI_UNSPECIFIED,
        }
    }

    pub(crate) fn av_transfer(self) -> ffi::AVColorTransferCharacteristic {
        use ffi::AVColorTransferCharacteristic as T;
        match self.transfer {
            1 => T::AVCOL_TRC_BT709,
            4 => T::AVCOL_TRC_GAMMA22,
            5 => T::AVCOL_TRC_GAMMA28,
            6 => T::AVCOL_TRC_SMPTE170M,
            7 => T::AVCOL_TRC_SMPTE240M,
            8 => T::AVCOL_TRC_LINEAR,
            13 => T::AVCOL_TRC_IEC61966_2_1,
            14 => T::AVCOL_TRC_BT2020_10,
            15 => T::AVCOL_TRC_BT2020_12,
            16 => T::AVCOL_TRC_SMPTE2084,
            17 => T::AVCOL_TRC_SMPTE428,
            18 => T::AVCOL_TRC_ARIB_STD_B67,
            _ => T::AVCOL_TRC_UNSPECIFIED,
        }
    }

    pub(crate) fn av_matrix(self) -> ffi::AVColorSpace {
        use ffi::AVColorSpace as S;
        match self.matrix {
            0 => S::AVCOL_SPC_RGB,
            1 => S::AVCOL_SPC_BT709,
            4 => S::AVCOL_SPC_FCC,
            5 => S::AVCOL_SPC_BT470BG,
            6 => S::AVCOL_SPC_SMPTE170M,
            7 => S::AVCOL_SPC_SMPTE240M,
            9 => S::AVCOL_SPC_BT2020_NCL,
            10 => S::AVCOL_SPC_BT2020_CL,
            _ => S::AVCOL_SPC_UNSPECIFIED,
        }
    }

    pub(crate) fn av_range(self) -> ffi::AVColorRange {
        if self.full_range {
            ffi::AVColorRange::AVCOL_RANGE_JPEG
        } else {
            ffi::AVColorRange::AVCOL_RANGE_MPEG
        }
    }
}

impl Default for ColorInfo {
    fn default() -> Self {
        ColorInfo::BT709_LIMITED
    }
}

impl From<&StreamColor> for ColorInfo {
    fn from(c: &StreamColor) -> Self {
        ColorInfo {
            primaries: c.primaries,
            transfer: c.transfer,
            matrix: c.matrix,
            full_range: c.full_range,
        }
    }
}

/// HDR10 static metadata (SMPTE ST 2086 mastering display and CTA-861.3 light levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HdrMeta {
    /// Mastering display colour volume, if known.
    pub mastering: Option<Mastering>,
    /// Maximum content light level in cd/m2 (0 = unknown).
    pub max_cll: u16,
    /// Maximum frame-average light level in cd/m2 (0 = unknown).
    pub max_fall: u16,
}

impl HdrMeta {
    /// The metadata of an HDR stream colour; `None` when it carries none.
    pub fn from_stream_color(c: &StreamColor) -> Option<HdrMeta> {
        (c.mastering.is_some() || c.max_cll != 0 || c.max_fall != 0).then_some(HdrMeta {
            mastering: c.mastering,
            max_cll: c.max_cll,
            max_fall: c.max_fall,
        })
    }

    pub(crate) fn raw_light_level(&self) -> Option<RawLightLevel> {
        (self.max_cll != 0 || self.max_fall != 0).then_some(RawLightLevel {
            max_cll: self.max_cll as u32,
            max_fall: self.max_fall as u32,
        })
    }
}

/// Layout of FFmpeg's `AVMasteringDisplayMetadata` (the header is not in the bindings).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawMastering {
    pub primaries: [[ffi::AVRational; 2]; 3],
    pub white: [ffi::AVRational; 2],
    pub min_luminance: ffi::AVRational,
    pub max_luminance: ffi::AVRational,
    pub has_primaries: i32,
    pub has_luminance: i32,
}

/// Layout of FFmpeg's `AVContentLightMetadata`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawLightLevel {
    pub max_cll: u32,
    pub max_fall: u32,
}

/// Chromaticities are in 0.00002 units, luminances in 0.0001 cd/m2.
pub(crate) fn raw_mastering(m: &Mastering) -> RawMastering {
    let chroma = |v: u16| ffi::AVRational {
        num: v as i32,
        den: 50_000,
    };
    let lum = |v: u32| ffi::AVRational {
        num: v.min(i32::MAX as u32) as i32,
        den: 10_000,
    };
    RawMastering {
        primaries: m.primaries.map(|p| p.map(chroma)),
        white: m.white.map(chroma),
        min_luminance: lum(m.min_luminance),
        max_luminance: lum(m.max_luminance),
        has_primaries: 1,
        has_luminance: 1,
    }
}

/// Copies `value` into new side data of `frame`.
///
/// # Safety
/// `frame` must be a valid, exclusively accessed `AVFrame`.
unsafe fn add_frame_side_data<T>(
    frame: *mut ffi::AVFrame,
    kind: ffi::AVFrameSideDataType,
    value: T,
) -> Result<(), crate::FfmpegError> {
    // SAFETY: caller contract; the new side data has `size_of::<T>()` writable bytes.
    unsafe {
        let sd = ffi::av_frame_new_side_data(frame, kind, std::mem::size_of::<T>());
        if sd.is_null() {
            return Err(crate::FfmpegError::msg("av_frame_new_side_data failed"));
        }
        std::ptr::write_unaligned((*sd).data as *mut T, value);
    }
    Ok(())
}

/// Sets the colour fields of `frame` and attaches the HDR10 side data.
///
/// # Safety
/// `frame` must be a valid, exclusively accessed `AVFrame` without HDR side data yet.
pub(crate) unsafe fn tag_frame(
    frame: *mut ffi::AVFrame,
    color: ColorInfo,
    hdr: Option<&HdrMeta>,
) -> Result<(), crate::FfmpegError> {
    // SAFETY: caller contract.
    unsafe {
        let f = &mut *frame;
        f.color_primaries = color.av_primaries();
        f.color_trc = color.av_transfer();
        f.colorspace = color.av_matrix();
        f.color_range = color.av_range();
        if let Some(h) = hdr {
            if let Some(m) = &h.mastering {
                add_frame_side_data(
                    frame,
                    ffi::AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA,
                    raw_mastering(m),
                )?;
            }
            if let Some(l) = h.raw_light_level() {
                add_frame_side_data(
                    frame,
                    ffi::AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
                    l,
                )?;
            }
        }
    }
    Ok(())
}

/// Attaches the HDR10 metadata to `avctx->decoded_side_data`, where hardware encoders that read
/// stream-level metadata (NVENC) look for it. Needs FFmpeg 7.1 or newer; older builds rely on
/// the per-frame side data alone.
///
/// # Safety
/// `ctx` must be a valid, not yet opened `AVCodecContext`.
#[cfg(ffmpeg_7_1)]
pub(crate) unsafe fn tag_context(
    ctx: *mut ffi::AVCodecContext,
    hdr: &HdrMeta,
) -> Result<(), crate::FfmpegError> {
    // SAFETY: caller contract; `av_frame_side_data_new` appends to the context-owned array.
    unsafe {
        let c = &mut *ctx;
        let mut add = |kind, size: usize| {
            let sd = ffi::av_frame_side_data_new(
                &mut c.decoded_side_data,
                &mut c.nb_decoded_side_data,
                kind,
                size,
                0,
            );
            if sd.is_null() {
                Err(crate::FfmpegError::msg("av_frame_side_data_new failed"))
            } else {
                Ok((*sd).data)
            }
        };
        if let Some(m) = &hdr.mastering {
            let data = add(
                ffi::AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA,
                std::mem::size_of::<RawMastering>(),
            )?;
            std::ptr::write_unaligned(data as *mut RawMastering, raw_mastering(m));
        }
        if let Some(l) = hdr.raw_light_level() {
            let data = add(
                ffi::AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
                std::mem::size_of::<RawLightLevel>(),
            )?;
            std::ptr::write_unaligned(data as *mut RawLightLevel, l);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffi::{AVColorPrimaries as P, AVColorSpace as S, AVColorTransferCharacteristic as T};

    #[test]
    fn h273_codes_map_to_ffmpeg() {
        let c = ColorInfo::BT709_LIMITED;
        assert_eq!(c.av_primaries(), P::AVCOL_PRI_BT709);
        assert_eq!(c.av_transfer(), T::AVCOL_TRC_BT709);
        assert_eq!(c.av_matrix(), S::AVCOL_SPC_BT709);
        assert_eq!(c.av_range(), ffi::AVColorRange::AVCOL_RANGE_MPEG);
        let h = ColorInfo::from(&StreamColor::hdr10(None, 0, 0));
        assert_eq!(h, ColorInfo::BT2020_PQ_LIMITED);
        assert_eq!(h.av_primaries(), P::AVCOL_PRI_BT2020);
        assert_eq!(h.av_transfer(), T::AVCOL_TRC_SMPTE2084);
        assert_eq!(h.av_matrix(), S::AVCOL_SPC_BT2020_NCL);
        let full = ColorInfo {
            full_range: true,
            ..h
        };
        assert_eq!(full.av_range(), ffi::AVColorRange::AVCOL_RANGE_JPEG);
        let unknown = ColorInfo {
            primaries: 200,
            transfer: 200,
            matrix: 200,
            full_range: false,
        };
        assert_eq!(unknown.av_primaries(), P::AVCOL_PRI_UNSPECIFIED);
        assert_eq!(unknown.av_transfer(), T::AVCOL_TRC_UNSPECIFIED);
        assert_eq!(unknown.av_matrix(), S::AVCOL_SPC_UNSPECIFIED);
        assert_eq!(ColorInfo::default(), ColorInfo::BT709_LIMITED);
        // The H.273 code points are the numeric values of the FFmpeg enums.
        assert_eq!(P::AVCOL_PRI_BT2020 as u8, 9);
        assert_eq!(T::AVCOL_TRC_SMPTE2084 as u8, 16);
        assert_eq!(S::AVCOL_SPC_BT2020_NCL as u8, 9);
    }

    #[test]
    fn mastering_rationals() {
        let m = Mastering {
            primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
            white: [15635, 16450],
            max_luminance: 10_000_000,
            min_luminance: 1,
        };
        let r = raw_mastering(&m);
        let q = |a: ffi::AVRational| (a.num, a.den);
        assert_eq!(q(r.primaries[0][0]), (34000, 50_000));
        assert_eq!(q(r.primaries[2][1]), (3000, 50_000));
        assert_eq!(q(r.white[1]), (16450, 50_000));
        assert_eq!(q(r.max_luminance), (10_000_000, 10_000));
        assert_eq!(q(r.min_luminance), (1, 10_000));
        assert_eq!((r.has_primaries, r.has_luminance), (1, 1));
        // 1000 cd/m2 is 10_000_000 / 10_000 = 1000.
        assert_eq!(r.max_luminance.num / r.max_luminance.den, 1000);
        assert_eq!(std::mem::size_of::<RawMastering>(), 88);
        assert_eq!(std::mem::size_of::<RawLightLevel>(), 8);
    }

    #[test]
    fn hdr_meta_from_stream_color() {
        assert_eq!(
            HdrMeta::from_stream_color(&StreamColor::hdr10(None, 0, 0)),
            None
        );
        let m = HdrMeta::from_stream_color(&StreamColor::hdr10(None, 1000, 400)).unwrap();
        assert_eq!(
            m.raw_light_level(),
            Some(RawLightLevel {
                max_cll: 1000,
                max_fall: 400
            })
        );
        assert_eq!(HdrMeta::default().raw_light_level(), None);
    }
}
