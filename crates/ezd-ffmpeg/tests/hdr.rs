//! HDR10 (P010, HEVC Main10, BT.2020 PQ) encode round trip with whatever 10-bit HEVC encoder
//! this machine has (hevc_vaapi on the Intel iGPU, or hevc_nvenc); skipped with a message
//! when there is none.

use ezd_ffmpeg::sys as ffi;
use ezd_ffmpeg::*;
use ezd_proto::{Mastering, StreamColor};
use std::ptr;

const W: u32 = 640;
const H: u32 = 360;
const FRAMES: i64 = 6;

fn mastering() -> Mastering {
    Mastering {
        primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
        white: [15635, 16450],
        max_luminance: 10_000_000,
        min_luminance: 50,
    }
}

/// A P010 frame: a horizontal luma ramp in the top 10 bits of each 16-bit word, neutral chroma.
fn p010_frame(n: i64) -> Vec<u8> {
    let (w, h) = (W as usize, H as usize);
    let mut out = Vec::with_capacity(w * h * 3);
    for _y in 0..h {
        for x in 0..w {
            let v = 64 + ((x as i64 + n * 8) % 876) as u16; // limited range 64..940
            out.extend_from_slice(&(v << 6).to_le_bytes());
        }
    }
    for _ in 0..h / 2 * w / 2 {
        out.extend_from_slice(&(512u16 << 6).to_le_bytes());
        out.extend_from_slice(&(512u16 << 6).to_le_bytes());
    }
    out
}

fn open_encoder(color: &StreamColor) -> Option<Encoder> {
    let candidates: [(&str, Option<(HwKind, &str)>); 2] = [
        ("hevc_vaapi", Some((HwKind::Vaapi, "/dev/dri/renderD129"))),
        ("hevc_nvenc", None),
    ];
    for (name, hw) in candidates {
        let hw = match hw {
            Some((kind, dev)) => match HwDevice::open(kind, Some(dev)) {
                Ok(d) => Some(d),
                Err(e) => {
                    eprintln!("{name}: device {dev}: {e}");
                    continue;
                }
            },
            None => None,
        };
        let mut cfg = EncoderConfig::new(name, W, H, 60_000, 8_000);
        cfg.pix_fmt = RawPixelFormat::P010;
        cfg.hw = hw;
        cfg.color = ColorInfo::from(color);
        cfg.hdr = HdrMeta::from_stream_color(color);
        match Encoder::open(cfg) {
            Ok(e) => return Some(e),
            Err(e) => eprintln!("{name}: cannot open for P010: {e}"),
        }
    }
    None
}

struct Seen {
    format: i32,
    primaries: ffi::AVColorPrimaries,
    trc: ffi::AVColorTransferCharacteristic,
    matrix: ffi::AVColorSpace,
    range: ffi::AVColorRange,
    mastering: Option<[i32; 4]>,
    cll: Option<(u32, u32)>,
}

/// Decodes Annex-B access units with the raw API and reports what the first picture signals.
fn decode_first(units: &[Vec<u8>]) -> Seen {
    // SAFETY: plain FFmpeg decode loop on locally owned contexts, freed before returning.
    unsafe {
        let codec = ffi::avcodec_find_decoder(ffi::AVCodecID::AV_CODEC_ID_HEVC);
        assert!(!codec.is_null());
        let mut ctx = ffi::avcodec_alloc_context3(codec);
        assert_eq!(ffi::avcodec_open2(ctx, codec, ptr::null_mut()), 0);
        let mut pkt = ffi::av_packet_alloc();
        let mut frame = ffi::av_frame_alloc();
        let mut seen = None;
        for u in units {
            assert_eq!(ffi::av_new_packet(pkt, u.len() as i32), 0);
            ptr::copy_nonoverlapping(u.as_ptr(), (*pkt).data, u.len());
            assert_eq!(ffi::avcodec_send_packet(ctx, pkt), 0);
            ffi::av_packet_unref(pkt);
            if ffi::avcodec_receive_frame(ctx, frame) == 0 {
                let f = &*frame;
                let sd = |t| ffi::av_frame_get_side_data(frame, t);
                let m = sd(ffi::AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA);
                let l = sd(ffi::AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL);
                seen = Some(Seen {
                    format: f.format,
                    primaries: f.color_primaries,
                    trc: f.color_trc,
                    matrix: f.colorspace,
                    range: f.color_range,
                    // AVMasteringDisplayMetadata after 12 primaries and 4 white point ints:
                    // min_luminance (num, den) then max_luminance (num, den).
                    mastering: (!m.is_null()).then(|| {
                        let d = (*m).data as *const i32;
                        [*d.add(16), *d.add(17), *d.add(18), *d.add(19)]
                    }),
                    cll: (!l.is_null()).then(|| {
                        let d = (*l).data as *const u32;
                        (*d, *d.add(1))
                    }),
                });
                break;
            }
        }
        ffi::av_frame_free(&mut frame);
        ffi::av_packet_free(&mut pkt);
        ffi::avcodec_free_context(&mut ctx);
        seen.expect("the decoder produced no picture")
    }
}

#[test]
fn hdr10_p010_stream_is_signalled_as_bt2020_pq() {
    let color = StreamColor::hdr10(Some(mastering()), 1000, 400);
    let Some(mut enc) = open_encoder(&color) else {
        eprintln!("no 10-bit HEVC encoder available here, skipping");
        return;
    };
    eprintln!("encoding with {}", enc.name());
    let mut units = vec![enc.extradata().to_vec()];
    for n in 0..FRAMES {
        enc.send_frame(&p010_frame(n), n == 0).unwrap();
        while let Some(p) = enc.receive_packet().unwrap() {
            units.push(p.data);
        }
    }
    for p in enc.flush().unwrap() {
        units.push(p.data);
    }
    assert!(units.len() > 2, "the encoder produced no packets");
    let name = enc.name().to_string();
    let seen = decode_first(&units);

    assert_eq!(
        seen.format,
        ffi::AVPixelFormat::AV_PIX_FMT_YUV420P10LE as i32,
        "Main10 stream"
    );
    assert_eq!(seen.primaries, ffi::AVColorPrimaries::AVCOL_PRI_BT2020);
    assert_eq!(
        seen.trc,
        ffi::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084
    );
    assert_eq!(seen.matrix, ffi::AVColorSpace::AVCOL_SPC_BT2020_NCL);
    assert_eq!(seen.range, ffi::AVColorRange::AVCOL_RANGE_MPEG);
    eprintln!("{name}: mastering {:?}, cll {:?}", seen.mastering, seen.cll);
    // The static metadata is an SEI that only some encoders write; check it where present.
    if let Some(max) = seen.mastering {
        assert_eq!(max, [50, 10_000, 10_000_000, 10_000]);
    }
    if let Some(cll) = seen.cll {
        assert_eq!(cll, (1000, 400));
    }
}
