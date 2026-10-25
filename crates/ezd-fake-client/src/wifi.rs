//! Wi-Fi (QUIC) connection of the fake client: pairing with a PIN, remembering the host, and
//! opening sessions on a paired host. The store and the connection logic are shared with the
//! Android app (`ezd_client_core::wifi`).
//!
//! The client keeps its identity (`identity.crt`/`identity.key`) and the paired hosts
//! (`hosts.json`) in a state directory.

use ezd_client_core::wifi::{PairRequest, connect_paired, pair_host};
pub use ezd_client_core::wifi::{StoredHost, WifiError, load_hosts};
use ezd_link::Link;
use ezd_link::pair::PairUri;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// The default state directory: `ezdisplay-fake-client` inside the user's config directory.
pub fn default_state_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ezdisplay-fake-client"))
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
    /// Identity and paired hosts live here.
    pub state_dir: PathBuf,
    /// Name announced when pairing.
    pub device_name: String,
    /// Keep retrying the session connection this long.
    pub connect_timeout: Duration,
}

/// What happened besides the connection, for the caller to tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiOutcome {
    /// Set when this call paired with a host.
    pub paired_with: Option<StoredHost>,
}

/// Pairs if asked to, then opens a session. Without pairing, the host is the paired one stored
/// for `addr`, or the only paired host. The result is the link to hand to
/// [`run_fake_client`](crate::run_fake_client).
pub async fn connect_wifi(opts: &WifiOptions) -> Result<(Link, WifiOutcome), WifiError> {
    let addr = opts
        .addr
        .or(opts.uri.as_ref().map(|u| u.addr))
        .ok_or(WifiError::NoAddress)?;
    let pin = opts
        .pin
        .clone()
        .or(opts.uri.as_ref().map(|u| u.pin.clone()));
    let mut paired_with = None;
    let fingerprint = if let Some(pin) = pin {
        let req = PairRequest {
            addr,
            pin,
            // The URI pins the host certificate.
            pinned: opts.uri.as_ref().map(|u| u.fingerprint),
            device_name: opts.device_name.clone(),
        };
        let stored = pair_host(&opts.state_dir, &req).await?;
        let fp = stored.fingerprint_bytes().expect("fresh pairing");
        paired_with = Some(stored);
        fp
    } else {
        let hosts = load_hosts(&opts.state_dir)?;
        let by_addr = hosts
            .iter()
            .find(|h| h.addr.as_deref() == Some(addr.to_string().as_str()));
        let host = by_addr
            .or(if hosts.len() == 1 {
                hosts.first()
            } else {
                None
            })
            .ok_or(WifiError::NotPaired)?;
        host.fingerprint_bytes().expect("checked by load_hosts")
    };
    let link = connect_paired(&opts.state_dir, addr, fingerprint, opts.connect_timeout).await?;
    Ok((link, WifiOutcome { paired_with }))
}
