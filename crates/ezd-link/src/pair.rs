//! PIN pairing over a Wi-Fi link.
//!
//! The host shows a one-time 6-digit PIN (and a QR code with its address and certificate
//! fingerprint). Over a fresh QUIC connection both sides run SPAKE2 with the PIN as the
//! password, then exchange key confirmations: HMACs over a transcript that holds both
//! certificate fingerprints and TLS keying material of this connection. Confirmations only
//! match when both used the same PIN *and* see the same TLS session, so a man in the middle
//! (who must run two TLS sessions) fails, and each attempt tests a single PIN guess. After
//! [`MAX_ATTEMPTS`] attempts the window closes.
//!
//! On success each side pins the other's fingerprint; later connections use
//! [`crate::quic::connect`] with the pinned host fingerprint and skip pairing.

use crate::identity::{fingerprint_hex, parse_fingerprint};
use crate::quic::{Opened, QuicConn, QuicError, finish_and_close, read_msg, write_msg};
use ezd_proto::pair::{
    Fingerprint, LinkOpen, LinkReply, PIN_DIGITS, PairConfirm, PairOutcome, Refusal, is_valid_pin,
};
use ring::hmac;
use spake2::{Ed25519Group, Identity as SpakeId, Password, Spake2};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Pairing attempts allowed per window (each tests one PIN guess).
pub const MAX_ATTEMPTS: u32 = 5;
/// Default lifetime of a pairing window.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Longest device or host name kept from a peer.
pub const MAX_NAME_CHARS: usize = 64;

const SPAKE_ID_CLIENT: &[u8] = b"ezdisplay client";
const SPAKE_ID_HOST: &[u8] = b"ezdisplay host";
const TRANSCRIPT_TAG: &[u8] = b"ezd-pair-v1";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-ezd-pair-v1";
const CONFIRM_HOST: &[u8] = b"host confirm";
const CONFIRM_CLIENT: &[u8] = b"client confirm";

/// A host the client paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedHost {
    /// Host certificate fingerprint to pin.
    pub fingerprint: Fingerprint,
    /// Host name, as announced by the host.
    pub name: String,
}

/// A client the host paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedClient {
    /// Client certificate fingerprint to pin.
    pub fingerprint: Fingerprint,
    /// Device name, as announced by the client.
    pub name: String,
}

/// Why a host-side pairing attempt ended without pairing.
#[derive(Debug, thiserror::Error)]
pub enum HostPairError {
    /// The attempt was refused (closed window, too many attempts, wrong PIN).
    #[error("pairing refused: {0:?}")]
    Refused(Refusal),
    /// The connection failed or the client sent something unexpected.
    #[error(transparent)]
    Link(#[from] QuicError),
    /// Storing the client failed; the client was told the pairing was refused.
    #[error("cannot store the paired device: {0}")]
    Store(std::io::Error),
}

/// Generates a uniformly random PIN of [`PIN_DIGITS`] digits.
pub fn generate_pin() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    // Rejection sampling keeps the digits uniform.
    const LIMIT: u32 = u32::MAX - u32::MAX % 1_000_000;
    loop {
        let mut b = [0u8; 4];
        rng.fill(&mut b).expect("system random source");
        let v = u32::from_le_bytes(b);
        if v < LIMIT {
            return format!("{:0width$}", v % 1_000_000, width = PIN_DIGITS);
        }
    }
}

/// State of a [`PairingWindow`]. Its `Debug` output hides the PIN.
#[derive(Clone, PartialEq, Eq)]
pub enum WindowState {
    /// Accepting attempts with this PIN until `expires`.
    Open {
        /// The PIN.
        pin: String,
        /// When the window closes by itself.
        expires: Instant,
        /// Attempts already started.
        attempts: u32,
    },
    /// Not accepting attempts.
    Closed(CloseReason),
}

impl std::fmt::Debug for WindowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowState::Open {
                expires, attempts, ..
            } => f
                .debug_struct("Open")
                .field("pin", &"<hidden>")
                .field("expires", expires)
                .field("attempts", attempts)
                .finish(),
            WindowState::Closed(why) => f.debug_tuple("Closed").field(why).finish(),
        }
    }
}

/// Why a pairing window is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// Never opened, or closed on request.
    NotOpen,
    /// A device paired.
    Paired,
    /// [`MAX_ATTEMPTS`] attempts were used.
    TooManyAttempts,
    /// It expired.
    Expired,
}

