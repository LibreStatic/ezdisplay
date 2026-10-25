//! Host session <-> fake client over a real QUIC loopback: pairing with a PIN, then reconnecting
//! with the stored pairing and no PIN.
#![cfg(feature = "wifi")]

use ezd_fake_client::wifi::{WifiError, WifiOptions, connect_wifi, load_host};
use ezd_fake_client::{ClientEnd, FakeClientConfig, run_fake_client};
use ezd_ffmpeg::{SwVideoSource, encoder_available};
use ezd_host_core::run_host_session;
use ezd_link::identity::Identity;
use ezd_link::pair::{PairUri, PairingWindow, host_pair};
use ezd_link::quic::{Congestion, QuicError, QuicServer};
use ezd_proto::Prefs;
use ezd_proto::pair::{Fingerprint, LinkOpen, Refusal};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let dir =
            std::env::temp_dir().join(format!("ezd-fake-client-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A host that pairs while its window is open and streams to paired clients.
fn spawn_host(
    server: QuicServer,
    window: Arc<PairingWindow>,
    paired: Arc<Mutex<Vec<Fingerprint>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // A failed handshake is an `Err` and the server keeps accepting.
        while let Some(accepted) = server.accept().await {
            let Ok(conn) = accepted else { continue };
            let (window, paired) = (window.clone(), paired.clone());
            tokio::spawn(async move {
                let Ok(opened) = conn.accept_open().await else {
                    return;
                };
                match opened.open.clone() {
                    LinkOpen::Pair { .. } => {
                        let _ = host_pair(opened, &window, "test host", |c| {
                            paired.lock().unwrap().push(c.fingerprint);
                            Ok(())
                        })
                        .await;
                    }
                    LinkOpen::Session => {
                        let known = opened
                            .peer_fingerprint()
                            .is_some_and(|fp| paired.lock().unwrap().contains(&fp));
                        if !known {
                            return opened.refuse(Refusal::NotPaired).await;
                        }
                        let link = opened.accept_session().await.unwrap();
                        let _ = run_host_session(
                            link,
                            Box::new(SwVideoSource::new()),
                            Prefs::default(),
                        )
                        .await;
                    }
                }
            });
        }
    })
}

fn options(addr: SocketAddr, dir: &TempDir, pin: Option<&str>) -> WifiOptions {
    WifiOptions {
        addr: Some(addr),
        pin: pin.map(Into::into),
        uri: None,
        state_dir: dir.0.clone(),
        device_name: "fake tablet".into(),
        connect_timeout: Duration::from_secs(2),
    }
}

fn client_cfg() -> FakeClientConfig {
    FakeClientConfig {
        width: 1280,
        height: 720,
        refresh_rates_mhz: vec![60_000],
        frames: Some(30),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairs_with_a_pin_then_reconnects_without_one() {
    if !encoder_available("libx264") && !encoder_available("libopenh264") {
        eprintln!("no software H.264 encoder, skipping");
        return;
    }
    let host_id = Identity::generate().unwrap();
    let server =
        QuicServer::bind("127.0.0.1:0".parse().unwrap(), &host_id, Congestion::Cubic).unwrap();
    let addr = server.local_addr().unwrap();
    let window = Arc::new(PairingWindow::default());
    window.open_with("123456".into(), Duration::from_secs(60));
    let paired = Arc::new(Mutex::new(Vec::new()));
    let host = spawn_host(server, window.clone(), paired.clone());
    let dir = TempDir::new("pair");

    // No pairing yet and no PIN.
    let err = connect_wifi(&options(addr, &dir, None))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, WifiError::NotPaired), "{err:?}");

    // Pair, then stream.
    let (link, outcome) = connect_wifi(&options(addr, &dir, Some("123456")))
        .await
        .unwrap();
    assert_eq!(outcome.paired_with.unwrap().name, "test host");
    let report = timeout(Duration::from_secs(60), run_fake_client(link, client_cfg()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.end, ClientEnd::FramesReached);
    assert!(report.frames_decoded >= 30);
    assert_eq!(
        load_host(&dir.0).unwrap().unwrap().fingerprint,
        ezd_link::identity::fingerprint_hex(&host_id.fingerprint())
    );

    // Later: the address alone is enough, and the window is closed.
    assert!(!window.is_open());
    let (link, outcome) = connect_wifi(&options(addr, &dir, None)).await.unwrap();
    assert!(outcome.paired_with.is_none());
    let report = timeout(Duration::from_secs(60), run_fake_client(link, client_cfg()))
        .await
        .unwrap()
        .unwrap();
    assert!(report.frames_decoded >= 30);
    host.abort();
}

#[tokio::test]
async fn a_wrong_pin_is_refused_and_nothing_is_stored() {
    let host_id = Identity::generate().unwrap();
    let server =
        QuicServer::bind("127.0.0.1:0".parse().unwrap(), &host_id, Congestion::Cubic).unwrap();
    let addr = server.local_addr().unwrap();
    let window = Arc::new(PairingWindow::default());
    window.open_with("123456".into(), Duration::from_secs(60));
    let host = spawn_host(server, window, Arc::default());
    let dir = TempDir::new("wrong-pin");
    let err = connect_wifi(&options(addr, &dir, Some("654321")))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, WifiError::Quic(QuicError::Refused(_))),
        "{err:?}"
    );
    assert!(load_host(&dir.0).unwrap().is_none());
    host.abort();
}

#[tokio::test]
async fn a_uri_pins_the_host_and_supplies_address_and_pin() {
    let host_id = Identity::generate().unwrap();
    let server =
        QuicServer::bind("127.0.0.1:0".parse().unwrap(), &host_id, Congestion::Cubic).unwrap();
    let addr = server.local_addr().unwrap();
    let window = Arc::new(PairingWindow::default());
    window.open_with("123456".into(), Duration::from_secs(60));
    let host = spawn_host(server, window, Arc::default());
    let uri = |fingerprint| PairUri {
        addr,
        fingerprint,
        pin: "123456".into(),
        name: "test host".into(),
    };
    let dir = TempDir::new("uri");

    // A URI for another certificate fails the handshake; nothing is stored.
    let opts = |u: PairUri| WifiOptions {
        addr: None,
        pin: None,
        uri: Some(u),
        ..options(addr, &dir, None)
    };
    let err = connect_wifi(&opts(uri([7; 32]))).await.err().unwrap();
    assert!(
        matches!(err, WifiError::Quic(QuicError::FingerprintMismatch)),
        "{err:?}"
    );
    assert!(load_host(&dir.0).unwrap().is_none());

    let (_link, outcome) = connect_wifi(&opts(uri(host_id.fingerprint())))
        .await
        .unwrap();
    assert!(outcome.paired_with.is_some());
    host.abort();
}

#[test]
fn a_corrupt_host_file_is_an_error() {
    let dir = TempDir::new("corrupt");
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(dir.0.join("host.json"), "nope").unwrap();
    assert!(matches!(load_host(&dir.0), Err(WifiError::Corrupt { .. })));
}
