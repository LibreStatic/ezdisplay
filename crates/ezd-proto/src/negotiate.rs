//! Capability negotiation: a pure function from caps + preferences to a [`StreamConfig`].

use crate::msg::{ClientCaps, ClientHdr, Codec, DecoderCaps, EncoderCaps, HostCaps, StreamConfig};
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

/// User preference for HDR10 streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HdrMode {
    /// Stream HDR10 whenever the source, the client and the host encoder all support it.
    #[default]
    Auto,
    /// Never stream HDR10; an HDR source is tone-mapped to SDR.
    Off,
}

/// Why an HDR source is streamed as tone-mapped SDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdrReason {
    /// The user set [`HdrMode::Off`].
    TurnedOff,
    /// The client speaks a protocol older than 0.5 (sent no `ClientHdr`).
    OldClient,
    /// The client display does not show HDR10.
    DisplayNotHdr,
    /// The client has no 10-bit HDR10 decoder (for a codec the host can also encode).
    NoMain10Decoder,
    /// The host has no 10-bit encoder (for a codec the client can also decode).
    NoMain10Encoder,
}

/// Outcome of [`pick_dynamic_range`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicRange {
    /// Stream HDR10 (BT.2020 PQ, 10-bit) with this codec.
    Hdr10 {
        /// Codec to use; overrides the normal codec choice.
        codec: Codec,
    },
    /// The source is HDR but the stream is SDR (tone-mapped on the host) for this reason.
    SdrToneMapped(SdrReason),
    /// The source is SDR.
    Sdr,
}

