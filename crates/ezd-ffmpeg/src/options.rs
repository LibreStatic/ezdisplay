//! Low-latency option presets per encoder name and quality level.

use crate::rate::HIGH_QP;
use ezd_proto::QualityLevel;

/// Ordered fallback chain of AVOption sets for `name`: the first set that
/// opens successfully is used (e.g. VAAPI with `low_power` first, then without).
///
/// [`QualityLevel::Normal`] is the rate-controlled preset sized to the link.
/// [`QualityLevel::High`] is a fixed quantiser of [`HIGH_QP`] (CRF for libx264). Lossless is the
/// `hevc_nvenc` lossless tune followed by the high sets, so a failed lossless open falls back to
/// high quality; every other encoder gets the high sets.
pub fn option_sets(name: &str, quality: QualityLevel) -> Vec<Vec<(&'static str, String)>> {
    option_sets_for(name, false, quality)
}

/// Like [`option_sets`] for 10-bit input (P010): the HEVC encoders get `profile=main10`, and
/// `hevc_vaapi` loses its `low_power` set (Gen 9.5 has no low-power HEVC Main10 and some
/// drivers open it anyway). Other encoders are unchanged.
pub fn option_sets_for(
    name: &str,
    ten_bit: bool,
    quality: QualityLevel,
) -> Vec<Vec<(&'static str, String)>> {
    let mut sets = match quality {
        QualityLevel::Normal => normal_option_sets(name),
        QualityLevel::High => high_option_sets(name),
        QualityLevel::Lossless => {
            let mut sets = Vec::new();
            if name == "hevc_nvenc" {
                sets.push(vec![
                    ("preset", "p1".to_string()),
                    ("tune", "lossless".to_string()),
                    ("zerolatency", "1".to_string()),
                    ("delay", "0".to_string()),
                    ("bf", "0".to_string()),
                    ("forced-idr", "1".to_string()),
                ]);
            }
            sets.extend(high_option_sets(name));
            sets
        }
    };
    if ten_bit && matches!(name, "hevc_nvenc" | "hevc_qsv" | "hevc_amf" | "hevc_vaapi") {
        if name == "hevc_vaapi" {
            sets.retain(|set| !set.iter().any(|(k, _)| *k == "low_power"));
        }
        for set in &mut sets {
            set.push(("profile", "main10".to_string()));
        }
    }
    sets
}

