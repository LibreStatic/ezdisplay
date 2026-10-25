//! Wi-Fi (QUIC) client side shared by the Android app and the fake client: the store of paired
//! hosts, pairing (PIN or scanned URI) and opening sessions on a paired host.
//!
//! The client keeps its identity (`identity.crt`/`identity.key`) and the paired hosts
//! (`hosts.json`, a list keyed by certificate fingerprint) in a state directory.

use ezd_link::Link;
use ezd_link::identity::{Identity, fingerprint_hex, parse_fingerprint, write_atomic};
use ezd_link::pair::pair;
use ezd_link::quic::{Congestion, QuicError, connect};
use ezd_proto::pair::{Fingerprint, Refusal};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HOSTS_FILE: &str = "hosts.json";
const RETRY_DELAY: Duration = Duration::from_millis(200);

/// A host this client paired with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredHost {
    /// Host name, as announced when pairing.
    pub name: String,
    /// Host certificate fingerprint, lower-case hex.
    pub fingerprint: String,
    /// Address the host was last reached at (`ip:port`), a fallback when discovery finds none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
}

impl StoredHost {
    /// The parsed fingerprint (validated on load, so `None` only for hand-built values).
    pub fn fingerprint_bytes(&self) -> Option<Fingerprint> {
        parse_fingerprint(&self.fingerprint)
    }
}