/// The host's pairing window: at most one PIN at a time, [`MAX_ATTEMPTS`] attempts, a
/// deadline. Shared between the accept loop and whoever shows the PIN.
///
/// Every open or close starts a new generation. An attempt only pairs if the window it began
/// in is still current when it finishes, so closing or reopening the window (or its expiry)
/// cancels attempts in flight, and one window pairs at most one device.
#[derive(Debug)]
pub struct PairingWindow {
    state: Mutex<Generation>,
}

#[derive(Debug)]
struct Generation {
    state: WindowState,
    id: u64,
}

impl Default for PairingWindow {
    fn default() -> Self {
        PairingWindow {
            state: Mutex::new(Generation {
                state: WindowState::Closed(CloseReason::NotOpen),
                id: 0,
            }),
        }
    }
}

impl PairingWindow {
    /// Opens (or reopens) the window with a new random PIN; returns the PIN.
    pub fn open(&self, ttl: Duration) -> String {
        let pin = generate_pin();
        self.open_with(pin.clone(), ttl);
        pin
    }

    /// Opens the window with a given PIN (tests).
    pub fn open_with(&self, pin: String, ttl: Duration) {
        let mut g = self.lock();
        g.id += 1;
        g.state = WindowState::Open {
            pin,
            expires: Instant::now() + ttl,
            attempts: 0,
        };
    }

    /// Closes the window and cancels attempts in flight.
    pub fn close(&self) {
        let mut g = self.lock();
        g.id += 1;
        g.state = WindowState::Closed(CloseReason::NotOpen);
    }

    /// Current state (an expired window reads as closed).
    pub fn state(&self) -> WindowState {
        let mut g = self.lock();
        Self::expire(&mut g.state);
        g.state.clone()
    }

    /// True while attempts are accepted.
    pub fn is_open(&self) -> bool {
        matches!(self.state(), WindowState::Open { .. })
    }

    /// Starts an attempt: returns the PIN and the window generation, or why no attempt is
    /// possible. Every started attempt counts, even one the client abandons.
    fn begin(&self) -> Result<(String, u64), Refusal> {
        let mut g = self.lock();
        let id = g.id;
        Self::expire(&mut g.state);
        match &mut g.state {
            WindowState::Open { pin, attempts, .. } => {
                *attempts += 1;
                let pin = pin.clone();
                if *attempts >= MAX_ATTEMPTS {
                    // This is the last one; a success turns it into `Paired`.
                    g.state = WindowState::Closed(CloseReason::TooManyAttempts);
                }
                Ok((pin, id))
            }
            WindowState::Closed(CloseReason::TooManyAttempts) => Err(Refusal::TooManyAttempts),
            WindowState::Closed(_) => Err(Refusal::PairingClosed),
        }
    }

    /// Finishes a successful attempt of generation `id`: if that window is still current and
    /// has not paired or expired, runs `store` and closes the window as paired, all under the
    /// lock (`store` must not use the window). `Ok(false)` when the window moved on.
    fn commit(
        &self,
        id: u64,
        store: impl FnOnce() -> std::io::Result<()>,
    ) -> std::io::Result<bool> {
        let mut g = self.lock();
        Self::expire(&mut g.state);
        let live = g.id == id
            && matches!(
                g.state,
                WindowState::Open { .. } | WindowState::Closed(CloseReason::TooManyAttempts)
            );
        if !live {
            return Ok(false);
        }
        store()?;
        g.state = WindowState::Closed(CloseReason::Paired);
        Ok(true)
    }

