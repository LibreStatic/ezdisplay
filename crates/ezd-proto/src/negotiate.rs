//! Capability negotiation: a pure function from caps + preferences to a [`StreamConfig`].

use crate::msg::{ClientCaps, Codec, DecoderCaps, EncoderCaps, HostCaps, StreamConfig};
use crate::{ProtoError, Result};

/// Kind of transport carrying the link; selects the default bitrate ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// USB (ADB reverse / AOA): up to 80 Mbit/s by default.
    Usb,
    /// Wi-Fi / Ethernet: up to 30 Mbit/s by default.
    Network,
}

impl LinkKind {
    /// Default maximum bitrate in kbit/s for this link.
    pub fn default_max_bitrate_kbps(self) -> u32 {
        match self {
            LinkKind::Usb => 80_000,
            LinkKind::Network => 30_000,
        }
    }
}

/// User/host preferences fed into [`negotiate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    /// Preferred codec; honoured only if both sides support it, else H.264 is used.
    pub codec: Option<Codec>,
    /// Never pick a refresh rate above this (millihertz). Default 120000.
    pub max_refresh_mhz: u32,
    /// Transport kind.
    pub link: LinkKind,
    /// Bitrate ceiling in kbit/s; `None` uses [`LinkKind::default_max_bitrate_kbps`].
    pub max_bitrate_kbps: Option<u32>,
    /// Explicit bitrate (still clamped to the ceiling); `None` derives it from resolution and fps.
    pub bitrate_kbps: Option<u32>,
    /// Force a specific host encoder by name (must support the chosen codec).
    pub encoder: Option<String>,
    /// Stream size as a fraction of the client's native size, in thousandths (250-1000).
    /// Default 1000. Values outside the range are clamped.
    pub scale_permille: u16,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            codec: None,
            max_refresh_mhz: 120_000,
            link: LinkKind::Usb,
            max_bitrate_kbps: None,
            bitrate_kbps: None,
            encoder: None,
            scale_permille: 1000,
        }
    }
}

const MIN_BITRATE_KBPS: u32 = 500;
const FALLBACK_REFRESH_MHZ: u32 = 60_000;

fn bits_per_pixel(codec: Codec) -> f64 {
    match codec {
        Codec::H264 => 0.07,
        Codec::Hevc | Codec::Av1 => 0.05,
    }
}

fn pick_codec(client: &ClientCaps, host: &HostCaps, prefs: &Prefs) -> Result<Codec> {
    let ok = |c: Codec| {
        client.decoders.iter().any(|d| d.codec == c)
            && host
                .encoders
                .iter()
                .any(|e| e.codec == c && prefs.encoder.as_deref().is_none_or(|n| n == e.name))
    };
    // Preference first (if satisfiable), then H.264, then whatever else both sides share.
    [
        prefs.codec,
        Some(Codec::H264),
        Some(Codec::Hevc),
        Some(Codec::Av1),
    ]
    .into_iter()
    .flatten()
    .find(|&c| ok(c))
    .ok_or(ProtoError::NoCommonCodec)
}

/// Best decoder for `codec`: hardware+low-latency, then low-latency, then hardware, then first.
fn pick_decoder(client: &ClientCaps, codec: Codec) -> Option<&DecoderCaps> {
    let score = |d: &DecoderCaps| (d.hardware && d.low_latency, d.low_latency, d.hardware);
    let mut best: Option<&DecoderCaps> = None;
    for d in client.decoders.iter().filter(|d| d.codec == codec) {
        if best.is_none_or(|b| score(d) > score(b)) {
            best = Some(d);
        }
    }
    best
}

fn even_floor(v: u64) -> u32 {
    ((v.max(2)) & !1) as u32
}

/// Native size scaled by `prefs.scale_permille`, clamped to the decoder limits, aspect
/// preserved, rounded down to even.
fn pick_resolution(w: u32, h: u32, dec: &DecoderCaps, prefs: &Prefs) -> (u32, u32) {
    let s = prefs.scale_permille.clamp(250, 1000) as u64;
    let (mut w, mut h) = (w as u64 * s / 1000, h as u64 * s / 1000);
    let (mw, mh) = (dec.max_width as u64, dec.max_height as u64);
    // Scale down by the tighter of the two limits (cross-multiplied to stay in integers).
    if mw != 0 && w > mw {
        h = h * mw / w;
        w = mw;
    }
    if mh != 0 && h > mh {
        w = w * mh / h;
        h = mh;
    }
    (even_floor(w), even_floor(h))
}

