//! HDR10 colour math, platform independent and unit-tested everywhere.
//!
//! Two jobs:
//!
//! * unit conversions from what DXGI reports for an HDR output (`DXGI_OUTPUT_DESC1`, the SDR
//!   white level of the display configuration) to the values the protocol and FFmpeg use;
//! * a per-pixel reference implementation of the two GPU conversions in `win/hdr.rs`
//!   (scRGB -> P010 PQ and scRGB -> SDR BGRA8). The Windows-only WARP test compares the shaders
//!   against these functions, so the formulas here and the HLSL there must stay in step.
//!
//! Desktop Duplication delivers an HDR desktop as scRGB: linear light, BT.709 primaries,
//! `1.0 = 80 cd/m2`, values above 1.0 for highlights and below 0.0 for colours outside BT.709.

use ezd_host_core::SourceHdr;
use ezd_proto::Mastering;

/// Luminance of scRGB 1.0 in cd/m2.
pub const SCRGB_WHITE_NITS: f32 = 80.0;
/// Peak of the PQ curve in cd/m2.
pub const PQ_PEAK_NITS: f32 = 10_000.0;
/// SDR white level assumed when Windows does not tell (the sRGB reference white).
pub const DEFAULT_SDR_WHITE_NITS: f32 = 80.0;

/// Used when the driver reports no peak luminance (virtual displays often do not).
pub const FALLBACK_MAX_CLL: u16 = 1000;
/// Used when the driver reports no full-frame luminance.
pub const FALLBACK_MAX_FALL: u16 = 400;

/// BT.709 linear RGB to BT.2020 linear RGB (ITU-R BT.2087), row-major.
pub const M709_TO_2020: [[f32; 3]; 3] = [
    [0.627_404, 0.329_283, 0.043_313],
    [0.069_097, 0.919_540, 0.011_362],
    [0.016_391, 0.088_013, 0.895_595],
];

/// BT.2020 luma coefficients.
pub const KR: f32 = 0.2627;
pub const KB: f32 = 0.0593;
pub const KG: f32 = 1.0 - KR - KB;

// ------------------------------------------------------------------ DXGI -> protocol units

/// The parts of `DXGI_OUTPUT_DESC1` we use, as plain data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputColor {
    /// `ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020`: Windows HDR is on.
    pub hdr: bool,
    pub red: [f32; 2],
    pub green: [f32; 2],
    pub blue: [f32; 2],
    pub white: [f32; 2],
    /// cd/m2.
    pub min_luminance: f32,
    /// cd/m2.
    pub max_luminance: f32,
    /// cd/m2.
    pub max_full_frame_luminance: f32,
}

/// BT.2020 primaries and the D65 white point (CIE xy), the fallback when the driver reports none.
const BT2020_XY: ([f32; 2], [f32; 2], [f32; 2], [f32; 2]) = (
    [0.708, 0.292],
    [0.170, 0.797],
    [0.131, 0.046],
    [0.3127, 0.3290],
);

fn chroma_units(v: f32) -> u16 {
    // 0.00002 units.
    (v.clamp(0.0, 1.0) * 50_000.0).round() as u16
}

fn valid_xy(p: [f32; 2]) -> bool {
    p.iter().all(|v| v.is_finite() && *v > 0.0 && *v < 1.0)
}

