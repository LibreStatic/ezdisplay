//! Rate-control numbers shared by every encoder context setup, and the report of what ran.

use ezd_proto::{RateControl, RateMode};

/// The encoder may burst to this many times the negotiated bitrate.
pub const PEAK_FACTOR: u32 = 2;
/// VBV buffer size in frames at the peak rate.
pub const VBV_FRAMES: u32 = 4;
/// NVENC constant-quality target (`cq`).
pub const NVENC_CQ: u32 = 19;
/// libx264 constant rate factor.
pub const X264_CRF: u32 = 20;
/// Fixed quantiser (CRF for libx264) of the high-quality option sets.
pub const HIGH_QP: u32 = 10;
/// Fixed quantiser of the VAAPI CQP fallback sets.
pub const VAAPI_QP: u32 = 20;

/// Values for `bit_rate`, `rc_max_rate` and `rc_buffer_size` of an encoder context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateSettings {
    /// Target bitrate in bit/s.
    pub bit_rate: i64,
    /// Peak bitrate in bit/s.
    pub max_rate: i64,
    /// VBV buffer size in bits.
    pub buffer_size: i32,
}

/// Context rate settings for a negotiated bitrate and refresh rate (millihertz).
pub fn rate_settings(kbps: u32, refresh_mhz: u32) -> RateSettings {
    let bit_rate = kbps as i64 * 1000;
    let max_rate = bit_rate * PEAK_FACTOR as i64;
    let buffer = max_rate * VBV_FRAMES as i64 * 1000 / refresh_mhz.max(1) as i64;
    RateSettings {
        bit_rate,
        max_rate,
        buffer_size: buffer.clamp(1000, i32::MAX as i64) as i32,
    }
}

/// Rate-control description of the encoder `name` opened with `options`, for the stream report.
pub fn describe_rate(name: &str, options: &[(&str, String)], kbps: u32) -> RateControl {
    let get = |key: &str| {
        options
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    };
    let num = |key: &str| get(key).and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let (mode, quality) = if get("tune") == Some("lossless") {
        (RateMode::Lossless, 0)
    } else if let Some(crf) = get("crf") {
        (RateMode::Crf, crf.parse().unwrap_or(0))
    } else if let Some(rc) = get("rc") {
        match rc {
            "cbr" => (RateMode::Cbr, 0),
            "vbr" | "vbr_peak" | "vbr_latency" => (RateMode::Vbr, num("cq")),
            "cqp" | "constqp" => (RateMode::Cqp, num("qp").max(num("qp_i"))),
            _ => (RateMode::Unknown, 0),
        }
    } else if let Some(rc) = get("rc_mode") {
        match rc {
            "CBR" => (RateMode::Cbr, 0),
            "VBR" => (RateMode::Vbr, 0),
            "CQP" => (RateMode::Cqp, num("qp")),
            _ => (RateMode::Unknown, 0),
        }
    } else if name.ends_with("_qsv") {
        // QSV runs VBR whenever the maximum rate is above the bitrate.
        (RateMode::Vbr, 0)
    } else {
        (RateMode::Unknown, 0)
    };
    RateControl {
        mode: mode.code(),
        quality,
        target_kbps: kbps,
        // A fixed quantiser and lossless ignore the rate limits.
        max_kbps: if matches!(mode, RateMode::Cqp | RateMode::Lossless) {
            0
        } else {
            kbps.saturating_mul(PEAK_FACTOR)
        },
    }
}

/// Compact `k=v` label of the interesting options of a set, joined by spaces.
pub fn options_label(options: &[(&str, String)]) -> String {
    const KEYS: [&str; 9] = [
        "rc",
        "rc_mode",
        "cq",
        "qp",
        "qp_i",
        "crf",
        "low_power",
        "profile",
        "level",
    ];
    options
        .iter()
        .filter(|(k, v)| KEYS.contains(k) || (*k == "tune" && v == "lossless"))
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::option_sets;
    use ezd_proto::QualityLevel;

    #[test]
    fn settings_use_peak_and_four_frame_vbv() {
        let r = rate_settings(45_949, 120_000);
        assert_eq!(r.bit_rate, 45_949_000);
        assert_eq!(r.max_rate, 91_898_000);
        assert_eq!(r.buffer_size, (91_898_000i64 * 4 / 120) as i32);
        assert!(rate_settings(1, 0).buffer_size >= 1000);
    }

    #[test]
    fn describes_the_set_that_opened() {
        let d = |name: &str, i: usize| {
            let sets = option_sets(name, QualityLevel::Normal);
            describe_rate(name, &sets[i], 1000)
        };
        let nv = d("h264_nvenc", 0);
        assert_eq!((nv.rate_mode(), nv.quality), (RateMode::Vbr, 19));
        assert_eq!((nv.target_kbps, nv.max_kbps), (1000, 2000));
        let x = d("libx264", 0);
        assert_eq!((x.rate_mode(), x.quality), (RateMode::Crf, 20));
        let va = d("h264_vaapi", 0);
        assert_eq!((va.rate_mode(), va.quality), (RateMode::Vbr, 0));
        let cqp = d("h264_vaapi", 1);
        assert_eq!((cqp.rate_mode(), cqp.quality), (RateMode::Cqp, 20));
        assert_eq!(cqp.max_kbps, 0);
        assert_eq!(d("h264_qsv", 0).rate_mode(), RateMode::Vbr);
        assert_eq!(d("libopenh264", 0).rate_mode(), RateMode::Unknown);
    }

    #[test]
    fn describes_high_and_lossless_sets() {
        let sets = option_sets("hevc_nvenc", QualityLevel::Lossless);
        let lossless = describe_rate("hevc_nvenc", &sets[0], 1000);
        assert_eq!(
            (lossless.rate_mode(), lossless.quality, lossless.max_kbps),
            (RateMode::Lossless, 0, 0)
        );
        assert_eq!(options_label(&sets[0]), "tune=lossless");
        let high = describe_rate("hevc_nvenc", &sets[1], 1000);
        assert_eq!(
            (high.rate_mode(), high.quality, high.max_kbps),
            (RateMode::Cqp, 10, 0)
        );
        let x = option_sets("libx264", QualityLevel::High);
        let x = describe_rate("libx264", &x[0], 1000);
        assert_eq!((x.rate_mode(), x.quality), (RateMode::Crf, 10));
        let amf = option_sets("hevc_amf", QualityLevel::High);
        let amf = describe_rate("hevc_amf", &amf[0], 1000);
        assert_eq!((amf.rate_mode(), amf.quality), (RateMode::Cqp, 10));
    }

    #[test]
    fn label_keeps_only_interesting_keys() {
        let sets = option_sets("h264_vaapi", QualityLevel::Normal);
        assert_eq!(
            options_label(&sets[1]),
            "rc_mode=CQP qp=20 low_power=1 level=5.2"
        );
    }
}
