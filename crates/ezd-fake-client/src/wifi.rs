//! Wi-Fi (QUIC) connection of the fake client: pairing with a PIN, remembering the host, and
//! opening sessions on the paired host.
//!
//! The client keeps its identity (`identity.crt`/`identity.key`) and the paired host
//! (`host.json`) in a state directory.

use ezd_link::Link;
use ezd_link::identity::{Identity, fingerprint_hex, parse_fingerprint, write_atomic};
use ezd_link::pair::{PairUri, pair};
use ezd_link::quic::{Congestion, QuicError, connect};
use ezd_proto::pair::{Fingerprint, Refusal};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HOST_FILE: &str = "host.json";

/// The default state directory: `ezdisplay-fake-client` inside the user's config directory.
pub fn default_state_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ezdisplay-fake-client"))
}

/// The host this client paired with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredHost {
    /// Host name, as announced when pairing.
    pub name: String,
    /// Host certificate fingerprint, lower-case hex.
    pub fingerprint: String,
}

/// What to connect to and how.
#[derive(Debug, Clone)]
pub struct WifiOptions {
    /// Host address; taken from `uri` when `None`.
    pub addr: Option<SocketAddr>,
    /// Pair first with this PIN.
    pub pin: Option<String>,
    /// Pairing URI (pins the host fingerprint and carries a PIN).
    pub uri: Option<PairUri>,
    /// Identity and paired host live here.
    pub state_dir: PathBuf,
    /// Name announced when pairing.
    pub device_name: String,
    /// Keep retrying the session connection this long.
    pub connect_timeout: Duration,
}

/// Why the Wi-Fi connection failed.
#[derive(Debug, thiserror::Error)]
pub enum WifiError {
    /// No address was given.
    #[error("no host address: pass an address or a pairing URI")]
    NoAddress,
    /// No paired host and no PIN to pair with.
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
    /// `host.json` does not parse; it was left untouched.
    #[error("{} is corrupt: {reason}", path.display())]
    Corrupt {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
}

/// What happened besides the connection, for the caller to tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiOutcome {
    /// Set when this call paired with a host.
    pub paired_with: Option<StoredHost>,
}

/// Loads the stored host; a missing file is `None`, a corrupt one an error.
pub fn load_host(dir: &Path) -> Result<Option<StoredHost>, WifiError> {
    let path = dir.join(HOST_FILE);
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(WifiError::State { path, source }),
    };
    let host: StoredHost = serde_json::from_slice(&data).map_err(|e| WifiError::Corrupt {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    if parse_fingerprint(&host.fingerprint).is_none() {
        return Err(WifiError::Corrupt {
            path,
            reason: "invalid fingerprint".into(),
        });
    }
    Ok(Some(host))
}

fn save_host(dir: &Path, host: &StoredHost) -> Result<(), WifiError> {
    let path = dir.join(HOST_FILE);
    let json = serde_json::to_vec_pretty(host).expect("host serializes");
    write_atomic(&path, &json).map_err(|source| WifiError::State { path, source })
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

/// Pairs if asked to, then opens a session. The result is the link to hand to
/// [`run_fake_client`](crate::run_fake_client).
pub async fn connect_wifi(opts: &WifiOptions) -> Result<(Link, WifiOutcome), WifiError> {
    let addr = opts
        .addr
        .or(opts.uri.as_ref().map(|u| u.addr))
        .ok_or(WifiError::NoAddress)?;
    std::fs::create_dir_all(&opts.state_dir).map_err(|source| WifiError::State {
        path: opts.state_dir.clone(),
        source,
    })?;
    let id = Identity::load_or_create(&opts.state_dir).map_err(|source| WifiError::State {
        path: opts.state_dir.clone(),
        source,
    })?;
    let cc = Congestion::default();

    let pin = opts
        .pin
        .clone()
        .or(opts.uri.as_ref().map(|u| u.pin.clone()));
    let mut paired_with = None;
    let host_fp: Fingerprint = if let Some(pin) = pin {
        // The URI pins the host certificate; a bare PIN is authenticated by the pairing itself.
        let pinned = opts.uri.as_ref().map(|u| u.fingerprint);
        let conn = connect(addr, &id, pinned, cc).await?;
        let host = pair(conn, &pin, &opts.device_name).await?;
        let stored = StoredHost {
            name: host.name,
            fingerprint: fingerprint_hex(&host.fingerprint),
        };
        save_host(&opts.state_dir, &stored)?;
        paired_with = Some(stored);
        host.fingerprint
    } else {
        let host = load_host(&opts.state_dir)?.ok_or(WifiError::NotPaired)?;
        parse_fingerprint(&host.fingerprint).expect("checked by load_host")
    };

    let start = Instant::now();
    let link = loop {
        let attempt = async {
            connect(addr, &id, Some(host_fp), cc)
                .await?
                .open_session()
                .await
        };
        match attempt.await {
            Ok(l) => break l,
            Err(e) if retryable(&e) && start.elapsed() < opts.connect_timeout => {
                tracing::debug!("connect failed ({e}), retrying");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => return Err(e.into()),
        }
    };
    Ok((link, WifiOutcome { paired_with }))
}