impl OutputColor {
    /// The HDR10 properties for the stream. Missing or absurd values (a virtual display driver
    /// may invent them) are replaced: BT.2020 primaries and D65 without usable chromaticities,
    /// 1000 / 400 cd/m2 without luminances. `None` when the output is not HDR.
    pub fn source_hdr(&self) -> Option<SourceHdr> {
        if !self.hdr {
            return None;
        }
        let ok = [self.red, self.green, self.blue, self.white]
            .into_iter()
            .all(valid_xy);
        let (r, g, b, w) = if ok {
            (self.red, self.green, self.blue, self.white)
        } else {
            BT2020_XY
        };
        let sane = |v: f32| v.is_finite() && v > 0.0;
        let max_cll_nits = if sane(self.max_luminance) {
            self.max_luminance.min(PQ_PEAK_NITS)
        } else {
            f32::from(FALLBACK_MAX_CLL)
        };
        let max_cll = (max_cll_nits.round() as u32).clamp(1, 10_000) as u16;
        let fall_nits = if sane(self.max_full_frame_luminance) {
            self.max_full_frame_luminance
        } else {
            f32::from(FALLBACK_MAX_FALL)
        };
        // The frame average cannot exceed the peak.
        let max_fall = (fall_nits.round() as u32).clamp(1, u32::from(max_cll)) as u16;
        let min_nits = if self.min_luminance.is_finite() {
            self.min_luminance.clamp(0.0, max_cll_nits)
        } else {
            0.0
        };
        Some(SourceHdr {
            mastering: Some(Mastering {
                primaries: [r, g, b].map(|p| [chroma_units(p[0]), chroma_units(p[1])]),
                white: [chroma_units(w[0]), chroma_units(w[1])],
                // 0.0001 cd/m2 units.
                max_luminance: (max_cll_nits * 10_000.0).round() as u32,
                min_luminance: (min_nits * 10_000.0).round() as u32,
            }),
            max_cll,
            max_fall,
        })
    }
}

/// SDR white in cd/m2 from `DISPLAYCONFIG_SDR_WHITE_LEVEL::SDRWhiteLevel` (1000 = 80 cd/m2, the
/// "SDR content brightness" slider of Windows). 0 or garbage gives the 80 cd/m2 default.
pub fn sdr_white_nits(raw_level: u32) -> f32 {
    if raw_level == 0 {
        return DEFAULT_SDR_WHITE_NITS;
    }
    (raw_level as f32 / 1000.0 * SCRGB_WHITE_NITS).clamp(40.0, 1000.0)
}

// ------------------------------------------------------------------ reference pixel math

/// SMPTE ST 2084 inverse EOTF: absolute luminance in cd/m2 to a non-linear signal in 0..=1.
pub fn pq_encode(nits: f32) -> f32 {
    const M1: f32 = 0.159_301_76; // 2610 / 16384
    const M2: f32 = 78.84375; // 2523 / 4096 * 128
    const C1: f32 = 0.835_937_5; // 3424 / 4096
    const C2: f32 = 18.851_562; // 2413 / 4096 * 32
    const C3: f32 = 18.6875; // 2392 / 4096 * 32
    let y = (nits / PQ_PEAK_NITS).clamp(0.0, 1.0);
    let p = y.powf(M1);
    ((C1 + C2 * p) / (1.0 + C3 * p)).powf(M2)
}

/// scRGB (linear, BT.709) to the PQ-encoded BT.2020 R'G'B' signal, each channel 0..=1.
pub fn scrgb_to_pq_2020(rgb: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (o, row) in out.iter_mut().zip(M709_TO_2020) {
        let lin = row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2];
        // Colours outside BT.2020 clip to its edge; max() also maps NaN to 0 (clamp would not).
        #[allow(clippy::manual_clamp)]
        let nits = (lin * SCRGB_WHITE_NITS).max(0.0).min(PQ_PEAK_NITS);
        *o = pq_encode(nits);
    }
    out
}

/// BT.2020 non-constant-luminance Y'CbCr, limited range, 10-bit codes (not rounded):
/// Y in 64..=940, Cb and Cr in 64..=960 around 512.
pub fn rgb_to_ycbcr10(e: [f32; 3]) -> [f32; 3] {
    let y = KR * e[0] + KG * e[1] + KB * e[2];
    let cb = (e[2] - y) / (2.0 * (1.0 - KB));
    let cr = (e[0] - y) / (2.0 * (1.0 - KR));
    [64.0 + 876.0 * y, 512.0 + 896.0 * cb, 512.0 + 896.0 * cr]
}

/// scRGB pixel to the P010 luma code (10-bit, rounded), as the Y shader writes it.
pub fn scrgb_to_p010_luma(rgb: [f32; 3]) -> u16 {
    round_code(rgb_to_ycbcr10(scrgb_to_pq_2020(rgb))[0])
}