fn pick_refresh(client: &ClientCaps, dec: &DecoderCaps, prefs: &Prefs) -> u32 {
    let mut cap = prefs.max_refresh_mhz;
    if dec.max_fps != 0 {
        cap = cap.min(dec.max_fps.saturating_mul(1000));
    }
    let mut rates: Vec<u32> = client
        .display
        .refresh_rates_mhz
        .iter()
        .copied()
        .filter(|&r| r > 0)
        .collect();
    if rates.is_empty() {
        let cur = client.display.current_refresh_mhz;
        rates.push(if cur > 0 { cur } else { FALLBACK_REFRESH_MHZ });
    }
    rates
        .iter()
        .copied()
        .filter(|&r| r <= cap)
        .max()
        // Every supported rate is above the cap: take the lowest one available.
        .unwrap_or_else(|| rates.iter().copied().min().unwrap_or(FALLBACK_REFRESH_MHZ))
}

fn pick_encoder<'a>(host: &'a HostCaps, codec: Codec, prefs: &Prefs) -> Option<&'a EncoderCaps> {
    let mut cands = host
        .encoders
        .iter()
        .filter(|e| e.codec == codec && prefs.encoder.as_deref().is_none_or(|n| n == e.name));
    if prefs.encoder.is_some() {
        return cands.next();
    }
    let all: Vec<_> = cands.collect();
    all.iter()
        .find(|e| e.hardware)
        .or_else(|| all.first())
        .copied()
}