/// Why a Wi-Fi operation failed.
#[derive(Debug, thiserror::Error)]
pub enum WifiError {
    /// No address was given.
    #[error("no host address: pass an address or a pairing URI")]
    NoAddress,
    /// No paired host matches and there is no PIN to pair with.
    #[error("not paired with this host: pass --pin or --uri")]
    NotPaired,
    /// The connection, the handshake or the pairing failed.
    #[error("{0}")]
    Quic(#[from] QuicError),
    /// State directory problem.
    #[error("{}: {source}", path.display())]
    State {
        /// The file or directory.
        path: PathBuf,
        /// The OS error.
        source: std::io::Error,
    },
    /// `hosts.json` does not parse; it was left untouched.
    #[error("{} is corrupt: {reason}", path.display())]
    Corrupt {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
}

fn state_err(path: &Path) -> impl FnOnce(std::io::Error) -> WifiError + '_ {
    move |source| WifiError::State {
        path: path.to_path_buf(),
        source,
    }
}

/// Loads the paired hosts; a missing file is an empty list, a corrupt one an error.
pub fn load_hosts(dir: &Path) -> Result<Vec<StoredHost>, WifiError> {
    let path = dir.join(HOSTS_FILE);
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(WifiError::State { path, source }),
    };
    let hosts: Vec<StoredHost> = serde_json::from_slice(&data).map_err(|e| WifiError::Corrupt {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    if hosts.iter().any(|h| h.fingerprint_bytes().is_none()) {
        return Err(WifiError::Corrupt {
            path,
            reason: "invalid fingerprint".into(),
        });
    }
    Ok(hosts)
}

fn save_hosts(dir: &Path, hosts: &[StoredHost]) -> Result<(), WifiError> {
    let path = dir.join(HOSTS_FILE);
    let json = serde_json::to_vec_pretty(hosts).expect("hosts serialize");
    write_atomic(&path, &json).map_err(|source| WifiError::State { path, source })
}

/// Adds `host`, replacing the entry with the same fingerprint (re-pairing renames or moves it).
pub fn add_host(dir: &Path, host: StoredHost) -> Result<(), WifiError> {
    std::fs::create_dir_all(dir).map_err(state_err(dir))?;
    let mut hosts = load_hosts(dir)?;
    match hosts.iter_mut().find(|h| h.fingerprint == host.fingerprint) {
        Some(existing) => *existing = host,
        None => hosts.push(host),
    }
    save_hosts(dir, &hosts)
}

/// Forgets the host with this fingerprint (either case); false when it was not paired.
pub fn forget_host(dir: &Path, fingerprint: &str) -> Result<bool, WifiError> {
    let mut hosts = load_hosts(dir)?;
    let before = hosts.len();
    hosts.retain(|h| !h.fingerprint.eq_ignore_ascii_case(fingerprint));
    if hosts.len() == before {
        return Ok(false);
    }
    save_hosts(dir, &hosts)?;
    Ok(true)
}

/// Loads the identity of this device from `dir`, creating it on first use.
pub fn load_identity(dir: &Path) -> Result<Identity, WifiError> {
    std::fs::create_dir_all(dir).map_err(state_err(dir))?;
    Identity::load_or_create(dir).map_err(state_err(dir))
}

/// What pairing needs.
#[derive(Debug, Clone)]
pub struct PairRequest {
    /// Host address.
    pub addr: SocketAddr,
    /// The 6-digit PIN shown by the host.
    pub pin: String,
    /// Host certificate fingerprint when known (scanned URI); the handshake requires it. A bare
    /// PIN is authenticated by the pairing itself.
    pub pinned: Option<Fingerprint>,
    /// Name announced to the host.
    pub device_name: String,
}

/// Pairs with a host and stores it. Nothing is stored on failure.
pub async fn pair_host(dir: &Path, req: &PairRequest) -> Result<StoredHost, WifiError> {
    let id = load_identity(dir)?;
    let conn = connect(req.addr, &id, req.pinned, Congestion::default()).await?;
    let host = pair(conn, &req.pin, &req.device_name).await?;
    let stored = StoredHost {
        name: host.name,
        fingerprint: fingerprint_hex(&host.fingerprint),
        addr: Some(req.addr.to_string()),
    };
    add_host(dir, stored.clone())?;
    Ok(stored)
}

/// Worth another session attempt: the host is not up yet, or still finishing the previous
/// session (`Busy`).
fn retryable(e: &QuicError) -> bool {
    matches!(
        e,
        QuicError::Connect(_)
            | QuicError::Connection(_)
            | QuicError::Timeout
            | QuicError::Io(_)
            | QuicError::Refused(Refusal::Busy)
    )
}

/// Opens a session on a paired host, retrying while it is unreachable or busy for up to
/// `timeout`. The result is the link to hand to the client. On success the address is stored as
/// the host's last known one.
pub async fn connect_paired(
    dir: &Path,
    addr: SocketAddr,
    host: Fingerprint,
    timeout: Duration,
) -> Result<Link, WifiError> {
    let id = load_identity(dir)?;
    let cc = Congestion::default();
    let start = Instant::now();
    let link = loop {
        let attempt = async {
            connect(addr, &id, Some(host), cc)
                .await?
                .open_session()
                .await
        };
        match attempt.await {
            Ok(l) => break l,
            Err(e) if retryable(&e) && start.elapsed() < timeout => {
                tracing::debug!("connect failed ({e}), retrying");
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(QuicError::Refused(Refusal::NotPaired)) => {
                // The connection is pinned to the host's certificate, so only the real host can
                // say this: it forgot this device, and the pairing is gone for good.
                tracing::info!("the host no longer knows this device, forgetting it");
                if let Err(e) = forget_host(dir, &fingerprint_hex(&host)) {
                    tracing::warn!("cannot forget the host: {e}");
                }
                return Err(QuicError::Refused(Refusal::NotPaired).into());
            }
            Err(e) => return Err(e.into()),
        }
    };
    remember_addr(dir, &fingerprint_hex(&host), addr);
    Ok(link)
}

/// Stores the address a host was reached at; failures only cost the fallback address.
fn remember_addr(dir: &Path, fingerprint: &str, addr: SocketAddr) {
    let Ok(mut hosts) = load_hosts(dir) else {
        return;
    };
    let addr = addr.to_string();
    if let Some(h) = hosts.iter_mut().find(|h| h.fingerprint == fingerprint)
        && h.addr.as_deref() != Some(&addr)
    {
        h.addr = Some(addr);
        if let Err(e) = save_hosts(dir, &hosts) {
            tracing::debug!("cannot store the host address: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let dir = std::env::temp_dir()
                .join(format!("ezd-client-core-wifi-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn host(name: &str, byte: u8, addr: Option<&str>) -> StoredHost {
        StoredHost {
            name: name.into(),
            fingerprint: fingerprint_hex(&[byte; 32]),
            addr: addr.map(Into::into),
        }
    }

    #[test]
    fn a_missing_store_is_empty() {
        let dir = TempDir::new("missing");
        assert!(load_hosts(&dir.0).unwrap().is_empty());
        assert!(!forget_host(&dir.0, &fingerprint_hex(&[1; 32])).unwrap());
    }

    #[test]
    fn hosts_are_added_replaced_and_forgotten() {
        let dir = TempDir::new("multi");
        add_host(&dir.0, host("desk", 1, Some("192.168.1.5:27183"))).unwrap();
        add_host(&dir.0, host("laptop", 2, None)).unwrap();
        assert_eq!(load_hosts(&dir.0).unwrap().len(), 2);

        // Same fingerprint replaces: name and address change, order is kept.
        add_host(&dir.0, host("desk renamed", 1, Some("10.0.0.9:27183"))).unwrap();
        let hosts = load_hosts(&dir.0).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "desk renamed");
        assert_eq!(hosts[0].addr.as_deref(), Some("10.0.0.9:27183"));
        assert_eq!(hosts[1].name, "laptop");

        // Forget is case-insensitive and leaves the others.
        let upper = fingerprint_hex(&[1; 32]).to_uppercase();
        assert!(forget_host(&dir.0, &upper).unwrap());
        assert!(!forget_host(&dir.0, &upper).unwrap());
        let hosts = load_hosts(&dir.0).unwrap();
        assert_eq!(hosts, vec![host("laptop", 2, None)]);
    }

    #[test]
    fn the_json_round_trips_and_omits_a_missing_address() {
        let hosts = vec![host("desk", 1, Some("1.2.3.4:5")), host("laptop", 2, None)];
        let json = serde_json::to_string(&hosts).unwrap();
        assert!(!json.contains("null"));
        assert_eq!(
            serde_json::from_str::<Vec<StoredHost>>(&json).unwrap(),
            hosts
        );
    }

    #[test]
    fn a_corrupt_store_is_an_error_and_is_left_alone() {
        let dir = TempDir::new("corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join(HOSTS_FILE), "nope").unwrap();
        assert!(matches!(load_hosts(&dir.0), Err(WifiError::Corrupt { .. })));
        assert!(add_host(&dir.0, host("desk", 1, None)).is_err());
        assert_eq!(std::fs::read(dir.0.join(HOSTS_FILE)).unwrap(), b"nope");

        std::fs::write(
            dir.0.join(HOSTS_FILE),
            r#"[{"name":"x","fingerprint":"abcd"}]"#,
        )
        .unwrap();
        assert!(matches!(load_hosts(&dir.0), Err(WifiError::Corrupt { .. })));
    }

    #[tokio::test]
    async fn a_host_that_forgot_us_is_forgotten_without_retrying() {
        let dir = TempDir::new("not-paired");
        let host_id = Identity::generate().unwrap();
        let server = ezd_link::quic::QuicServer::bind(
            "127.0.0.1:0".parse().unwrap(),
            &host_id,
            Congestion::default(),
        )
        .unwrap();
        let addr = server.local_addr().unwrap();
        let fp = host_id.fingerprint();
        add_host(
            &dir.0,
            StoredHost {
                name: "desk".into(),
                fingerprint: fingerprint_hex(&fp),
                addr: Some(addr.to_string()),
            },
        )
        .unwrap();
        add_host(&dir.0, host("laptop", 2, None)).unwrap();
        let served = tokio::spawn(async move {
            let mut refused = 0;
            while let Some(Ok(conn)) = server.accept().await {
                if let Ok(opened) = conn.accept_open().await {
                    opened.refuse(Refusal::NotPaired).await;
                    refused += 1;
                }
                if refused > 0 {
                    break;
                }
            }
            refused
        });
        let err = connect_paired(&dir.0, addr, fp, Duration::from_secs(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(err, WifiError::Quic(QuicError::Refused(Refusal::NotPaired))),
            "{err:?}"
        );
        assert_eq!(served.await.unwrap(), 1, "the refusal must not be retried");
        assert_eq!(load_hosts(&dir.0).unwrap(), vec![host("laptop", 2, None)]);
    }

    #[test]
    fn remembering_an_address_updates_only_that_host() {
        let dir = TempDir::new("remember");
        add_host(&dir.0, host("desk", 1, None)).unwrap();
        add_host(&dir.0, host("laptop", 2, Some("9.9.9.9:1"))).unwrap();
        remember_addr(
            &dir.0,
            &fingerprint_hex(&[1; 32]),
            "192.168.1.5:27183".parse().unwrap(),
        );
        let hosts = load_hosts(&dir.0).unwrap();
        assert_eq!(hosts[0].addr.as_deref(), Some("192.168.1.5:27183"));
        assert_eq!(hosts[1].addr.as_deref(), Some("9.9.9.9:1"));
    }
}
