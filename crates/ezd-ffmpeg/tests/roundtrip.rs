//! Real libx264 encode/decode round trips (skipped with a message if libx264 is absent).

use ezd_ffmpeg::*;
use ezd_host_core::synthetic::{PixelFormat, SyntheticFrames, read_frame_number};
use ezd_host_core::{CursorSink, VideoSource};
use ezd_proto::{Codec, StreamConfig};
use std::time::Duration;

fn have_x264() -> bool {
    let ok = encoder_available("libx264");
    if !ok {
        eprintln!("libx264 not available, skipping");
    }
    ok
}

fn nal_types(annexb: &[u8]) -> Vec<u8> {
    let mut v = vec![];
    let mut i = 0;
    while i + 3 < annexb.len() {
        if annexb[i] == 0 && annexb[i + 1] == 0 && annexb[i + 2] == 1 {
            v.push(annexb[i + 3] & 0x1f);
            i += 3;
        } else {
            i += 1;
        }
    }
    v
}

#[test]
fn synthetic_numbers_survive_x264() {
    if !have_x264() {
        return;
    }
    let (w, h) = (1280, 720);
    let gen_ = SyntheticFrames::new(w, h, PixelFormat::I420).unwrap();
    let mut enc = Encoder::open(EncoderConfig::new("libx264", w, h, 60_000, 6_000)).unwrap();
    let extra = enc.extradata().to_vec();
    assert_eq!(&extra[..4], &[0, 0, 0, 1], "extradata must be Annex-B");
    let nt = nal_types(&extra);
    assert!(
        nt.contains(&7) && nt.contains(&8),
        "SPS and PPS in extradata: {nt:?}"
    );

    let mut dec = Decoder::new(Codec::H264).unwrap();
    dec.push_config(&extra).unwrap();
    let mut decoded = 0;
    for n in 0..90u64 {
        let force = n == 45;
        let pkt = enc
            .encode(&gen_.frame(n), force)
            .unwrap()
            .expect("one packet per frame");
        if n == 0 || force {
            assert!(pkt.keyframe, "frame {n} must be a keyframe");
            assert!(nal_types(&pkt.data).contains(&5), "IDR NAL expected");
            assert!(
                nal_types(&pkt.data).contains(&7),
                "in-band SPS on keyframes"
            );
        } else {
            assert!(!pkt.keyframe, "frame {n} must not be a keyframe");
        }
        for f in dec.decode(&pkt.data, n as i64).unwrap() {
            assert_eq!(
                read_frame_number(&f.y, f.width as usize, f.width, f.height),
                Some(n)
            );
            decoded += 1;
        }
    }
    assert_eq!(
        decoded, 90,
        "low-delay decoding yields one picture per packet"
    );
}

#[test]
fn probe() {
    if have_x264() {
        assert!(probe_encoder("libx264", 640, 360));
    }
    let r = probe_encoder_with("definitely_not_an_encoder", 640, 360, None);
    assert!(!r.ok);
}

#[test]
fn sw_source_paces_and_coalesces() {
    if !have_x264() {
        return;
    }
    let mut src = SwVideoSource::new();
    let cfg = StreamConfig {
        width: 640,
        height: 360,
        refresh_mhz: 60_000,
        codec: Codec::H264,
        bitrate_kbps: 2_000,
        encoder_name: "libx264".into(),
    };
    src.start(&cfg, CursorSink::null()).unwrap();
    let mut dec = Decoder::new(Codec::H264).unwrap();
    let mut last = None;
    let t0 = std::time::Instant::now();
    for i in 0..10 {
        let f = src
            .next_frame(Duration::from_millis(100))
            .unwrap()
            .expect("frame");
        if i == 0 {
            assert!(f.keyframe && f.codec_config.is_some());
            dec.push_config(f.codec_config.as_ref().unwrap()).unwrap();
        } else {
            assert!(f.codec_config.is_none());
        }
        let id = f.frame_id.unwrap();
        assert!(last.is_none_or(|l| id > l));
        last = Some(id);
        let pics = dec.decode(&f.data, 0).unwrap();
        assert_eq!(read_frame_number(&pics[0].y, 640, 640, 360), Some(id));
    }
    assert!(
        t0.elapsed() >= Duration::from_millis(120),
        "paced at 60 fps"
    );
    // Stall the consumer: the next frame must jump ahead instead of delivering the backlog.
    let before = last.unwrap();
    std::thread::sleep(Duration::from_millis(250));
    let f = src.next_frame(Duration::from_millis(100)).unwrap().unwrap();
    assert!(
        f.frame_id.unwrap() >= before + 10,
        "coalesced to newest: {} -> {:?}",
        before,
        f.frame_id
    );
    // Keyframe request.
    src.request_keyframe();
    let f = src.next_frame(Duration::from_millis(100)).unwrap().unwrap();
    assert!(f.keyframe);
    src.stop();
    assert!(src.next_frame(Duration::from_millis(1)).is_err());
}
