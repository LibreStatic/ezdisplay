//! Host session (SwVideoSource) <-> fake client over a real TCP loopback.

use ezd_fake_client::{ClientEnd, FakeClientConfig, run_fake_client};
use ezd_ffmpeg::{SwVideoSource, encoder_available};
use ezd_host_core::{EndReason, run_host_session};
use ezd_link::Link;
use ezd_proto::Prefs;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_to_fake_client_over_tcp() {
    if !encoder_available("libx264") && !encoder_available("libopenh264") {
        eprintln!("no software H.264 encoder, skipping");
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let host = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let link = Link::from_tcp(sock).unwrap();
        run_host_session(link, Box::new(SwVideoSource::new()), Prefs::default()).await
    });

    let cfg = FakeClientConfig {
        width: 1280,
        height: 720,
        refresh_rates_mhz: vec![60_000],
        frames: Some(150),
        request_keyframe_after: Some(60),
        ..Default::default()
    };
    let link = Link::connect_tcp(addr).await.unwrap();
    let report = timeout(Duration::from_secs(60), run_fake_client(link, cfg))
        .await
        .unwrap()
        .unwrap();
    let summary = timeout(Duration::from_secs(10), host)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    println!("{}", report.summary());
    println!(
        "host: frames {} keyframes {} bytes {} rtt {:?} us transfer p50 {:?} p95 {:?} us ack_timeouts {}",
        summary.frames_sent,
        summary.keyframes_sent,
        summary.video_bytes,
        summary.rtt_us,
        summary.transfer_us_p50,
        summary.transfer_us_p95,
        summary.ack_timeouts
    );

    // Stream negotiated as asked.
    assert_eq!(
        (
            report.stream.width,
            report.stream.height,
            report.stream.refresh_mhz
        ),
        (1280, 720, 60_000)
    );
    assert_eq!(report.end, ClientEnd::FramesReached);

    // All frames decoded, numbers read from the picture match the frame ids, strictly increasing.
    assert!(
        report.frames_decoded >= 120,
        "decoded {}",
        report.frames_decoded
    );
    assert_eq!(report.frames_dropped, 0);
    assert_eq!(report.frames_decoded, report.frames_received);
    assert_eq!(report.frame_numbers.len(), report.frame_ids.len());
    for (num, id) in report.frame_numbers.iter().zip(&report.frame_ids) {
        assert_eq!(
            *num,
            Some(*id),
            "frame number in picture must equal frame_id"
        );
    }
    assert!(
        report.frame_ids.windows(2).all(|w| w[0] < w[1]),
        "ids strictly increase"
    );

    // Codec config precedes the first keyframe; keyframe request yields another keyframe.
    assert_eq!(report.codec_configs, 1);
    assert!(report.config_before_first_keyframe);
    assert_eq!(report.keyframe_ids.first(), report.frame_ids.first());
    assert!(
        report.keyframe_ids.len() >= 2,
        "keyframes: {:?}",
        report.keyframe_ids
    );
    assert!(report.keyframe_ids[1] >= 60);

    // Cursor traffic arrived.
    assert_eq!(report.cursor_shapes, 1);
    assert!(
        report.cursor_states > 10,
        "cursor states: {}",
        report.cursor_states
    );
    assert_eq!(report.cursor_missing_shape, 0);

    // Clock sync ran and latency was measured.
    assert!(report.clock_offset_us.is_some());
    assert!(report.e2e_us_p50 > 0);

    // Clean shutdown on both sides.
    assert_eq!(summary.end, EndReason::ClientBye("client done".into()));
    assert!(summary.frames_sent >= 150);
    assert_eq!(summary.codec_configs_sent, 1);
    assert_eq!(summary.keyframe_requests, 1);
    assert_eq!(summary.ack_timeouts, 0, "every frame was acked in time");
    assert!(summary.transfer_us_p50.is_some(), "transfer time measured");
    assert!(
        summary.last_client_stats.is_some(),
        "client stats reached the host"
    );
}
