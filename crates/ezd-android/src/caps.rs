//! Client capabilities gathered by Kotlin and passed to native as JSON.
//!
//! ```json
//! {
//!   "device_name": "SM-X710",
//!   "display": {"width_px": 2560, "height_px": 1600, "density_dpi": 274,
//!               "refresh_rates_mhz": [60000, 120000], "current_refresh_mhz": 120000},
//!   "decoders": [{"codec": "h264", "name": "c2.qti.avc.decoder", "hardware": true,
//!                 "low_latency": true, "max_width": 4096, "max_height": 4096, "max_fps": 240}],
//!   "stylus": true,
//!   "max_touch_points": 10
//! }
//! ```

use ezd_proto::{ClientCaps, Codec, DecoderCaps, DisplayInfo};
use serde::Deserialize;

/// Parsed capabilities plus the device name.
#[derive(Debug, Clone)]
pub struct ParsedCaps {
    /// `Hello.device_name`.
    pub device_name: String,
    /// Wire capabilities.
    pub caps: ClientCaps,
}

#[derive(Deserialize)]
struct JsonCaps {
    #[serde(default)]
    device_name: String,
    display: JsonDisplay,
    #[serde(default)]
    decoders: Vec<JsonDecoder>,
    #[serde(default)]
    stylus: bool,
    #[serde(default = "default_touch")]
    max_touch_points: u32,
}

fn default_touch() -> u32 {
    10
}

#[derive(Deserialize)]
struct JsonDisplay {
    width_px: u32,
    height_px: u32,
    #[serde(default = "default_dpi")]
    density_dpi: u32,
    refresh_rates_mhz: Vec<u32>,
    #[serde(default)]
    current_refresh_mhz: u32,
}

fn default_dpi() -> u32 {
    160
}

#[derive(Deserialize)]
struct JsonDecoder {
    codec: String,
    name: String,
    #[serde(default)]
    hardware: bool,
    #[serde(default)]
    low_latency: bool,
    #[serde(default)]
    max_width: u32,
    #[serde(default)]
    max_height: u32,
    #[serde(default)]
    max_fps: u32,
}

fn parse_codec(s: &str) -> Option<Codec> {
    match s.to_ascii_lowercase().as_str() {
        "h264" | "avc" | "video/avc" => Some(Codec::H264),
        "hevc" | "h265" | "video/hevc" => Some(Codec::Hevc),
        "av1" | "video/av01" => Some(Codec::Av1),
        _ => None,
    }
}

/// Parses the Kotlin-provided capabilities JSON. Decoders of unknown codecs are skipped.
pub fn parse_caps(json: &str) -> Result<ParsedCaps, String> {
    let j: JsonCaps = serde_json::from_str(json).map_err(|e| format!("bad caps json: {e}"))?;
    if j.display.width_px == 0 || j.display.height_px == 0 || j.display.refresh_rates_mhz.is_empty()
    {
        return Err("caps: display size and refresh rates are required".into());
    }
    let current = if j.display.current_refresh_mhz != 0 {
        j.display.current_refresh_mhz
    } else {
        j.display.refresh_rates_mhz[0]
    };
    let decoders = j
        .decoders
        .into_iter()
        .filter_map(|d| {
            Some(DecoderCaps {
                codec: parse_codec(&d.codec)?,
                name: d.name,
                hardware: d.hardware,
                low_latency: d.low_latency,
                max_width: d.max_width,
                max_height: d.max_height,
                max_fps: d.max_fps,
            })
        })
        .collect();
    Ok(ParsedCaps {
        device_name: if j.device_name.is_empty() {
            "android".into()
        } else {
            j.device_name
        },
        caps: ClientCaps {
            display: DisplayInfo {
                width_px: j.display.width_px,
                height_px: j.display.height_px,
                density_dpi: j.display.density_dpi,
                refresh_rates_mhz: j.display.refresh_rates_mhz,
                current_refresh_mhz: current,
            },
            decoders,
            stylus: j.stylus,
            max_touch_points: j.max_touch_points.min(255) as u8,
        },
    })
}

