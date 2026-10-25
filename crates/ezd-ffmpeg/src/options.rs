//! Low-latency option presets per encoder name.

/// Ordered fallback chain of AVOption sets for `name`: the first set that
/// opens successfully is used (e.g. VAAPI with `low_power` first, then without).
pub fn option_sets(name: &str) -> Vec<Vec<(&'static str, String)>> {
    let s = |k: &'static str, v: &str| (k, v.to_string());
    match name {
        "libx264" => vec![vec![
            s("preset", "ultrafast"),
            s("tune", "zerolatency"),
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
            s("rc", "cbr"),
            s("forced-idr", "1"),
        ]],
        // Level 5.2: FFmpeg picks level 6.x above 5.2's macroblock rate (2960x1848 at 120 Hz),
        // and Intel's iHD driver then fails every CBR frame ("internal encoding error") on
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
                s("rc_mode", "CBR"),
                s("low_power", "1"),
                s("async_depth", "1"),
                s("level", "5.2"),
            ],
            vec![
                s("rc_mode", "CQP"),
                s("qp", "24"),
                s("low_power", "1"),
                s("async_depth", "1"),
                s("level", "5.2"),
            ],
            vec![
                s("rc_mode", "CBR"),
                s("async_depth", "1"),
                s("level", "5.2"),
            ],
        ],
        "hevc_vaapi" | "av1_vaapi" => vec![
            vec![
                s("rc_mode", "CBR"),
                s("low_power", "1"),
                s("async_depth", "1"),
            ],
            vec![s("rc_mode", "CBR"), s("async_depth", "1")],
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
                    ("rc", "cbr"),
                    ("quality", "speed"),
                ],
                &[("usage", "lowlatency"), ("rc", "cbr"), ("quality", "speed")],
                &[("usage", "lowlatency"), ("quality", "speed")],
                &[("rc", "cbr"), ("quality", "speed")],
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

#[cfg(test)]
mod tests {
    use super::*;

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
            let sets = option_sets(n);
            assert!(!sets.is_empty() && !sets[0].is_empty(), "{n}");
        }
        assert_eq!(option_sets("unknown_enc"), vec![vec![]]);
        assert_eq!(option_sets("h264_vaapi").len(), 3);
        assert_eq!(option_sets("hevc_vaapi").len(), 2);
        assert_eq!(option_sets("h264_amf").len(), 6);
        assert!(
            option_sets("h264_amf")
                .iter()
                .all(|set| set.contains(&("async_depth", "1".to_string())))
        );
    }
}