    fn expire(s: &mut WindowState) {
        if let WindowState::Open { expires, .. } = s
            && Instant::now() >= *expires
        {
            *s = WindowState::Closed(CloseReason::Expired);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Generation> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Transcript both confirmations cover.
fn transcript(
    conn: &QuicConn,
    host_fp: &Fingerprint,
    client_fp: &Fingerprint,
) -> Result<Vec<u8>, QuicError> {
    let exported = conn.export(EXPORTER_LABEL)?;
    let mut t = Vec::with_capacity(TRANSCRIPT_TAG.len() + 96);
    t.extend_from_slice(TRANSCRIPT_TAG);
    t.extend_from_slice(&exported);
    t.extend_from_slice(host_fp);
    t.extend_from_slice(client_fp);
    Ok(t)
}

/// Confirmation key for one side, derived from the SPAKE2 key.
fn confirm_key(spake_key: &[u8], side: &[u8]) -> hmac::Key {
    let k = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, spake_key), side);
    hmac::Key::new(hmac::HMAC_SHA256, k.as_ref())
}

fn confirm_tag(spake_key: &[u8], side: &[u8], transcript: &[u8]) -> [u8; 32] {
    let tag = hmac::sign(&confirm_key(spake_key, side), transcript);
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

fn confirm_ok(spake_key: &[u8], side: &[u8], transcript: &[u8], tag: &[u8; 32]) -> bool {
    // Constant time.
    hmac::verify(&confirm_key(spake_key, side), transcript, tag).is_ok()
}

/// Keeps printable characters and at most [`MAX_NAME_CHARS`] of them. Bidirectional
/// overrides and invisible format characters are dropped so a name cannot disguise itself.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|&c| !c.is_control() && !is_format_char(c))
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Invisible or direction-changing format characters (Unicode `Cf` ranges that matter for
/// display spoofing).
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// Client: pairs over `conn` (from [`crate::quic::connect`]) with the PIN shown by the host.
/// On success the caller pins [`PairedHost::fingerprint`] and opens sessions on new
/// connections.
pub async fn pair(conn: QuicConn, pin: &str, device_name: &str) -> Result<PairedHost, QuicError> {
    if !is_valid_pin(pin) {
        return Err(QuicError::InvalidPin);
    }
    let host_fp = conn
        .peer_fingerprint()
        .ok_or(QuicError::Unexpected("no host certificate"))?;
    let (state, msg_a) = Spake2::<Ed25519Group>::start_a(
        &Password::new(pin.as_bytes()),
        &SpakeId::new(SPAKE_ID_CLIENT),
        &SpakeId::new(SPAKE_ID_HOST),
    );
    let (mut send, mut recv) = conn.open_bi().await?;
    write_msg(
        &mut send,
        &LinkOpen::Pair {
            device_name: clean_name(device_name),
            spake: msg_a,
        },
    )
    .await?;
    let (host_name, msg_b, host_confirm) = match read_msg::<LinkReply>(&mut recv).await? {
        LinkReply::PairChallenge {
            host_name,
            spake,
            confirm,
        } => (host_name, spake, confirm),
        LinkReply::Refused(r) => return Err(QuicError::Refused(r)),
        LinkReply::Accepted => return Err(QuicError::Unexpected("session reply to pairing")),
    };
    let t = transcript(&conn, &host_fp, &conn.local_fingerprint())?;
    let key = state.finish(&msg_b);
    let Some(key) = key
        .ok()
        .filter(|k| confirm_ok(k, CONFIRM_HOST, &t, &host_confirm))
    else {
        // Wrong PIN, or someone in the middle. Tell the host nothing it could test offline.
        conn.close("pairing failed");
        return Err(QuicError::Refused(Refusal::WrongPin));
    };
    write_msg(
        &mut send,
        &PairConfirm {
            confirm: confirm_tag(&key, CONFIRM_CLIENT, &t),
        },
    )
    .await?;
    let outcome = read_msg::<PairOutcome>(&mut recv).await?;
    finish_and_close(&mut send, &conn, "paired").await;
    match outcome {
        PairOutcome::Paired => Ok(PairedHost {
            fingerprint: host_fp,
            name: clean_name(&host_name),
        }),
        PairOutcome::Refused(r) => Err(QuicError::Refused(r)),
    }
}

/// Host: runs a pairing attempt for a client whose [`LinkOpen::Pair`] was read. `store` is
/// called with the new client before the client is told it is paired; if it fails the client
/// is refused.
pub async fn host_pair(
    opened: Opened,
    window: &PairingWindow,
    host_name: &str,
    store: impl FnOnce(&PairedClient) -> std::io::Result<()>,
) -> Result<PairedClient, HostPairError> {
    let LinkOpen::Pair {
        device_name,
        spake: msg_a,
    } = opened.open.clone()
    else {
        return Err(QuicError::Unexpected("not a pairing request").into());
    };
    let Some(client_fp) = opened.peer_fingerprint() else {
        return Err(QuicError::Unexpected("no client certificate").into());
    };
    let (pin, generation) = match window.begin() {
        Ok(begun) => begun,
        Err(why) => {
            opened.refuse(why).await;
            return Err(HostPairError::Refused(why));
        }
    };
    let Opened {
        conn,
        mut send,
        mut recv,
        ..
    } = opened;
    let (state, msg_b) = Spake2::<Ed25519Group>::start_b(
        &Password::new(pin.as_bytes()),
        &SpakeId::new(SPAKE_ID_CLIENT),
        &SpakeId::new(SPAKE_ID_HOST),
    );
    let Ok(key) = state.finish(&msg_a) else {
        let _ = write_msg(&mut send, &LinkReply::Refused(Refusal::WrongPin)).await;
        finish_and_close(&mut send, &conn, "pairing failed").await;
        return Err(HostPairError::Refused(Refusal::WrongPin));
    };
    let t = transcript(&conn, &conn.local_fingerprint(), &client_fp)?;
    write_msg(
        &mut send,
        &LinkReply::PairChallenge {
            host_name: clean_name(host_name),
            spake: msg_b,
            confirm: confirm_tag(&key, CONFIRM_HOST, &t),
        },
    )
    .await?;
    // A client with the wrong PIN closes the connection here.
    let confirm = read_msg::<PairConfirm>(&mut recv).await?;
    if !confirm_ok(&key, CONFIRM_CLIENT, &t, &confirm.confirm) {
        let _ = write_msg(&mut send, &PairOutcome::Refused(Refusal::WrongPin)).await;
        finish_and_close(&mut send, &conn, "pairing failed").await;
        return Err(HostPairError::Refused(Refusal::WrongPin));
    }
    let client = PairedClient {
        fingerprint: client_fp,
        name: clean_name(&device_name),
    };
    let refusal = match window.commit(generation, || store(&client)) {
        Ok(true) => None,
        // Closed, reopened or expired while this attempt ran, or another device paired.
        Ok(false) => Some(HostPairError::Refused(Refusal::PairingClosed)),
        Err(e) => Some(HostPairError::Store(e)),
    };
    if let Some(err) = refusal {
        let _ = write_msg(&mut send, &PairOutcome::Refused(Refusal::PairingClosed)).await;
        finish_and_close(&mut send, &conn, "pairing failed").await;
        return Err(err);
    }
    write_msg(&mut send, &PairOutcome::Paired).await?;
    finish_and_close(&mut send, &conn, "paired").await;
    Ok(client)
}

/// Contents of the host's pairing QR code:
/// `ezd://pair?a=<ip:port>&fp=<hex>&pin=<pin>&n=<percent-encoded name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairUri {
    /// Host address.
    pub addr: SocketAddr,
    /// Host certificate fingerprint; the client requires it in the TLS handshake.
    pub fingerprint: Fingerprint,
    /// The current PIN.
    pub pin: String,
    /// Host name.
    pub name: String,
}

const URI_PREFIX: &str = "ezd://pair?";

impl PairUri {
    /// Formats the URI.
    pub fn to_uri(&self) -> String {
        format!(
            "{URI_PREFIX}a={}&fp={}&pin={}&n={}",
            self.addr,
            fingerprint_hex(&self.fingerprint),
            self.pin,
            percent_encode(&self.name)
        )
    }

    /// Parses a URI; unknown parameters are ignored.
    pub fn parse(uri: &str) -> Option<PairUri> {
        let query = uri.trim().strip_prefix(URI_PREFIX)?;
        let (mut addr, mut fp, mut pin, mut name) = (None, None, None, String::new());
        for kv in query.split('&') {
            let (k, v) = kv.split_once('=')?;
            match k {
                "a" => addr = v.parse().ok(),
                "fp" => fp = parse_fingerprint(v),
                "pin" if is_valid_pin(v) => pin = Some(v.to_string()),
                "n" => name = clean_name(&percent_decode(v)?),
                _ => {}
            }
        }
        Some(PairUri {
            addr: addr?,
            fingerprint: fp?,
            pin: pin?,
            name,
        })
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = (bytes.next()? as char).to_digit(16)?;
            let lo = (bytes.next()? as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;
    use crate::quic::{Congestion, QuicServer, connect};
    use std::sync::Arc;

    #[test]
    fn pins_are_six_digits() {
        for _ in 0..100 {
            assert!(is_valid_pin(&generate_pin()));
        }
    }

    #[test]
    fn uri_round_trip() {
        let u = PairUri {
            addr: "192.168.0.5:27183".parse().unwrap(),
            fingerprint: [0xab; 32],
            pin: "004213".into(),
            name: "Estación & PC".into(),
        };
        let s = u.to_uri();
        assert!(s.starts_with("ezd://pair?a=192.168.0.5:27183&fp=abab"));
        assert_eq!(PairUri::parse(&s), Some(u));
        assert_eq!(PairUri::parse("https://example.com"), None);
        assert_eq!(PairUri::parse(&s.replace("pin=004213", "pin=42")), None);
    }

    #[test]
    fn window_counts_attempts_and_expires() {
        let w = PairingWindow::default();
        assert_eq!(w.begin(), Err(Refusal::PairingClosed));
        w.open_with("111111".into(), Duration::from_secs(60));
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(w.begin().map(|(pin, _)| pin).as_deref(), Ok("111111"));
        }
        assert_eq!(w.begin(), Err(Refusal::TooManyAttempts));
        w.open_with("222222".into(), Duration::ZERO);
        assert_eq!(w.state(), WindowState::Closed(CloseReason::Expired));
        assert_eq!(w.begin(), Err(Refusal::PairingClosed));
    }

    #[test]
    fn last_attempt_can_still_pair() {
        let w = PairingWindow::default();
        w.open_with("111111".into(), Duration::from_secs(60));
        let ids: Vec<u64> = (0..MAX_ATTEMPTS).map(|_| w.begin().unwrap().1).collect();
        assert!(w.commit(ids[4], || Ok(())).unwrap());
        assert_eq!(w.state(), WindowState::Closed(CloseReason::Paired));
        // One window pairs one device.
        assert!(!w.commit(ids[3], || panic!("stored twice")).unwrap());
    }

    #[test]
    fn closing_or_reopening_cancels_attempts_in_flight() {
        let w = PairingWindow::default();
        w.open_with("111111".into(), Duration::from_secs(60));
        let (_, old) = w.begin().unwrap();
        w.close();
        assert!(!w.commit(old, || panic!("stored after close")).unwrap());

        w.open_with("222222".into(), Duration::from_secs(60));
        let (_, old) = w.begin().unwrap();
        w.open_with("333333".into(), Duration::from_secs(60));
        assert!(!w.commit(old, || panic!("stored after reopen")).unwrap());
        assert!(w.is_open(), "a stale attempt must not close the new window");

        w.open_with("444444".into(), Duration::from_millis(1));
        let (_, old) = w.begin().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(!w.commit(old, || panic!("stored after expiry")).unwrap());
    }

    #[test]
    fn failed_store_keeps_the_window_open() {
        let w = PairingWindow::default();
        w.open_with("111111".into(), Duration::from_secs(60));
        let (_, id) = w.begin().unwrap();
        assert!(
            w.commit(id, || Err(std::io::Error::other("disk full")))
                .is_err()
        );
        assert!(w.is_open());
    }

    #[test]
    fn debug_hides_the_pin() {
        let w = PairingWindow::default();
        w.open_with("987654".into(), Duration::from_secs(60));
        assert!(!format!("{w:?} {:?}", w.state()).contains("987654"));
    }

    #[test]
    fn names_lose_invisible_and_bidi_characters() {
        assert_eq!(clean_name("  tab\u{202E}let\u{200B}\u{7}  "), "tablet");
        assert_eq!(clean_name(&"é".repeat(100)).chars().count(), MAX_NAME_CHARS);
    }

    struct Fixture {
        server: Arc<QuicServer>,
        addr: SocketAddr,
        host: Identity,
        client: Identity,
        window: Arc<PairingWindow>,
    }

    fn fixture() -> Fixture {
        let host = Identity::generate().unwrap();
        let server =
            QuicServer::bind("127.0.0.1:0".parse().unwrap(), &host, Congestion::default()).unwrap();
        let addr = server.local_addr().unwrap();
        Fixture {
            server: Arc::new(server),
            addr,
            host,
            client: Identity::generate().unwrap(),
            window: Arc::new(PairingWindow::default()),
        }
    }

    /// Host side of one connection: pairs, or accepts a session if the client is `paired`.
    async fn host_once(
        f: &Fixture,
        paired: Option<Fingerprint>,
    ) -> Result<Option<PairedClient>, HostPairError> {
        let conn = f.server.accept().await.unwrap()?;
        let opened = conn.accept_open().await?;
        match opened.open {
            LinkOpen::Session => {
                if opened.peer_fingerprint() == paired && paired.is_some() {
                    let link = opened.accept_session().await?;
                    let (tx, mut rx, _t) = link.into_mux(1024);
                    tx.send(&ezd_proto::Message::Ping(ezd_proto::Ping {
                        seq: 1,
                        t0_us: 7,
                    }))
                    .unwrap();
                    // Waits for the client's Bye so the connection outlives the exchange.
                    let _ = rx.recv().await;
                } else {
                    opened.refuse(Refusal::NotPaired).await;
                }
                Ok(None)
            }
            LinkOpen::Pair { .. } => host_pair(opened, &f.window, "test host", |_| Ok(()))
                .await
                .map(Some),
        }
    }

    /// Client side of one pairing connection.
    async fn client_pair(f: &Fixture, pin: &str) -> Result<PairedHost, QuicError> {
        let conn = connect(f.addr, &f.client, None, Congestion::default()).await?;
        pair(conn, pin, "tablet").await
    }

    #[tokio::test]
    async fn pairing_then_session_without_pin() {
        let f = fixture();
        f.window.open_with("123456".into(), Duration::from_secs(60));
        let (host, client) = tokio::join!(host_once(&f, None), client_pair(&f, "123456"));
        let host = host.unwrap().unwrap();
        let client = client.unwrap();
        assert_eq!(host.fingerprint, f.client.fingerprint());
        assert_eq!(host.name, "tablet");
        assert_eq!(client.fingerprint, f.host.fingerprint());
        assert_eq!(client.name, "test host");
        assert_eq!(f.window.state(), WindowState::Closed(CloseReason::Paired));

        // Reconnect with the pinned host certificate: no PIN needed.
        let session = async {
            let conn = connect(f.addr, &f.client, Some(client.fingerprint), Congestion::Bbr)
                .await
                .unwrap();
            let link = conn.open_session().await.unwrap();
            assert!(link.has_video_stream());
            let (tx, mut rx, _t) = link.into_mux(1024);
            let m = rx.recv().await.unwrap().unwrap();
            tx.send(&ezd_proto::Message::Bye(ezd_proto::Bye {
                reason: "done".into(),
            }))
            .unwrap();
            m
        };
        let (h, m) = tokio::join!(host_once(&f, Some(host.fingerprint)), session);
        h.unwrap();
        assert_eq!(
            m,
            ezd_proto::Message::Ping(ezd_proto::Ping { seq: 1, t0_us: 7 })
        );
    }

    #[tokio::test]
    async fn wrong_pin_is_refused_and_attempts_run_out() {
        let f = fixture();
        f.window.open_with("123456".into(), Duration::from_secs(60));
        for i in 0..MAX_ATTEMPTS {
            let (host, client) = tokio::join!(host_once(&f, None), client_pair(&f, "654321"));
            assert!(
                matches!(client, Err(QuicError::Refused(Refusal::WrongPin))),
                "attempt {i}: {client:?}"
            );
            assert!(host.is_err());
        }
        // Even the right PIN is refused now.
        let (_, client) = tokio::join!(host_once(&f, None), client_pair(&f, "123456"));
        assert!(matches!(
            client,
            Err(QuicError::Refused(Refusal::TooManyAttempts))
        ));
    }

    #[tokio::test]
    async fn closed_window_refuses_pairing() {
        let f = fixture();
        let (_, client) = tokio::join!(host_once(&f, None), client_pair(&f, "123456"));
        assert!(matches!(
            client,
            Err(QuicError::Refused(Refusal::PairingClosed))
        ));
    }

    #[tokio::test]
    async fn unpaired_session_is_refused() {
        let f = fixture();
        let session = async {
            connect(
                f.addr,
                &f.client,
                Some(f.host.fingerprint()),
                Congestion::default(),
            )
            .await?
            .open_session()
            .await
        };
        let (_, client) = tokio::join!(host_once(&f, None), session);
        assert!(matches!(
            client,
            Err(QuicError::Refused(Refusal::NotPaired))
        ));
    }

    #[tokio::test]
    async fn unpinned_connection_cannot_open_a_session() {
        let f = fixture();
        let server = f.server.clone();
        tokio::spawn(async move { while server.accept().await.is_some() {} });
        let err = connect(f.addr, &f.client, None, Congestion::default())
            .await
            .unwrap()
            .open_session()
            .await
            .unwrap_err();
        assert!(matches!(err, QuicError::Unexpected(_)), "{err:?}");
    }

    #[tokio::test]
    async fn pinned_fingerprint_mismatch_fails_the_handshake() {
        let f = fixture();
        let server = f.server.clone();
        // The host handshake fails too; just drain it.
        tokio::spawn(async move { while server.accept().await.is_some() {} });
        let err = connect(f.addr, &f.client, Some([0; 32]), Congestion::default())
            .await
            .unwrap_err();
        assert!(matches!(err, QuicError::FingerprintMismatch), "{err:?}");
    }
}