fn normal_option_sets(name: &str) -> Vec<Vec<(&'static str, String)>> {
    let s = |k: &'static str, v: &str| (k, v.to_string());
    match name {
        "libx264" => vec![vec![
            s("preset", "ultrafast"),
            s("tune", "zerolatency"),
            s("crf", &crate::rate::X264_CRF.to_string()),
            s("forced-idr", "1"),
            // Keep in-band SPS/PPS on keyframes in addition to the global-header extradata.
            s(
                "x264-params",
                "repeat-headers=1:scenecut=0:rc-lookahead=0:sliced-threads=1",
            ),
        ]],
        "libopenh264" => vec![vec![s("allow_skip_frames", "0")]],
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => vec![vec![
            s("preset", "p1"),
            s("tune", "ull"),
            s("zerolatency", "1"),
            s("delay", "0"),
            s("bf", "0"),
            // Constant quality inside the VBV: a sharp first IDR and few bits on still content.
            s("rc", "vbr"),
            s("cq", &crate::rate::NVENC_CQ.to_string()),
            s("forced-idr", "1"),
        ]],
        // Level 5.2: FFmpeg picks level 6.x above 5.2's macroblock rate (2960x1848 at 120 Hz),
        // and Intel's iHD driver then fails every rate-controlled frame ("internal encoding error") on
        // Gen 9.5. 5.2 still covers the frame size and buffering, and the tablet's decoder
        // accepts it.
        //
        // Fixed QP in low-power mode comes before the regular mode: Gen 9.5 has no rate control
        // in low-power mode, but the regular mode runs on the shader cores the compositor also
        // uses: on a UHD 630 with KWin and a 120 Hz video on screen, a 2960x1848 frame took
        // 34 ms there against 13 ms in low-power mode. The video credit loop keeps a larger
        // frame from queueing up behind the link.
        "h264_vaapi" => vec![
            vec![
                s("rc_mode", "VBR"),
                s("low_power", "1"),
                s("async_depth", "1"),
                s("level", "5.2"),
            ],
            vec![
                s("rc_mode", "CQP"),
                s("qp", &crate::rate::VAAPI_QP.to_string()),
                s("low_power", "1"),
                s("async_depth", "1"),
                s("level", "5.2"),
            ],
            vec![
                s("rc_mode", "VBR"),
                s("async_depth", "1"),
                s("level", "5.2"),
            ],
        ],
        "hevc_vaapi" | "av1_vaapi" => vec![
            vec![
                s("rc_mode", "VBR"),
                s("low_power", "1"),
                s("async_depth", "1"),
            ],
            vec![s("rc_mode", "VBR"), s("async_depth", "1")],
        ],
        "h264_qsv" | "hevc_qsv" | "av1_qsv" => vec![
            vec![
                s("low_power", "1"),
                s("async_depth", "1"),
                s("forced_idr", "1"),
            ],
            vec![s("async_depth", "1"), s("forced_idr", "1")],
        ],
        // Older encoder blocks (VCE) refuse the ultra-low-latency usage (Init error 3) and, on
        // Polaris, low latency with CBR (error 36), so step down to fewer options and finally the
        // driver's defaults. Every set keeps `async_depth=1`: with FFmpeg's default of 16,
        // receive_packet returns before the frame is encoded and the packet only comes out with
        // the next frame (an extra frame of latency, and the last frame stuck while the screen is
        // still). The speed preset keeps a 2960x1848 frame within a 120 Hz budget on VCE.
        "h264_amf" | "hevc_amf" | "av1_amf" => {
            let sets: [&[(&'static str, &str)]; 6] = [
                &[
                    ("usage", "ultralowlatency"),
                    ("rc", "vbr_peak"),
                    ("quality", "speed"),
                ],
                &[
                    ("usage", "lowlatency"),
                    ("rc", "vbr_peak"),
                    ("quality", "speed"),
                ],
                &[("usage", "lowlatency"), ("quality", "speed")],
                &[("rc", "vbr_peak"), ("quality", "speed")],
                &[("quality", "speed")],
                &[],
            ];
            sets.iter()
                .map(|set| {
                    let mut v: Vec<_> = set.iter().map(|&(k, val)| s(k, val)).collect();
                    v.push(s("forced_idr", "1"));
                    v.push(s("async_depth", "1"));
                    v
                })
                .collect()
        }
        _ => vec![vec![]],
    }
}

/// Near-transparent constant-quantiser sets. Encoders without a verified fixed-quantiser mode
/// keep their normal sets: QSV needs `AV_CODEC_FLAG_QSCALE` for CQP, which the encoder context
/// does not set, and its ICQ mode is not available on every driver.
fn high_option_sets(name: &str) -> Vec<Vec<(&'static str, String)>> {
    let s = |k: &'static str, v: &str| (k, v.to_string());
    let qp = HIGH_QP.to_string();
    match name {
        "libx264" => vec![vec![
            s("preset", "ultrafast"),
            s("tune", "zerolatency"),
            s("crf", &qp),
            s("forced-idr", "1"),
            s(
                "x264-params",
                "repeat-headers=1:scenecut=0:rc-lookahead=0:sliced-threads=1",
            ),
        ]],
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => vec![vec![
            s("preset", "p1"),
            s("tune", "ull"),
            s("zerolatency", "1"),
            s("delay", "0"),
            s("bf", "0"),
            s("rc", "constqp"),
            s("qp", &qp),
            s("forced-idr", "1"),
        ]],
        // The same low-power first order as the normal sets (see `normal_option_sets`).
        "h264_vaapi" => [true, false]
            .into_iter()
            .map(|low_power| {
                let mut v = vec![s("rc_mode", "CQP"), s("qp", &qp)];
                if low_power {
                    v.push(s("low_power", "1"));
                }
                v.push(s("async_depth", "1"));
                v.push(s("level", "5.2"));
                v
            })
            .collect(),
        "hevc_vaapi" | "av1_vaapi" => [true, false]
            .into_iter()
            .map(|low_power| {
                let mut v = vec![s("rc_mode", "CQP"), s("qp", &qp)];
                if low_power {
                    v.push(s("low_power", "1"));
                }
                v.push(s("async_depth", "1"));
                v
            })
            .collect(),
        // Fixed quantiser only: the sets without `rc` would run the driver's default rate control.
        "h264_amf" | "hevc_amf" | "av1_amf" => {
            let sets: [&[(&'static str, &str)]; 3] = [
                &[("usage", "ultralowlatency"), ("quality", "speed")],
                &[("usage", "lowlatency"), ("quality", "speed")],
                &[("quality", "speed")],
            ];
            sets.iter()
                .map(|set| {
                    let mut v: Vec<_> = set.iter().map(|&(k, val)| s(k, val)).collect();
                    v.push(s("rc", "cqp"));
                    v.push(s("qp_i", &qp));
                    v.push(s("qp_p", &qp));
                    v.push(s("forced_idr", "1"));
                    v.push(s("async_depth", "1"));
                    v
                })
                .collect()
        }
        _ => normal_option_sets(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: QualityLevel = QualityLevel::Normal;

    #[test]
    fn every_known_encoder_has_a_preset() {
        for n in [
            "libx264",
            "h264_nvenc",
            "h264_vaapi",
            "h264_qsv",
            "h264_amf",
            "libopenh264",
        ] {
            let sets = option_sets(n, N);
            assert!(!sets.is_empty() && !sets[0].is_empty(), "{n}");
        }
        let has = |n: &str, kv: (&'static str, &str)| {
            option_sets(n, N)
                .iter()
                .any(|set| set.contains(&(kv.0, kv.1.to_string())))
        };
        for n in ["h264_nvenc", "hevc_nvenc", "av1_nvenc"] {
            assert!(has(n, ("rc", "vbr")) && has(n, ("cq", "19")), "{n}");
            assert!(!has(n, ("rc", "cbr")), "{n}");
        }
        assert!(has("libx264", ("crf", "20")));
        for n in ["h264_vaapi", "hevc_vaapi", "av1_vaapi"] {
            assert!(
                has(n, ("rc_mode", "VBR")) && !has(n, ("rc_mode", "CBR")),
                "{n}"
            );
        }
        assert!(has("h264_vaapi", ("qp", "20")) && !has("h264_vaapi", ("qp", "24")));
        assert!(has("h264_amf", ("rc", "vbr_peak")) && !has("h264_amf", ("rc", "cbr")));
        assert_eq!(option_sets("unknown_enc", N), vec![vec![]]);
        assert_eq!(option_sets("h264_vaapi", N).len(), 3);
        assert_eq!(option_sets("hevc_vaapi", N).len(), 2);
        assert_eq!(option_sets("h264_amf", N).len(), 6);
        assert!(
            option_sets("h264_amf", N)
                .iter()
                .all(|set| set.contains(&("async_depth", "1".to_string())))
        );
    }

    #[test]
    fn ten_bit_sets_select_main10() {
        let main10 = ("profile", "main10".to_string());
        for n in ["hevc_nvenc", "hevc_qsv", "hevc_amf", "hevc_vaapi"] {
            let plain = option_sets_for(n, false, N);
            assert_eq!(plain, option_sets(n, N), "{n}");
            assert!(plain.iter().all(|set| !set.contains(&main10)), "{n}");
            let ten = option_sets_for(n, true, N);
            assert!(
                !ten.is_empty() && ten.iter().all(|set| set.contains(&main10)),
                "{n}"
            );
        }
        // Counts: AMF and QSV keep their fallback chains; VAAPI drops low_power.
        assert_eq!(option_sets_for("hevc_amf", true, N).len(), 6);
        assert_eq!(option_sets_for("hevc_qsv", true, N).len(), 2);
        assert_eq!(option_sets_for("hevc_vaapi", true, N).len(), 1);
        assert!(
            option_sets_for("hevc_vaapi", true, N)
                .iter()
                .flatten()
                .all(|(k, _)| *k != "low_power")
        );
        // Unrelated encoders ignore the flag.
        assert_eq!(
            option_sets_for("h264_nvenc", true, N),
            option_sets("h264_nvenc", N)
        );
        assert_eq!(
            option_sets_for("av1_vaapi", true, N),
            option_sets("av1_vaapi", N)
        );
    }

    fn has(sets: &[Vec<(&'static str, String)>], k: &str, v: &str) -> bool {
        sets.iter()
            .all(|set| set.contains(&(k_static(k), v.to_string())))
    }

    fn k_static(k: &str) -> &'static str {
        [
            "rc",
            "qp",
            "tune",
            "crf",
            "rc_mode",
            "qp_i",
            "qp_p",
            "low_power",
        ]
        .into_iter()
        .find(|c| *c == k)
        .unwrap()
    }

    #[test]
    fn high_sets_use_a_fixed_quantiser() {
        let h = QualityLevel::High;
        for n in ["h264_nvenc", "hevc_nvenc", "av1_nvenc"] {
            let sets = option_sets(n, h);
            assert_eq!(sets.len(), 1, "{n}");
            assert!(has(&sets, "rc", "constqp") && has(&sets, "qp", "10"), "{n}");
            assert!(has(&sets, "tune", "ull"), "{n}");
            assert!(
                sets[0].contains(&("zerolatency", "1".into()))
                    && sets[0].contains(&("forced-idr", "1".into()))
            );
        }
        assert!(has(&option_sets("libx264", h), "crf", "10"));
        for n in ["h264_vaapi", "hevc_vaapi", "av1_vaapi"] {
            let sets = option_sets(n, h);
            assert_eq!(sets.len(), 2, "{n}");
            assert!(
                has(&sets, "rc_mode", "CQP") && has(&sets, "qp", "10"),
                "{n}"
            );
            assert!(sets[0].contains(&("low_power", "1".into())), "{n}");
            assert!(!sets[1].iter().any(|(k, _)| *k == "low_power"), "{n}");
        }
        let amf = option_sets("h264_amf", h);
        assert!(has(&amf, "rc", "cqp") && has(&amf, "qp_i", "10") && has(&amf, "qp_p", "10"));
        assert!(amf.iter().all(|s| s.contains(&("async_depth", "1".into()))));
        // QSV keeps its normal sets.
        assert_eq!(option_sets("h264_qsv", h), option_sets("h264_qsv", N));
        // Ten-bit still selects Main10 for the fixed-quantiser sets.
        let ten = option_sets_for("hevc_vaapi", true, h);
        assert_eq!(ten.len(), 1);
        assert!(ten[0].contains(&("profile", "main10".into())));
    }

    #[test]
    fn lossless_is_an_hevc_nvenc_tune_with_high_fallback() {
        let l = QualityLevel::Lossless;
        let sets = option_sets("hevc_nvenc", l);
        assert_eq!(sets.len(), 2);
        assert!(sets[0].contains(&("tune", "lossless".into())));
        assert!(
            !sets[0]
                .iter()
                .any(|(k, _)| matches!(*k, "rc" | "qp" | "cq"))
        );
        for key in ["preset", "zerolatency", "delay", "bf", "forced-idr"] {
            assert!(sets[0].iter().any(|(k, _)| *k == key), "{key}");
        }
        assert_eq!(sets[1..], option_sets("hevc_nvenc", QualityLevel::High)[..]);
        // Other encoders (including H.264 NVENC) have no lossless mode: the high sets.
        for n in [
            "h264_nvenc",
            "hevc_vaapi",
            "libx264",
            "hevc_amf",
            "hevc_qsv",
        ] {
            assert_eq!(option_sets(n, l), option_sets(n, QualityLevel::High), "{n}");
        }
    }
}