/// Computes the stream configuration for a client/host pair.
///
/// * codec: `prefs.codec` if both sides support it, else H.264 (then HEVC, AV1);
/// * decoder: hardware + low-latency preferred; its limits clamp resolution and fps;
/// * resolution: client native times `prefs.scale_permille`, clamped to the decoder maximum,
///   aspect kept, even;
/// * refresh: highest client rate not above `prefs.max_refresh_mhz` (and decoder fps);
/// * bitrate: `w*h*fps*0.07` bit/s for H.264 (`0.05` HEVC/AV1) unless `prefs.bitrate_kbps`,
///   clamped to the link ceiling;
/// * encoder: first hardware encoder for the codec, else the first software one.
pub fn negotiate(client: &ClientCaps, host: &HostCaps, prefs: &Prefs) -> Result<StreamConfig> {
    if client.display.width_px == 0 || client.display.height_px == 0 {
        return Err(ProtoError::InvalidCaps("client display has zero size"));
    }
    let codec = pick_codec(client, host, prefs)?;
    let dec = pick_decoder(client, codec).ok_or(ProtoError::NoCommonCodec)?;
    let enc = pick_encoder(host, codec, prefs).ok_or(ProtoError::NoCommonCodec)?;
    let (width, height) = pick_resolution(
        client.display.width_px,
        client.display.height_px,
        dec,
        prefs,
    );
    let refresh_mhz = pick_refresh(client, dec, prefs);
    let fps = refresh_mhz as f64 / 1000.0;
    let ceiling = prefs
        .max_bitrate_kbps
        .unwrap_or_else(|| prefs.link.default_max_bitrate_kbps())
        .max(MIN_BITRATE_KBPS);
    let wanted = prefs.bitrate_kbps.unwrap_or_else(|| {
        (width as f64 * height as f64 * fps * bits_per_pixel(codec) / 1000.0).round() as u32
    });
    Ok(StreamConfig {
        width,
        height,
        refresh_mhz,
        codec,
        bitrate_kbps: wanted.clamp(MIN_BITRATE_KBPS, ceiling),
        encoder_name: enc.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::DisplayInfo;

    fn dec(codec: Codec, name: &str, hw: bool, ll: bool) -> DecoderCaps {
        DecoderCaps {
            codec,
            name: name.into(),
            hardware: hw,
            low_latency: ll,
            max_width: 0,
            max_height: 0,
            max_fps: 0,
        }
    }
    fn enc(codec: Codec, name: &str, hw: bool) -> EncoderCaps {
        EncoderCaps {
            codec,
            name: name.into(),
            hardware: hw,
        }
    }
    fn client(w: u32, h: u32, rates: &[u32], decoders: Vec<DecoderCaps>) -> ClientCaps {
        ClientCaps {
            display: DisplayInfo {
                width_px: w,
                height_px: h,
                density_dpi: 240,
                refresh_rates_mhz: rates.to_vec(),
                current_refresh_mhz: rates.first().copied().unwrap_or(0),
            },
            decoders,
            stylus: false,
            max_touch_points: 10,
        }
    }
    fn host(encoders: Vec<EncoderCaps>) -> HostCaps {
        HostCaps {
            os: "t".into(),
            gpu: "t".into(),
            encoders,
        }
    }

    #[test]
    fn table() {
        struct Case {
            name: &'static str,
            client: ClientCaps,
            host: HostCaps,
            prefs: Prefs,
            want: Option<(u32, u32, u32, Codec, u32, &'static str)>,
        }
        let h264_hw = host(vec![
            enc(Codec::H264, "libx264", false),
            enc(Codec::H264, "h264_nvenc", true),
            enc(Codec::Hevc, "hevc_nvenc", true),
        ]);
        let std_dec = vec![
            dec(Codec::H264, "d", true, true),
            dec(Codec::Hevc, "dh", true, true),
        ];
        let cases = vec![
            Case {
                name: "defaults: native 1080p, 120 Hz, h264, hw encoder",
                client: client(1920, 1080, &[60000, 120000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: Some((1920, 1080, 120000, Codec::H264, 17418, "h264_nvenc")),
            },
            Case {
                name: "refresh capped by prefs",
                client: client(1920, 1080, &[60000, 90000, 120000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    max_refresh_mhz: 100000,
                    ..Prefs::default()
                },
                want: Some((1920, 1080, 90000, Codec::H264, 13064, "h264_nvenc")),
            },
            Case {
                name: "all rates above cap -> lowest",
                client: client(1920, 1080, &[90000, 120000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    max_refresh_mhz: 60000,
                    ..Prefs::default()
                },
                want: Some((1920, 1080, 90000, Codec::H264, 13064, "h264_nvenc")),
            },
            Case {
                name: "hevc preferred and supported (0.05 bpp)",
                client: client(1920, 1080, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    codec: Some(Codec::Hevc),
                    ..Prefs::default()
                },
                want: Some((1920, 1080, 60000, Codec::Hevc, 6221, "hevc_nvenc")),
            },
            Case {
                name: "hevc preferred but client lacks it -> h264",
                client: client(
                    1920,
                    1080,
                    &[60000],
                    vec![dec(Codec::H264, "d", true, true)],
                ),
                host: h264_hw.clone(),
                prefs: Prefs {
                    codec: Some(Codec::Hevc),
                    ..Prefs::default()
                },
                want: Some((1920, 1080, 60000, Codec::H264, 8709, "h264_nvenc")),
            },
            Case {
                name: "software encoder fallback",
                client: client(1280, 720, &[60000], std_dec.clone()),
                host: host(vec![enc(Codec::H264, "libx264", false)]),
                prefs: Prefs::default(),
                want: Some((1280, 720, 60000, Codec::H264, 3871, "libx264")),
            },
            Case {
                name: "decoder max resolution clamps, aspect kept, even",
                client: client(
                    2560,
                    1600,
                    &[60000],
                    vec![DecoderCaps {
                        max_width: 1920,
                        max_height: 1920,
                        ..dec(Codec::H264, "d", true, true)
                    }],
                ),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: Some((1920, 1200, 60000, Codec::H264, 9677, "h264_nvenc")),
            },
            Case {
                name: "height limit is the tighter one",
                client: client(
                    2000,
                    1000,
                    &[60000],
                    vec![DecoderCaps {
                        max_width: 4096,
                        max_height: 720,
                        ..dec(Codec::H264, "d", true, true)
                    }],
                ),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: Some((1440, 720, 60000, Codec::H264, 4355, "h264_nvenc")),
            },
            Case {
                name: "decoder max_fps caps refresh",
                client: client(
                    1920,
                    1080,
                    &[60000, 120000],
                    vec![DecoderCaps {
                        max_fps: 60,
                        ..dec(Codec::H264, "d", true, true)
                    }],
                ),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: Some((1920, 1080, 60000, Codec::H264, 8709, "h264_nvenc")),
            },
            Case {
                name: "network ceiling 30 Mbps",
                client: client(3840, 2160, &[120000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    link: LinkKind::Network,
                    ..Prefs::default()
                },
                want: Some((3840, 2160, 120000, Codec::H264, 30000, "h264_nvenc")),
            },
            Case {
                name: "usb ceiling 80 Mbps",
                client: client(3840, 2160, &[120000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    bitrate_kbps: Some(500_000),
                    ..Prefs::default()
                },
                want: Some((3840, 2160, 120000, Codec::H264, 80000, "h264_nvenc")),
            },
            Case {
                name: "explicit user bitrate",
                client: client(1920, 1080, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    bitrate_kbps: Some(12345),
                    ..Prefs::default()
                },
                want: Some((1920, 1080, 60000, Codec::H264, 12345, "h264_nvenc")),
            },
            Case {
                name: "forced encoder by name",
                client: client(1920, 1080, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    encoder: Some("libx264".into()),
                    ..Prefs::default()
                },
                want: Some((1920, 1080, 60000, Codec::H264, 8709, "libx264")),
            },
            Case {
                name: "prefers hw+low-latency decoder for limits",
                client: client(
                    1920,
                    1080,
                    &[60000],
                    vec![
                        DecoderCaps {
                            max_width: 640,
                            max_height: 480,
                            ..dec(Codec::H264, "sw", false, false)
                        },
                        dec(Codec::H264, "hw", true, true),
                    ],
                ),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: Some((1920, 1080, 60000, Codec::H264, 8709, "h264_nvenc")),
            },
            Case {
                name: "no refresh list falls back to current / 60",
                client: client(1920, 1080, &[], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: Some((1920, 1080, 60000, Codec::H264, 8709, "h264_nvenc")),
            },
            Case {
                name: "scale 750: 2220x1386, bitrate follows area",
                client: client(2960, 1848, &[60000, 120000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    scale_permille: 750,
                    ..Prefs::default()
                },
                want: Some((2220, 1386, 120000, Codec::H264, 25846, "h264_nvenc")),
            },
            Case {
                name: "scale 500: 1480x924",
                client: client(2960, 1848, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    scale_permille: 500,
                    ..Prefs::default()
                },
                want: Some((1480, 924, 60000, Codec::H264, 5744, "h264_nvenc")),
            },
            Case {
                name: "scale then decoder limit: 2220x1386 -> 1920x1198",
                client: client(
                    2960,
                    1848,
                    &[60000],
                    vec![DecoderCaps {
                        max_width: 1920,
                        max_height: 1920,
                        ..dec(Codec::H264, "d", true, true)
                    }],
                ),
                host: h264_hw.clone(),
                prefs: Prefs {
                    scale_permille: 750,
                    ..Prefs::default()
                },
                want: Some((1920, 1198, 60000, Codec::H264, 9661, "h264_nvenc")),
            },
            Case {
                name: "scaled odd size rounds down to even",
                client: client(1001, 1001, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    scale_permille: 999,
                    ..Prefs::default()
                },
                want: Some((998, 998, 60000, Codec::H264, 4183, "h264_nvenc")),
            },
            Case {
                name: "scale 0 clamps to 250",
                client: client(2960, 1848, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    scale_permille: 0,
                    ..Prefs::default()
                },
                want: Some((740, 462, 60000, Codec::H264, 1436, "h264_nvenc")),
            },
            Case {
                name: "scale 2000 clamps to 1000",
                client: client(2960, 1848, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs {
                    scale_permille: 2000,
                    ..Prefs::default()
                },
                want: Some((2960, 1848, 60000, Codec::H264, 22974, "h264_nvenc")),
            },
            Case {
                name: "no common codec",
                client: client(1920, 1080, &[60000], vec![dec(Codec::Av1, "d", true, true)]),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: None,
            },
            Case {
                name: "zero-size display",
                client: client(0, 0, &[60000], std_dec.clone()),
                host: h264_hw.clone(),
                prefs: Prefs::default(),
                want: None,
            },
        ];
        for c in cases {
            let got = negotiate(&c.client, &c.host, &c.prefs);
            match c.want {
                None => assert!(got.is_err(), "{}: expected error, got {got:?}", c.name),
                Some((w, h, r, codec, br, enc)) => {
                    let got = got.unwrap_or_else(|e| panic!("{}: {e}", c.name));
                    assert_eq!(
                        (
                            got.width,
                            got.height,
                            got.refresh_mhz,
                            got.codec,
                            got.bitrate_kbps,
                            got.encoder_name.as_str()
                        ),
                        (w, h, r, codec, br, enc),
                        "{}",
                        c.name
                    );
                }
            }
        }
    }
}
