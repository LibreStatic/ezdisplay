//! MediaCodec colour keys derived from the stream's [`StreamColor`]. Pure, so it is tested on the host.

use ezd_proto::{Codec, StreamColor};

/// `MediaFormat.COLOR_STANDARD_BT709`.
pub const COLOR_STANDARD_BT709: i32 = 1;
/// `MediaFormat.COLOR_STANDARD_BT2020`.
pub const COLOR_STANDARD_BT2020: i32 = 6;
/// `MediaFormat.COLOR_TRANSFER_SDR_VIDEO`.
pub const COLOR_TRANSFER_SDR_VIDEO: i32 = 3;
/// `MediaFormat.COLOR_TRANSFER_ST2084`.
pub const COLOR_TRANSFER_ST2084: i32 = 6;
/// `MediaFormat.COLOR_TRANSFER_HLG`.
pub const COLOR_TRANSFER_HLG: i32 = 7;
/// `MediaFormat.COLOR_RANGE_FULL`.
pub const COLOR_RANGE_FULL: i32 = 1;
/// `MediaFormat.COLOR_RANGE_LIMITED`.
pub const COLOR_RANGE_LIMITED: i32 = 2;
/// `MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10` and `AV1ProfileMain10HDR10`.
pub const PROFILE_MAIN10_HDR10: i32 = 0x1000;

/// Length of the HDR static metadata buffer.
pub const HDR_STATIC_INFO_LEN: usize = 25;

/// Values for the `color-*`, `profile` and `hdr-static-info` format keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecColor {
    /// `color-standard`.
    pub standard: i32,
    /// `color-transfer`.
    pub transfer: i32,
    /// `color-range`.
    pub range: i32,
    /// `profile`, only for HDR10 on codecs that have an HDR10 profile.
    pub profile: Option<i32>,
    /// `hdr-static-info`, only for PQ streams.
    pub hdr_static_info: Option<[u8; HDR_STATIC_INFO_LEN]>,
}

/// Maps the H.273 description of the stream to MediaFormat values. SDR streams are tagged
/// BT.709 / SDR video / limited (or full) range.
pub fn codec_color(codec: Codec, c: &StreamColor) -> CodecColor {
    let standard = if c.primaries == 9 || c.matrix == 9 || c.matrix == 10 {
        COLOR_STANDARD_BT2020
    } else {
        COLOR_STANDARD_BT709
    };
    let transfer = match c.transfer {
        16 => COLOR_TRANSFER_ST2084,
        18 => COLOR_TRANSFER_HLG,
        _ => COLOR_TRANSFER_SDR_VIDEO,
    };
    let pq = c.transfer == 16;
    CodecColor {
        standard,
        transfer,
        range: if c.full_range {
            COLOR_RANGE_FULL
        } else {
            COLOR_RANGE_LIMITED
        },
        profile: (pq && matches!(codec, Codec::Hevc | Codec::Av1)).then_some(PROFILE_MAIN10_HDR10),
        hdr_static_info: pq.then(|| hdr_static_info(c)),
    }
}

/// Builds the 25-byte CTA-861.3 Type 1 static metadata descriptor, as `MediaFormat.KEY_HDR_STATIC_INFO`
/// expects it. Layout (all multi-byte values little-endian), per the KEY_HDR_STATIC_INFO documentation and
/// AOSP `ColorUtils` / `HDRStaticInfo::Type1` (frameworks/av, media/hardware/HardwareAPI.h):
///
/// | offset | size | field |
/// |---|---|---|
/// | 0 | 1 | descriptor id, 0 for Type 1 |
/// | 1, 3 | 2+2 | red primary x, y (0.00002) |
/// | 5, 7 | 2+2 | green primary x, y |
/// | 9, 11 | 2+2 | blue primary x, y |
/// | 13, 15 | 2+2 | white point x, y |
/// | 17 | 2 | max mastering luminance, cd/m2 |
/// | 19 | 2 | min mastering luminance, 0.0001 cd/m2 |
/// | 21 | 2 | MaxCLL, cd/m2 |
/// | 23 | 2 | MaxFALL, cd/m2 |
///
/// Unknown mastering data is written as zeros (Android treats 0 as unspecified).
pub fn hdr_static_info(c: &StreamColor) -> [u8; HDR_STATIC_INFO_LEN] {
    let mut out = [0u8; HDR_STATIC_INFO_LEN];
    let mut put = |off: usize, v: u16| out[off..off + 2].copy_from_slice(&v.to_le_bytes());
    if let Some(m) = &c.mastering {
        for (i, p) in m.primaries.iter().enumerate() {
            put(1 + i * 4, p[0]);
            put(3 + i * 4, p[1]);
        }
        put(13, m.white[0]);
        put(15, m.white[1]);
        put(17, (m.max_luminance / 10_000).min(u16::MAX as u32) as u16);
        put(19, m.min_luminance.min(u16::MAX as u32) as u16);
    }
    put(21, c.max_cll);
    put(23, c.max_fall);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezd_proto::Mastering;

    fn hdr() -> StreamColor {
        StreamColor::hdr10(
            Some(Mastering {
                primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
                white: [15635, 16450],
                max_luminance: 10_000_000,
                min_luminance: 50,
            }),
            1000,
            400,
        )
    }

    #[test]
    fn static_info_exact_bytes() {
        let b = hdr_static_info(&hdr());
        let mut want = vec![0u8];
        for v in [
            34000u16, 16000, 13250, 34500, 7500, 3000, 15635, 16450, 1000, 50, 1000, 400,
        ] {
            want.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(b.to_vec(), want);
        assert_eq!(b.len(), 25);
        // Spot check the little-endian encoding of red x = 34000 = 0x84D0 and max luminance 1000 = 0x03E8.
        assert_eq!(&b[1..3], &[0xD0, 0x84]);
        assert_eq!(&b[17..19], &[0xE8, 0x03]);
        assert_eq!(&b[19..21], &[50, 0]);
    }

    #[test]
    fn static_info_without_mastering_keeps_light_levels() {
        let b = hdr_static_info(&StreamColor::hdr10(None, 600, 200));
        assert!(b[..21].iter().all(|&x| x == 0));
        assert_eq!(&b[21..], &[0x58, 0x02, 200, 0]);
    }

    #[test]
    fn hdr10_hevc_keys() {
        let k = codec_color(Codec::Hevc, &hdr());
        assert_eq!(k.standard, 6);
        assert_eq!(k.transfer, 6);
        assert_eq!(k.range, 2);
        assert_eq!(k.profile, Some(0x1000));
        assert!(k.hdr_static_info.is_some());
        assert_eq!(codec_color(Codec::Av1, &hdr()).profile, Some(0x1000));
        assert_eq!(codec_color(Codec::H264, &hdr()).profile, None);
    }

    #[test]
    fn sdr_is_tagged_bt709_limited() {
        let k = codec_color(Codec::H264, &StreamColor::sdr_bt709());
        assert_eq!((k.standard, k.transfer, k.range), (1, 3, 2));
        assert_eq!(k.profile, None);
        assert_eq!(k.hdr_static_info, None);
    }
}
