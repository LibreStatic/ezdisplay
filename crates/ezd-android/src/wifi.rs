//! Wi-Fi pairing and paired-host management behind the JNI calls, as plain functions returning
//! the JSON Kotlin reads. Portable, so it builds and tests on the host.

use ezd_client_core::wifi::{
    PairRequest, StoredHost, WifiError, forget_host, load_hosts, load_identity, pair_host,
};
use ezd_link::identity::fingerprint_short;
use ezd_link::pair::PairUri;
use ezd_link::quic::QuicError;
use ezd_proto::pair::{Refusal, is_valid_pin};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

/// Port hosts listen on when an address comes without one.
pub const DEFAULT_PORT: u16 = 27183;

/// How long a pairing may take (connect plus the SPAKE2 exchange).
const PAIR_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Serialize)]
struct HostJson<'a> {
    name: &'a str,
    fingerprint: &'a str,
    #[serde(rename = "shortFp")]
    short_fp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    addr: Option<&'a str>,
}

fn host_json(h: &StoredHost) -> HostJson<'_> {
    HostJson {
        name: &h.name,
        fingerprint: &h.fingerprint,
        short_fp: h
            .fingerprint_bytes()
            .map(|fp| fingerprint_short(&fp))
            .unwrap_or_default(),
        addr: h.addr.as_deref(),
    }
}

/// `host:port`, or a bare IP address that gets [`DEFAULT_PORT`].
pub fn parse_addr(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    s.parse::<SocketAddr>()
        .ok()
        .or_else(|| s.parse::<IpAddr>().ok().map(|ip| (ip, DEFAULT_PORT).into()))
}

/// The stable error code Kotlin maps to a localized message.
fn error_code(e: &WifiError) -> &'static str {
    match e {
        WifiError::Quic(QuicError::Refused(r)) => match r {
            Refusal::WrongPin => "wrong_pin",
            Refusal::PairingClosed => "pairing_closed",
            Refusal::TooManyAttempts => "too_many_attempts",
            Refusal::Busy => "busy",
            Refusal::NotPaired => "not_paired",
        },
        WifiError::Quic(QuicError::FingerprintMismatch) => "fingerprint_mismatch",
        WifiError::Quic(QuicError::InvalidPin) => "invalid_pin",
        WifiError::Quic(QuicError::Timeout) => "timeout",
        WifiError::Quic(QuicError::Connect(_) | QuicError::Connection(_) | QuicError::Io(_)) => {
            "unreachable"
        }
        _ => "other",
    }
}

fn failure(code: &str, message: &str) -> String {
    serde_json::json!({ "ok": false, "error": code, "message": message }).to_string()
}

/// Pairs with the host at `addr` (`host:port` or an IP) using a 6-digit PIN, or with the host a
/// scanned `ezd://pair?...` URI describes (the URI carries address, PIN and the fingerprint to
/// pin). Blocks until done. Returns `{"ok":true,"host":{name,fingerprint,shortFp}}` or
/// `{"ok":false,"error":<code>,"message":<text>}`.
pub fn pair_json(addr: &str, pin_or_uri: &str, device_name: &str, state_dir: &Path) -> String {
    let input = pin_or_uri.trim();
    let req = if input.starts_with("ezd://") {
        let Some(uri) = PairUri::parse(input) else {
            return failure("invalid_uri", "not a pairing link");
        };
        PairRequest {
            addr: uri.addr,
            pin: uri.pin,
            pinned: Some(uri.fingerprint),
            device_name: device_name.into(),
        }
    } else {
        let Some(addr) = parse_addr(addr) else {
            return failure("invalid_address", "not a host address");
        };
        PairRequest {
            addr,
            pin: input.into(),
            pinned: None,
            device_name: device_name.into(),
        }
    };
    if !is_valid_pin(&req.pin) {
        return failure("invalid_pin", "the PIN must be 6 digits");
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return failure("other", &format!("runtime: {e}")),
    };
    let result = rt.block_on(async {
        tokio::time::timeout(PAIR_TIMEOUT, pair_host(state_dir, &req))
            .await
            .unwrap_or(Err(WifiError::Quic(QuicError::Timeout)))
    });
    match result {
        Ok(host) => serde_json::json!({ "ok": true, "host": host_json(&host) }).to_string(),
        Err(e) => failure(error_code(&e), &e.to_string()),
    }
}

/// The paired hosts as a JSON array of `{name,fingerprint,shortFp,addr?}`; `[]` when there are
/// none or the store is unreadable.
pub fn hosts_json(state_dir: &Path) -> String {
    let hosts = load_hosts(state_dir).unwrap_or_else(|e| {
        tracing::warn!("cannot read the paired hosts: {e}");
        Vec::new()
    });
    let list: Vec<HostJson<'_>> = hosts.iter().map(host_json).collect();
    serde_json::to_string(&list).expect("hosts serialize")
}

/// Forgets a paired host; false when it was unknown or the store failed.
pub fn forget(state_dir: &Path, fingerprint: &str) -> bool {
    forget_host(state_dir, fingerprint).unwrap_or_else(|e| {
        tracing::warn!("cannot forget the host: {e}");
        false
    })
}