/// scRGB 2x2 block (row-major) to the P010 `(Cb, Cr)` codes of its chroma sample, as the UV
/// shader writes them: the four pixels' R'G'B' are averaged (a 2x2 box filter, so the chroma
/// sample is centred on the block), then converted.
pub fn scrgb_to_p010_chroma(block: [[f32; 3]; 4]) -> (u16, u16) {
    let mut avg = [0.0f32; 3];
    for px in block {
        let e = scrgb_to_pq_2020(px);
        for (a, v) in avg.iter_mut().zip(e) {
            *a += v / 4.0;
        }
    }
    let c = rgb_to_ycbcr10(avg);
    (round_code(c[1]), round_code(c[2]))
}

fn round_code(code: f32) -> u16 {
    (code + 0.5).floor().clamp(0.0, 1023.0) as u16
}

/// The normalised value written to an `R16_UNORM` / `R16G16_UNORM` view of a P010 plane so the
/// 10-bit `code` lands in the sample's high bits (`code << 6`).
pub fn p010_unorm(code: u16) -> f32 {
    f32::from(code) * 64.0 / 65_535.0
}

/// The 16-bit word stored in a P010 sample for `code`.
pub fn p010_word(code: u16) -> u16 {
    code << 6
}

/// sRGB OETF.
pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// scRGB to the SDR pixel shown at `sdr_white_nits`, as 8-bit R, G, B.
///
/// 1. Scale so the user's SDR white (`sdr_white_nits`) becomes exactly 1.0: the desktop then
///    looks as it does in SDR.
/// 2. Colours outside BT.709 (negative components) clip to the gamut edge.
/// 3. An SDR display has no headroom above white. Anything brighter is divided by its largest
///    channel, which keeps the hue and saturation ratios instead of letting one channel clip
///    and shift the colour. (A Reinhard shoulder would have to start below 1.0 and dim SDR
///    white itself, which is what the UI is made of, so there is none.)
/// 4. sRGB OETF.
pub fn scrgb_to_sdr8(rgb: [f32; 3], sdr_white_nits: f32) -> [u8; 3] {
    let scale = SCRGB_WHITE_NITS / sdr_white_nits.max(1.0);
    let mut c = rgb.map(|v| (v * scale).max(0.0));
    let m = c[0].max(c[1]).max(c[2]);
    if m > 1.0 {
        c = c.map(|v| v / m);
    }
    c.map(|v| (srgb_encode(v) * 255.0 + 0.5).floor() as u8)
}

/// IEEE binary16 bits of `v` (round to nearest even, overflow to infinity). Used to build FP16
/// test textures; the capture itself never converts on the CPU.
pub fn f32_to_f16_bits(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x007f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x0200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x0080_0000;
        let shift = (14 - e) as u32;
        let mut h = (m >> shift) as u16;
        let rem = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        if rem > half || (rem == half && h & 1 == 1) {
            h += 1;
        }
        return sign | h;
    }
    let mut h = sign | ((e as u16) << 10) | (man >> 13) as u16;
    let rem = man & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    h
}