impl DynamicRange {
    /// True for [`DynamicRange::Hdr10`].
    pub fn is_hdr10(&self) -> bool {
        matches!(self, DynamicRange::Hdr10 { .. })
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
    /// HDR10 streaming policy, used by [`negotiate_hdr`]. Default [`HdrMode::Auto`].
    pub hdr: HdrMode,
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
            hdr: HdrMode::Auto,
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

/// Decides between HDR10 and SDR for a stream. Pure.
///
/// * `source_hdr`: the captured content is HDR; otherwise the result is [`DynamicRange::Sdr`];
/// * `prefs_hdr`: [`HdrMode::Off`] gives `SdrToneMapped(TurnedOff)`;
/// * `client`: the client's `ClientHdr`, `None` for a client older than protocol 0.5;
/// * `host_main10`: codecs the host can encode in 10 bits;
/// * `client_decoders`: the client's decoders; a codec in `ClientHdr::main10` only counts when
///   a decoder for it is listed.
///
/// HDR10 picks HEVC when both sides have it, else AV1.
pub fn pick_dynamic_range(
    source_hdr: bool,
    prefs_hdr: HdrMode,
    client: Option<&ClientHdr>,
    host_main10: &[Codec],
    client_decoders: &[DecoderCaps],
) -> DynamicRange {
    if !source_hdr {
        return DynamicRange::Sdr;
    }
    if prefs_hdr == HdrMode::Off {
        return DynamicRange::SdrToneMapped(SdrReason::TurnedOff);
    }
    let Some(client) = client else {
        return DynamicRange::SdrToneMapped(SdrReason::OldClient);
    };
    if !client.display_hdr10 {
        return DynamicRange::SdrToneMapped(SdrReason::DisplayNotHdr);
    }
    let client_codecs: Vec<Codec> = client
        .main10_codecs()
        .into_iter()
        .filter(|c| client_decoders.iter().any(|d| d.codec == *c))
        .collect();
    if client_codecs.is_empty() {
        return DynamicRange::SdrToneMapped(SdrReason::NoMain10Decoder);
    }
    [Codec::Hevc, Codec::Av1]
        .into_iter()
        .find(|c| client_codecs.contains(c) && host_main10.contains(c))
        .map_or(
            DynamicRange::SdrToneMapped(SdrReason::NoMain10Encoder),
            |codec| DynamicRange::Hdr10 { codec },
        )
}

/// Like [`negotiate`] for an HDR-aware host: also decides the dynamic range with
/// [`pick_dynamic_range`]. When it is [`DynamicRange::Hdr10`] the codec is forced to the HDR10
/// codec (`StreamConfig::codec` matches it). A `prefs.encoder` override only counts as a 10-bit
/// encoder when it matches the codec's encoder.
pub fn negotiate_hdr(
    client: &ClientCaps,
    host: &HostCaps,
    prefs: &Prefs,
    source_hdr: bool,
    client_hdr: Option<&ClientHdr>,
    host_main10: &[Codec],
) -> Result<(StreamConfig, DynamicRange)> {
    let usable: Vec<Codec> = host_main10
        .iter()
        .copied()
        .filter(|&c| pick_encoder(host, c, prefs).is_some())
        .collect();
    let range = pick_dynamic_range(source_hdr, prefs.hdr, client_hdr, &usable, &client.decoders);
    if let DynamicRange::Hdr10 { codec } = range {
        let forced = Prefs {
            codec: Some(codec),
            ..prefs.clone()
        };
        let cfg = negotiate(client, host, &forced)?;
        debug_assert_eq!(cfg.codec, codec);
        return Ok((cfg, range));
    }
    Ok((negotiate(client, host, prefs)?, range))
}

/// Bitrate in kbit/s derived from the stream size, rate and codec (before the link ceiling).
pub fn auto_bitrate_kbps(width: u32, height: u32, refresh_mhz: u32, codec: Codec) -> u32 {
    let fps = refresh_mhz as f64 / 1000.0;
    (width as f64 * height as f64 * fps * bits_per_pixel(codec) / 1000.0).round() as u32
}

/// Bitrate ceiling in kbit/s that `prefs` impose on the negotiated bitrate.
pub fn bitrate_ceiling_kbps(prefs: &Prefs) -> u32 {
    prefs
        .max_bitrate_kbps
        .unwrap_or_else(|| prefs.link.default_max_bitrate_kbps())
        .max(MIN_BITRATE_KBPS)
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
    let ceiling = bitrate_ceiling_kbps(prefs);
    let wanted = prefs
        .bitrate_kbps
        .unwrap_or_else(|| auto_bitrate_kbps(width, height, refresh_mhz, codec));
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
    fn dynamic_range_table() {
        let hdr = |display: bool, codecs: &[Codec]| ClientHdr::new(display, 1000, 600, 50, codecs);
        let decs = vec![
            dec(Codec::H264, "d", true, true),
            dec(Codec::Hevc, "dh", true, true),
            dec(Codec::Av1, "da", true, true),
        ];
        let good = hdr(true, &[Codec::Hevc, Codec::Av1]);
        let ten = [Codec::Hevc, Codec::Av1];
        use DynamicRange::*;
        struct Row<'a> {
            name: &'static str,
            source: bool,
            mode: HdrMode,
            client: Option<&'a ClientHdr>,
            host: &'a [Codec],
            decoders: &'a [DecoderCaps],
            want: DynamicRange,
        }
        let no_main10 = hdr(true, &[]);
        let not_hdr = hdr(false, &[Codec::Hevc]);
        let av1_only = hdr(true, &[Codec::Av1]);
        let rows = [
            Row {
                name: "sdr source",
                source: false,
                mode: HdrMode::Auto,
                client: Some(&good),
                host: &ten,
                decoders: &decs,
                want: Sdr,
            },
            Row {
                name: "sdr source ignores everything",
                source: false,
                mode: HdrMode::Off,
                client: None,
                host: &[],
                decoders: &[],
                want: Sdr,
            },
            Row {
                name: "turned off",
                source: true,
                mode: HdrMode::Off,
                client: Some(&good),
                host: &ten,
                decoders: &decs,
                want: SdrToneMapped(SdrReason::TurnedOff),
            },
            Row {
                name: "old client",
                source: true,
                mode: HdrMode::Auto,
                client: None,
                host: &ten,
                decoders: &decs,
                want: SdrToneMapped(SdrReason::OldClient),
            },
            Row {
                name: "display not hdr",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&not_hdr),
                host: &ten,
                decoders: &decs,
                want: SdrToneMapped(SdrReason::DisplayNotHdr),
            },
            Row {
                name: "no main10 decoder",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&no_main10),
                host: &ten,
                decoders: &decs,
                want: SdrToneMapped(SdrReason::NoMain10Decoder),
            },
            Row {
                name: "main10 claimed without a decoder entry",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&good),
                host: &ten,
                decoders: &decs[..1],
                want: SdrToneMapped(SdrReason::NoMain10Decoder),
            },
            Row {
                name: "no main10 encoder",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&good),
                host: &[],
                decoders: &decs,
                want: SdrToneMapped(SdrReason::NoMain10Encoder),
            },
            Row {
                name: "no common main10 codec",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&hevc_only_hdr()),
                host: &[Codec::Av1],
                decoders: &decs,
                want: SdrToneMapped(SdrReason::NoMain10Encoder),
            },
            Row {
                name: "hevc preferred",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&good),
                host: &ten,
                decoders: &decs,
                want: Hdr10 { codec: Codec::Hevc },
            },
            Row {
                name: "av1 when no hevc on the host",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&good),
                host: &[Codec::Av1],
                decoders: &decs,
                want: Hdr10 { codec: Codec::Av1 },
            },
            Row {
                name: "av1 when no hevc on the client",
                source: true,
                mode: HdrMode::Auto,
                client: Some(&av1_only),
                host: &ten,
                decoders: &decs,
                want: Hdr10 { codec: Codec::Av1 },
            },
        ];
        for r in rows {
            let got = pick_dynamic_range(r.source, r.mode, r.client, r.host, r.decoders);
            assert_eq!(got, r.want, "{}", r.name);
        }
    }

    fn hevc_only_hdr() -> ClientHdr {
        ClientHdr::new(true, 1000, 600, 50, &[Codec::Hevc])
    }

    #[test]
    fn negotiate_hdr_forces_the_hdr_codec() {
        let c = client(
            1920,
            1080,
            &[60000],
            vec![
                dec(Codec::H264, "d", true, true),
                dec(Codec::Hevc, "dh", true, true),
            ],
        );
        let h = host(vec![
            enc(Codec::H264, "h264_nvenc", true),
            enc(Codec::Hevc, "hevc_nvenc", true),
        ]);
        let ch = ClientHdr::new(true, 1000, 600, 50, &[Codec::Hevc]);
        let (cfg, range) =
            negotiate_hdr(&c, &h, &Prefs::default(), true, Some(&ch), &[Codec::Hevc]).unwrap();
        assert_eq!(range, DynamicRange::Hdr10 { codec: Codec::Hevc });
        assert_eq!(
            (cfg.codec, cfg.encoder_name.as_str()),
            (Codec::Hevc, "hevc_nvenc")
        );

        // Forcing an unrelated encoder removes the 10-bit encoder.
        let forced = Prefs {
            encoder: Some("h264_nvenc".into()),
            ..Prefs::default()
        };
        let (cfg, range) = negotiate_hdr(&c, &h, &forced, true, Some(&ch), &[Codec::Hevc]).unwrap();
        assert_eq!(
            range,
            DynamicRange::SdrToneMapped(SdrReason::NoMain10Encoder)
        );
        assert_eq!(cfg.codec, Codec::H264);

        // SDR path is exactly `negotiate`.
        let (cfg, range) = negotiate_hdr(&c, &h, &Prefs::default(), false, None, &[]).unwrap();
        assert_eq!(range, DynamicRange::Sdr);
        assert_eq!(cfg, negotiate(&c, &h, &Prefs::default()).unwrap());
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