/// Picks the decoder to use for `codec`: hardware and low-latency first, like the host's negotiation.
/// Some vendors list a dedicated low-latency instance next to the regular one, both reporting
/// the low-latency feature (Qualcomm: `c2.qti.avc.decoder.low_latency`); that one wins the tie.
/// Among equals the first listed is kept, so the result does not depend on later entries.
pub fn choose_decoder(caps: &ClientCaps, codec: Codec) -> Option<&DecoderCaps> {
    let rank = |d: &DecoderCaps| {
        (
            d.hardware,
            d.low_latency,
            d.name.to_ascii_lowercase().contains("low_latency"),
        )
    };
    caps.decoders
        .iter()
        .filter(|d| d.codec == codec)
        .fold(None, |best: Option<&DecoderCaps>, d| match best {
            Some(b) if rank(b) >= rank(d) => Some(b),
            _ => Some(d),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{
        "device_name": "tab",
        "display": {"width_px": 2560, "height_px": 1600, "density_dpi": 274,
                    "refresh_rates_mhz": [60000, 120000], "current_refresh_mhz": 120000},
        "decoders": [
          {"codec": "h264", "name": "OMX.google.h264.decoder", "hardware": false},
          {"codec": "h264", "name": "c2.qti.avc.decoder", "hardware": true, "low_latency": true, "max_width": 4096},
          {"codec": "vp9", "name": "x"}
        ],
        "stylus": true
    }"#;

    #[test]
    fn parses_and_skips_unknown_codecs() {
        let p = parse_caps(JSON).unwrap();
        assert_eq!(p.device_name, "tab");
        assert_eq!(p.caps.decoders.len(), 2);
        assert_eq!(p.caps.max_touch_points, 10);
        assert!(p.caps.stylus);
        assert_eq!(p.caps.display.current_refresh_mhz, 120_000);
        let d = choose_decoder(&p.caps, Codec::H264).unwrap();
        assert_eq!(d.name, "c2.qti.avc.decoder");
        assert!(choose_decoder(&p.caps, Codec::Av1).is_none());
    }

    #[test]
    fn prefers_the_dedicated_low_latency_instance() {
        let caps = |names: &[&str]| {
            let decoders: Vec<String> = names
                .iter()
                .map(|n| {
                    format!(r#"{{"codec": "h264", "name": "{n}", "hardware": true, "low_latency": true}}"#)
                })
                .collect();
            let json = format!(
                r#"{{"display": {{"width_px": 2960, "height_px": 1848, "refresh_rates_mhz": [120000]}},
                    "decoders": [{}]}}"#,
                decoders.join(",")
            );
            parse_caps(&json).unwrap().caps
        };
        // Real order on a Galaxy Tab S9 Ultra, and the reverse.
        for names in [
            ["c2.qti.avc.decoder", "c2.qti.avc.decoder.low_latency"],
            ["c2.qti.avc.decoder.low_latency", "c2.qti.avc.decoder"],
        ] {
            let c = caps(&names);
            assert_eq!(
                choose_decoder(&c, Codec::H264).unwrap().name,
                "c2.qti.avc.decoder.low_latency"
            );
        }
        // Equal rank: the first listed stays.
        let c = caps(&["c2.a.avc.decoder", "c2.b.avc.decoder"]);
        assert_eq!(
            choose_decoder(&c, Codec::H264).unwrap().name,
            "c2.a.avc.decoder"
        );
    }

    #[test]
    fn rejects_missing_display() {
        assert!(parse_caps("{}").is_err());
        assert!(
            parse_caps(r#"{"display":{"width_px":0,"height_px":1,"refresh_rates_mhz":[60000]}}"#)
                .is_err()
        );
    }
}