/// The value of binary16 `bits`.
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let man = f32::from(bits & 0x3ff);
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 if man == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(hdr: bool) -> OutputColor {
        OutputColor {
            hdr,
            red: [0.708, 0.292],
            green: [0.170, 0.797],
            blue: [0.131, 0.046],
            white: [0.3127, 0.3290],
            min_luminance: 0.001,
            max_luminance: 1015.0,
            max_full_frame_luminance: 600.0,
        }
    }

    #[test]
    fn sdr_output_has_no_hdr_source() {
        assert_eq!(out(false).source_hdr(), None);
    }

    #[test]
    fn desc1_converts_to_protocol_units() {
        let h = out(true).source_hdr().unwrap();
        let m = h.mastering.unwrap();
        assert_eq!(m.primaries, [[35400, 14600], [8500, 39850], [6550, 2300]]);
        assert_eq!(m.white, [15635, 16450]);
        assert_eq!(m.max_luminance, 10_150_000);
        assert_eq!(m.min_luminance, 10);
        assert_eq!((h.max_cll, h.max_fall), (1015, 600));
    }

    #[test]
    fn missing_values_use_fallbacks() {
        let mut o = out(true);
        o.max_luminance = 0.0;
        o.max_full_frame_luminance = 0.0;
        o.red = [0.0, 0.0];
        let h = o.source_hdr().unwrap();
        assert_eq!((h.max_cll, h.max_fall), (1000, 400));
        let m = h.mastering.unwrap();
        assert_eq!(m.max_luminance, 10_000_000);
        // BT.2020 red.
        assert_eq!(m.primaries[0], [35400, 14600]);
        assert_eq!(m.white, [15635, 16450]);
    }

    #[test]
    fn absurd_values_are_clamped() {
        let mut o = out(true);
        o.max_luminance = 60_000.0;
        o.max_full_frame_luminance = 90_000.0;
        o.min_luminance = f32::NAN;
        let h = o.source_hdr().unwrap();
        assert_eq!(h.max_cll, 10_000);
        assert_eq!(h.max_fall, 10_000, "frame average never above the peak");
        assert_eq!(h.mastering.unwrap().min_luminance, 0);
        o.max_luminance = f32::NAN;
        o.max_full_frame_luminance = f32::INFINITY;
        let h = o.source_hdr().unwrap();
        assert_eq!((h.max_cll, h.max_fall), (1000, 400));
    }

    #[test]
    fn sdr_white_level() {
        assert_eq!(sdr_white_nits(1000), 80.0);
        assert_eq!(sdr_white_nits(2500), 200.0);
        assert_eq!(sdr_white_nits(6000), 480.0);
        assert_eq!(sdr_white_nits(0), 80.0, "unavailable -> default");
        assert_eq!(sdr_white_nits(u32::MAX), 1000.0);
        assert_eq!(sdr_white_nits(1), 40.0);
    }

    #[test]
    fn pq_known_values() {
        assert!(pq_encode(0.0).abs() < 1e-6);
        assert!((pq_encode(100.0) - 0.5081).abs() < 1e-3);
        assert!((pq_encode(1000.0) - 0.7518).abs() < 1e-3);
        assert!((pq_encode(10_000.0) - 1.0).abs() < 1e-5);
        assert!((pq_encode(1e6) - 1.0).abs() < 1e-5, "clamped at the peak");
        assert!(pq_encode(-5.0).abs() < 1e-6);
    }

    #[test]
    fn matrix_rows_sum_to_one() {
        for row in M709_TO_2020 {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
    }

    fn luma(rgb: [f32; 3]) -> u16 {
        scrgb_to_p010_luma(rgb)
    }

    #[test]
    fn p010_black_and_whites() {
        assert_eq!(luma([0.0; 3]), 64);
        // 80 nits.
        let y80 = luma([1.0; 3]);
        assert_eq!(y80, round_code(64.0 + 876.0 * pq_encode(80.0)));
        // 200 and 480 nits (SDR white raised in Windows, scRGB 2.5 and 6.0).
        let y200 = luma([2.5; 3]);
        let y480 = luma([6.0; 3]);
        assert!(y80 < y200 && y200 < y480);
        assert_eq!(y200, round_code(64.0 + 876.0 * pq_encode(200.0)));
        assert_eq!(y480, round_code(64.0 + 876.0 * pq_encode(480.0)));
        // 1000 nits = scRGB 12.5; 10000 nits and beyond reach the top of the range.
        assert_eq!(luma([12.5; 3]), round_code(64.0 + 876.0 * 0.7518));
        assert_eq!(luma([125.0; 3]), 940);
        assert_eq!(luma([1e6; 3]), 940, "clamped");
    }

    #[test]
    fn white_has_neutral_chroma() {
        for v in [1.0, 2.5, 12.5] {
            let c = scrgb_to_p010_chroma([[v; 3]; 4]);
            assert_eq!(c, (512, 512), "scRGB {v}");
        }
        assert_eq!(scrgb_to_p010_chroma([[0.0; 3]; 4]), (512, 512));
    }

    #[test]
    fn negative_values_clip() {
        // Outside BT.709 but inside BT.2020: the red of BT.2020 is (1.66, -0.12, -0.02) in scRGB x some nits.
        let neg = scrgb_to_pq_2020([-1.0, -1.0, -1.0]);
        assert!(neg.iter().all(|v| *v < 1e-5), "{neg:?}");
        assert_eq!(luma([-3.0, -0.5, -2.0]), 64);
        let c = scrgb_to_p010_chroma([[-1.0; 3]; 4]);
        assert_eq!(c, (512, 512));
        // NaN is treated as black rather than poisoning the picture.
        assert_eq!(luma([f32::NAN, 0.0, 0.0]), 64);
    }

    #[test]
    fn saturated_bt2020_red() {
        // 100 nits of BT.2020 red in scRGB: the inverse of the 709->2020 matrix applied to (1,0,0).
        let k = 100.0 / SCRGB_WHITE_NITS;
        let rgb = [1.660_491 * k, -0.124_550 * k, -0.018_151 * k];
        let e = scrgb_to_pq_2020(rgb);
        assert!((e[0] - pq_encode(100.0)).abs() < 2e-3, "{e:?}");
        assert!(e[1] < 5e-3 && e[2] < 5e-3, "{e:?}");
        let y = luma(rgb);
        let (cb, cr) = scrgb_to_p010_chroma([rgb; 4]);
        assert!(cr > 700, "strong Cr for red: {cr}");
        assert!(cb < 512, "Cb below neutral for red: {cb}");
        let expect_y = 64.0 + 876.0 * KR * pq_encode(100.0);
        assert!((f32::from(y) - expect_y).abs() < 3.0, "{y} vs {expect_y}");
    }

    #[test]
    fn half_float_conversion() {
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(0.5), 0x3800);
        assert_eq!(f32_to_f16_bits(-2.0), 0xc000);
        assert_eq!(f32_to_f16_bits(12.5), 0x4a40);
        assert_eq!(f32_to_f16_bits(65504.0), 0x7bff);
        assert_eq!(f32_to_f16_bits(1e6), 0x7c00);
        assert_eq!(f32_to_f16_bits(0.0), 0);
        assert_eq!(f32_to_f16_bits(f32::NAN) & 0x7c00, 0x7c00);
        for v in [
            0.0f32, 1.0, -0.5, 2.5, 6.0, 12.5, 125.0, 0.18, 1e-6, 65504.0,
        ] {
            let back = f16_bits_to_f32(f32_to_f16_bits(v));
            assert!((back - v).abs() <= v.abs() * 1e-3 + 1e-7, "{v} -> {back}");
        }
    }

    #[test]
    fn p010_packing() {
        assert_eq!(p010_word(64), 4096);
        assert_eq!(p010_word(940), 60160);
        // A normalised write of p010_unorm(code) stores exactly code << 6 in a 16-bit UNORM view.
        for code in [0u16, 64, 512, 940, 1023] {
            let stored = (p010_unorm(code) * 65_535.0).round() as u16;
            assert_eq!(stored, p010_word(code), "code {code}");
        }
    }

    #[test]
    fn sdr_white_maps_to_white() {
        for nits in [80.0f32, 200.0, 480.0] {
            let v = nits / SCRGB_WHITE_NITS;
            assert_eq!(scrgb_to_sdr8([v; 3], nits), [255; 3], "{nits} nits");
            // Half of white: the same signal at any white level.
            assert_eq!(
                scrgb_to_sdr8([v / 2.0; 3], nits),
                scrgb_to_sdr8([0.5; 3], 80.0)
            );
        }
        assert_eq!(scrgb_to_sdr8([0.0; 3], 200.0), [0; 3]);
        // sRGB code for 50% linear.
        assert_eq!(scrgb_to_sdr8([0.5; 3], 80.0), [188; 3]);
    }

    #[test]
    fn sdr_highlights_keep_hue_and_negatives_clip() {
        // 1000 nits white at 200 nits SDR white: clipped to white, not tinted.
        assert_eq!(scrgb_to_sdr8([12.5; 3], 200.0), [255; 3]);
        // A bright orange keeps the channel ratios (max channel -> 1.0).
        let o = scrgb_to_sdr8([10.0, 5.0, 0.0], 80.0);
        assert_eq!(o[0], 255);
        assert_eq!(o[1], scrgb_to_sdr8([0.5, 0.5, 0.5], 80.0)[0]);
        assert_eq!(o[2], 0);
        // Negative components clip to zero.
        assert_eq!(scrgb_to_sdr8([1.0, -0.4, -2.0], 80.0), [255, 0, 0]);
        assert_eq!(scrgb_to_sdr8([f32::NAN, 0.0, 0.0], 80.0), [0; 3]);
    }
}