/// Short fingerprint of this device (created on first use); empty when the state is unusable.
pub fn identity_short(state_dir: &Path) -> String {
    load_identity(state_dir)
        .map(|id| fingerprint_short(&id.fingerprint()))
        .unwrap_or_else(|e| {
            tracing::warn!("cannot load the identity: {e}");
            String::new()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezd_client_core::wifi::add_host;
    use ezd_link::identity::{Identity, fingerprint_hex};
    use ezd_link::pair::{PairingWindow, host_pair};
    use ezd_link::quic::{Congestion, QuicServer};
    use std::path::PathBuf;
    use std::sync::Arc;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("ezd-android-wifi-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn addresses_default_to_the_host_port() {
        assert_eq!(
            parse_addr("192.168.1.5"),
            Some("192.168.1.5:27183".parse().unwrap())
        );
        assert_eq!(
            parse_addr(" 192.168.1.5:4000 "),
            Some("192.168.1.5:4000".parse().unwrap())
        );
        assert_eq!(parse_addr("[fe80::1]:27183").map(|a| a.port()), Some(27183));
        assert_eq!(parse_addr("fe80::1").map(|a| a.port()), Some(27183));
        assert_eq!(parse_addr("not an address"), None);
    }

    #[test]
    fn hosts_json_lists_paired_hosts_with_short_fingerprints() {
        let dir = TempDir::new("hosts");
        assert_eq!(hosts_json(&dir.0), "[]");
        add_host(
            &dir.0,
            StoredHost {
                name: "desk".into(),
                fingerprint: fingerprint_hex(&[0xab; 32]),
                addr: Some("10.0.0.2:27183".into()),
            },
        )
        .unwrap();
        add_host(
            &dir.0,
            StoredHost {
                name: "laptop".into(),
                fingerprint: fingerprint_hex(&[0x01; 32]),
                addr: None,
            },
        )
        .unwrap();
        let v = json(&hosts_json(&dir.0));
        assert_eq!(v[0]["name"], "desk");
        assert_eq!(v[0]["shortFp"], "abab-abab-abab-abab");
        assert_eq!(v[0]["addr"], "10.0.0.2:27183");
        assert_eq!(v[1]["fingerprint"], fingerprint_hex(&[0x01; 32]));
        assert!(v[1].get("addr").is_none());

        assert!(forget(&dir.0, &fingerprint_hex(&[0xab; 32])));
        assert!(!forget(&dir.0, &fingerprint_hex(&[0xab; 32])));
        assert_eq!(json(&hosts_json(&dir.0)).as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_corrupt_store_lists_nothing() {
        let dir = TempDir::new("corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("hosts.json"), "nope").unwrap();
        assert_eq!(hosts_json(&dir.0), "[]");
        assert!(!forget(&dir.0, "00"));
    }

    #[test]
    fn the_identity_is_stable() {
        let dir = TempDir::new("identity");
        let a = identity_short(&dir.0);
        assert_eq!(a.len(), 19);
        assert_eq!(identity_short(&dir.0), a);
    }

    #[test]
    fn bad_input_is_reported_with_a_code() {
        let dir = TempDir::new("badinput");
        let v = json(&pair_json("nowhere", "123456", "tab", &dir.0));
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "invalid_address");
        let v = json(&pair_json("", "ezd://pair?a=bad", "tab", &dir.0));
        assert_eq!(v["error"], "invalid_uri");
        let v = json(&pair_json("127.0.0.1:1", "12", "tab", &dir.0));
        assert_eq!(v["error"], "invalid_pin");
    }

    #[test]
    fn pairing_over_loopback_reports_the_host_and_then_a_wrong_pin() {
        let dir = TempDir::new("pair");
        let host_id = Identity::generate().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (addr, window) = rt.block_on(async {
            let server =
                QuicServer::bind("127.0.0.1:0".parse().unwrap(), &host_id, Congestion::Cubic)
                    .unwrap();
            let addr = server.local_addr().unwrap();
            let window = Arc::new(PairingWindow::default());
            window.open_with("123456".into(), Duration::from_secs(60));
            let w = window.clone();
            tokio::spawn(async move {
                while let Some(accepted) = server.accept().await {
                    let Ok(conn) = accepted else { continue };
                    let w = w.clone();
                    tokio::spawn(async move {
                        if let Ok(opened) = conn.accept_open().await {
                            let _ = host_pair(opened, &w, "test host", |_| Ok(())).await;
                        }
                    });
                }
            });
            (addr, window)
        });

        let v = json(&pair_json(&addr.to_string(), "654321", "tab", &dir.0));
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "wrong_pin");
        assert_eq!(hosts_json(&dir.0), "[]");

        window.open_with("123456".into(), Duration::from_secs(60));
        let v = json(&pair_json(&addr.to_string(), " 123456 ", "tab", &dir.0));
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["host"]["name"], "test host");
        assert_eq!(
            v["host"]["fingerprint"],
            fingerprint_hex(&host_id.fingerprint())
        );
        assert_eq!(
            v["host"]["shortFp"],
            fingerprint_short(&host_id.fingerprint())
        );
        assert_eq!(json(&hosts_json(&dir.0))[0]["name"], "test host");
    }
}
