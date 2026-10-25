//! Wi-Fi links over QUIC.
//!
//! Each session (or pairing attempt) is one QUIC connection with mutual TLS 1.3. Both sides use
//! a self-signed [`Identity`]; there is no CA. The client checks the host certificate against
//! the pinned fingerprint (or accepts any during PIN-only pairing, which SPAKE2 then
//! authenticates); the host accepts any client certificate in the handshake and decides by its
//! fingerprint afterwards. TLS still verifies that each peer holds its certificate's key.
//!
//! Streams (see `docs/protocol.md`):
//! * control, bidirectional, opened by the client: [`LinkOpen`] / [`LinkReply`], then the framed
//!   control traffic;
//! * cursor and video, unidirectional, opened by the host after [`LinkReply::Accepted`];
//! * input, unidirectional, opened by the client after it read [`LinkReply::Accepted`].
//!
//! Every unidirectional stream starts with one tag byte ([`CURSOR_STREAM_TAG`],
//! [`VIDEO_STREAM_TAG`], [`INPUT_STREAM_TAG`]) and then carries the framed messages of its
//! channel. The receiver dispatches by tag, not by arrival order. Audio will be one more stream
//! ([`AUDIO_STREAM_TAG`] is reserved).
//!
//! Send priorities (higher first): control 4, input 3, cursor 2, audio 1 (reserved), video 0.
//! Each channel has its own stream, so a lost packet delays only the channel it belongs to.

use crate::identity::{Identity, fingerprint_of};
use crate::link::{BoxRead, BoxWrite, Lanes, Link};
use ezd_proto::ProtoError;
use ezd_proto::pair::{
    self, ALPN, CURSOR_STREAM_TAG, Fingerprint, INPUT_STREAM_TAG, LinkOpen, LinkReply,
    MAX_PREAMBLE_LEN, Refusal, VIDEO_STREAM_TAG,
};
use quinn::{Connection, Endpoint, RecvStream, SendStream, VarInt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};

/// TLS server name. Certificates are checked by fingerprint, so it is only a placeholder.
const SERVER_NAME: &str = "ezdisplay";
/// Time allowed for each preamble step (open, reply, lane streams).
pub const PREAMBLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Application close code for a normal end or a refusal.
const CLOSE_OK: u32 = 0;
/// Send priorities (higher is served first). 1 is left for audio, which slots between cursor
/// and video.
const CONTROL_PRIORITY: i32 = 4;
const INPUT_PRIORITY: i32 = 3;
const CURSOR_PRIORITY: i32 = 2;
const VIDEO_PRIORITY: i32 = 0;

/// Congestion controller of a Wi-Fi link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Congestion {
    /// CUBIC (quinn's default).
    #[default]
    Cubic,
    /// BBR (experimental in quinn); can keep queues shorter on a busy Wi-Fi channel.
    Bbr,
}

/// Errors of a Wi-Fi link.
#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    /// The connection could not be started (bad address, endpoint stopped).
    #[error("cannot connect: {0}")]
    Connect(#[from] quinn::ConnectError),
    /// The connection failed or was closed.
    #[error("connection failed: {0}")]
    Connection(#[from] quinn::ConnectionError),
    /// The host certificate is not the expected (paired or scanned) one.
    #[error("the host certificate does not match the paired one")]
    FingerprintMismatch,
    /// The host refused the session or the pairing attempt.
    #[error("refused by the host: {0:?}")]
    Refused(Refusal),
    /// The PIN is not 6 digits.
    #[error("the PIN must be 6 digits")]
    InvalidPin,
    /// A preamble message was not the expected one.
    #[error("unexpected message from the peer: {0}")]
    Unexpected(&'static str),
    /// A preamble step took longer than [`PREAMBLE_TIMEOUT`].
    #[error("timed out")]
    Timeout,
    /// TLS or endpoint configuration failed.
    #[error("TLS setup failed: {0}")]
    Tls(String),
    /// Stream I/O failure.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// Malformed preamble.
    #[error("protocol error: {0}")]
    Proto(#[from] ProtoError),
}

impl From<quinn::WriteError> for QuicError {
    fn from(e: quinn::WriteError) -> Self {
        QuicError::Io(e.into())
    }
}

impl From<quinn::ReadExactError> for QuicError {
    fn from(e: quinn::ReadExactError) -> Self {
        QuicError::Io(match e {
            quinn::ReadExactError::FinishedEarly(_) => io::ErrorKind::UnexpectedEof.into(),
            quinn::ReadExactError::ReadError(e) => e.into(),
        })
    }
}

fn tls_err(e: impl std::fmt::Display) -> QuicError {
    QuicError::Tls(e.to_string())
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn cert_and_key(id: &Identity) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    (
        vec![CertificateDer::from(id.cert_der().to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_der().to_vec())),
    )
}

/// Most QUIC connections the host keeps at once, handshakes included (see
/// [`QuicServer::open_connections`]).
pub const MAX_HOST_CONNECTIONS: usize = 8;

/// Which side a transport configuration is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Host,
    Client,
}

/// Transport settings for a low-latency LAN link.
///
/// Only the host receives from peers it has not authenticated yet (it accepts any client
/// certificate), so its receive windows are small: it only reads the preamble and input, never
/// video. The client receives video and needs room for a large keyframe in flight. Each side
/// allows exactly the streams the other opens: one bidirectional stream and one unidirectional
/// stream (input) from the client, two unidirectional streams (cursor, video) from the host.
fn transport(cc: Congestion, side: Side) -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(1)))
        .max_idle_timeout(quinn::IdleTimeout::try_from(Duration::from_secs(5)).ok())
        // A LAN round trip is a few ms; quinn's 333 ms default slows the first loss recovery.
        .initial_rtt(Duration::from_millis(10))
        .send_window(16 << 20)
        .datagram_receive_buffer_size(None);
    match side {
        Side::Host => t
            .stream_receive_window(VarInt::from_u32(256 << 10))
            .receive_window(VarInt::from_u32(256 << 10))
            .max_concurrent_bidi_streams(VarInt::from_u32(1))
            .max_concurrent_uni_streams(VarInt::from_u32(1)),
        Side::Client => t
            .stream_receive_window(VarInt::from_u32(8 << 20))
            .receive_window(VarInt::from_u32(16 << 20))
            .max_concurrent_bidi_streams(VarInt::from_u32(0))
            .max_concurrent_uni_streams(VarInt::from_u32(2)),
    };
    if cc == Congestion::Bbr {
        t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    }
    t
}

/// Accepts the host certificate if it matches `expected` (any when `None`).
#[derive(Debug)]
struct ServerCheck {
    expected: Option<Fingerprint>,
    mismatch: Arc<AtomicBool>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for ServerCheck {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Some(fp) = self.expected
            && fingerprint_of(end_entity) != fp
        {
            self.mismatch.store(true, Ordering::Relaxed);
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Requires a client certificate but accepts any; the host checks its fingerprint after the
/// handshake ([`Opened::peer_fingerprint`]).
#[derive(Debug)]
struct AnyClient {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// An established QUIC connection plus what is needed to keep it alive.
#[derive(Debug, Clone)]
pub struct QuicConn {
    conn: Connection,
    /// The client's own endpoint; the host's lives in [`QuicServer`].
    _endpoint: Option<Endpoint>,
    local_fp: Fingerprint,
    /// Client: the host certificate was checked against a pinned fingerprint.
    pinned: bool,
}

impl QuicConn {
    /// Fingerprint of the peer certificate.
    pub fn peer_fingerprint(&self) -> Option<Fingerprint> {
        let certs = self
            .conn
            .peer_identity()?
            .downcast::<Vec<CertificateDer<'static>>>()
            .ok()?;
        certs.first().map(|c| fingerprint_of(c))
    }

    /// Fingerprint of our own certificate.
    pub fn local_fingerprint(&self) -> Fingerprint {
        self.local_fp
    }

    /// Address of the peer.
    pub fn remote_address(&self) -> SocketAddr {
        self.conn.remote_address()
    }

    /// Closes the connection at once.
    pub fn close(&self, reason: &str) {
        self.conn.close(CLOSE_OK.into(), reason.as_bytes());
    }

    /// 32 bytes of TLS keying material for `label` (RFC 5705), unique to this connection.
    pub(crate) fn export(&self, label: &[u8]) -> Result<[u8; 32], QuicError> {
        let mut out = [0u8; 32];
        self.conn
            .export_keying_material(&mut out, label, b"")
            .map_err(|_| QuicError::Tls("keying material export failed".into()))?;
        Ok(out)
    }

    pub(crate) async fn open_bi(&self) -> Result<(SendStream, RecvStream), QuicError> {
        let (send, recv) = self.conn.open_bi().await?;
        let _ = send.set_priority(CONTROL_PRIORITY);
        Ok((send, recv))
    }

    /// Client: asks for a session on a paired host and returns the two-stream link.
    /// The connection must come from [`connect`] with a pinned fingerprint: an unpinned one
    /// is only authenticated by [`crate::pair::pair`].
    pub async fn open_session(self) -> Result<Link, QuicError> {
        if !self.pinned {
            return Err(QuicError::Unexpected(
                "session on a connection without a pinned host",
            ));
        }
        let (mut send, mut recv) = self.open_bi().await?;
        write_msg(&mut send, &LinkOpen::Session).await?;
        match read_msg::<LinkReply>(&mut recv).await? {
            LinkReply::Accepted => {}
            LinkReply::Refused(r) => return Err(QuicError::Refused(r)),
            LinkReply::PairChallenge { .. } => {
                return Err(QuicError::Unexpected("pairing reply to a session request"));
            }
        }
        let input = open_tagged(&self.conn, INPUT_STREAM_TAG, INPUT_PRIORITY).await?;
        let [cursor, video] =
            accept_tagged(&self.conn, [CURSOR_STREAM_TAG, VIDEO_STREAM_TAG]).await?;
        let peer = format!("quic:{}", self.conn.remote_address());
        let keep = Arc::new(Lingering(self));
        let read = Lanes {
            input: None,
            cursor: Some(Box::new(Keep::new(cursor, &keep)) as BoxRead),
            video: Some(Box::new(Keep::new(video, &keep)) as BoxRead),
        };
        let write = Lanes {
            input: Some(Box::new(input) as BoxWrite),
            cursor: None,
            video: None,
        };
        Ok(
            Link::new(Box::new(Keep::new(recv, &keep)), Box::new(send), peer)
                .with_lanes(read, write),
        )
    }

    /// Host: reads the client's [`LinkOpen`] on the control stream.
    pub async fn accept_open(self) -> Result<Opened, QuicError> {
        let (send, mut recv) = tokio::time::timeout(PREAMBLE_TIMEOUT, self.conn.accept_bi())
            .await
            .map_err(|_| QuicError::Timeout)??;
        let _ = send.set_priority(CONTROL_PRIORITY);
        let open = read_msg::<LinkOpen>(&mut recv).await?;
        Ok(Opened {
            conn: self,
            open,
            send,
            recv,
        })
    }
}

/// Host side of a connection whose [`LinkOpen`] was read.
#[derive(Debug)]
pub struct Opened {
    pub(crate) conn: QuicConn,
    /// What the client asked for.
    pub open: LinkOpen,
    pub(crate) send: SendStream,
    pub(crate) recv: RecvStream,
}

impl Opened {
    /// Fingerprint of the client certificate.
    pub fn peer_fingerprint(&self) -> Option<Fingerprint> {
        self.conn.peer_fingerprint()
    }

    /// Address of the client.
    pub fn remote_address(&self) -> SocketAddr {
        self.conn.remote_address()
    }

    /// Accepts a session: answers [`LinkReply::Accepted`], opens the cursor and video streams,
    /// accepts the client's input stream and returns the link.
    pub async fn accept_session(mut self) -> Result<Link, QuicError> {
        write_msg(&mut self.send, &LinkReply::Accepted).await?;
        let cursor = open_tagged(&self.conn.conn, CURSOR_STREAM_TAG, CURSOR_PRIORITY).await?;
        let video = open_tagged(&self.conn.conn, VIDEO_STREAM_TAG, VIDEO_PRIORITY).await?;
        let [input] = accept_tagged(&self.conn.conn, [INPUT_STREAM_TAG]).await?;
        let peer = format!("quic:{}", self.conn.remote_address());
        let keep = Arc::new(Lingering(self.conn));
        let read = Lanes {
            input: Some(Box::new(Keep::new(input, &keep)) as BoxRead),
            cursor: None,
            video: None,
        };
        let write = Lanes {
            input: None,
            cursor: Some(Box::new(cursor) as BoxWrite),
            video: Some(Box::new(video) as BoxWrite),
        };
        Ok(Link::new(
            Box::new(Keep::new(self.recv, &keep)),
            Box::new(self.send),
            peer,
        )
        .with_lanes(read, write))
    }

    /// Refuses the request and closes the connection once the answer is delivered (or after
    /// a short wait).
    pub async fn refuse(mut self, why: Refusal) {
        let _ = write_msg(&mut self.send, &LinkReply::Refused(why)).await;
        finish_and_close(&mut self.send, &self.conn, "refused").await;
    }
}

/// Opens a unidirectional stream with the given send priority and writes its tag byte (which
/// also makes the stream visible to the peer).
async fn open_tagged(conn: &Connection, tag: u8, priority: i32) -> Result<SendStream, QuicError> {
    let mut stream = tokio::time::timeout(PREAMBLE_TIMEOUT, conn.open_uni())
        .await
        .map_err(|_| QuicError::Timeout)??;
    let _ = stream.set_priority(priority);
    stream.write_all(&[tag]).await?;
    Ok(stream)
}

/// Accepts `N` unidirectional streams and returns them in the order of `tags`, whatever order
/// they arrive in. An unknown or repeated tag is an error. The whole exchange must finish within
/// [`PREAMBLE_TIMEOUT`].
async fn accept_tagged<const N: usize>(
    conn: &Connection,
    tags: [u8; N],
) -> Result<[RecvStream; N], QuicError> {
    let accept = async {
        let mut found: [Option<RecvStream>; N] = std::array::from_fn(|_| None);
        for _ in 0..N {
            let mut stream = conn.accept_uni().await?;
            let mut tag = [0u8; 1];
            stream.read_exact(&mut tag).await?;
            let slot = tags
                .iter()
                .position(|&t| t == tag[0])
                .ok_or(QuicError::Unexpected("unknown stream tag"))?;
            if found[slot].replace(stream).is_some() {
                return Err(QuicError::Unexpected("duplicate stream tag"));
            }
        }
        Ok::<_, QuicError>(found)
    };
    let found = tokio::time::timeout(PREAMBLE_TIMEOUT, accept)
        .await
        .map_err(|_| QuicError::Timeout)??;
    Ok(found.map(|s| s.expect("every tag was filled once")))
}

/// Finishes `send`, waits briefly for the peer to read it, then closes the connection.
pub(crate) async fn finish_and_close(send: &mut SendStream, conn: &QuicConn, reason: &str) {
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(1), send.stopped()).await;
    conn.close(reason);
}

/// Host endpoint listening for Wi-Fi clients.
#[derive(Debug)]
pub struct QuicServer {
    endpoint: Endpoint,
    local_fp: Fingerprint,
}

impl QuicServer {
    /// Binds a UDP socket on `addr` with the host `identity`.
    pub fn bind(addr: SocketAddr, identity: &Identity, cc: Congestion) -> Result<Self, QuicError> {
        let p = provider();
        let (certs, key) = cert_and_key(identity);
        let mut tls = rustls::ServerConfig::builder_with_provider(p.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(tls_err)?
            .with_client_cert_verifier(Arc::new(AnyClient { provider: p }))
            .with_single_cert(certs, key)
            .map_err(tls_err)?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(tls_err)?;
        let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        cfg.transport_config(Arc::new(transport(cc, Side::Host)));
        Ok(QuicServer {
            endpoint: Endpoint::server(cfg, addr)?,
            local_fp: identity.fingerprint(),
        })
    }

    /// The bound address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// Waits for the next incoming connection without completing its handshake, so a slow
    /// client never delays the next one: complete it with [`QuicIncoming::handshake`] in its
    /// own task. `None` once the endpoint is closed.
    pub async fn accept_incoming(&self) -> Option<QuicIncoming> {
        let incoming = self.endpoint.accept().await?;
        Some(QuicIncoming {
            incoming,
            local_fp: self.local_fp,
        })
    }

    /// Waits for the next client handshake; `None` once the endpoint is closed. A failed
    /// handshake is returned as an error and the server keeps accepting.
    pub async fn accept(&self) -> Option<Result<QuicConn, QuicError>> {
        Some(self.accept_incoming().await?.handshake().await)
    }

    /// Connections currently open on the endpoint, handshakes included. Refuse new ones
    /// ([`QuicIncoming::refuse`]) above [`MAX_HOST_CONNECTIONS`].
    pub fn open_connections(&self) -> usize {
        self.endpoint.open_connections()
    }

    /// Waits up to `limit` for every connection to finish (sessions that just ended deliver
    /// their last messages), for a clean shutdown before [`close`](Self::close).
    pub async fn wait_idle(&self, limit: Duration) {
        let _ = tokio::time::timeout(limit, self.endpoint.wait_idle()).await;
    }

    /// Stops accepting and closes every connection.
    pub fn close(&self) {
        self.endpoint.close(CLOSE_OK.into(), b"host stopped");
    }
}

/// A connection attempt that reached the host and has not finished its handshake.
/// Dropping it refuses the attempt.
#[derive(Debug)]
pub struct QuicIncoming {
    incoming: quinn::Incoming,
    local_fp: Fingerprint,
}

impl QuicIncoming {
    /// Address of the peer (not yet validated).
    pub fn remote_address(&self) -> SocketAddr {
        self.incoming.remote_address()
    }

    /// Completes the handshake within [`PREAMBLE_TIMEOUT`].
    pub async fn handshake(self) -> Result<QuicConn, QuicError> {
        let conn = tokio::time::timeout(PREAMBLE_TIMEOUT, self.incoming)
            .await
            .map_err(|_| QuicError::Timeout)??;
        Ok(QuicConn {
            conn,
            _endpoint: None,
            local_fp: self.local_fp,
            pinned: false,
        })
    }

    /// Refuses the attempt (host busy or over its connection limit).
    pub fn refuse(self) {
        self.incoming.refuse();
    }
}

/// Client: connects to a host. With `expected` the host certificate must have that
/// fingerprint ([`QuicError::FingerprintMismatch`] otherwise); without it any certificate is
/// accepted, which is only safe for [`crate::pair::pair`]; [`QuicConn::open_session`] refuses
/// such a connection.
pub async fn connect(
    addr: SocketAddr,
    identity: &Identity,
    expected: Option<Fingerprint>,
    cc: Congestion,
) -> Result<QuicConn, QuicError> {
    let p = provider();
    let mismatch = Arc::new(AtomicBool::new(false));
    let (certs, key) = cert_and_key(identity);
    let mut tls = rustls::ClientConfig::builder_with_provider(p.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_err)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ServerCheck {
            expected,
            mismatch: mismatch.clone(),
            provider: p,
        }))
        .with_client_auth_cert(certs, key)
        .map_err(tls_err)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(tls_err)?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(Arc::new(transport(cc, Side::Client)));
    let bind: SocketAddr = if addr.is_ipv6() {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    } else {
        (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    };
    let endpoint = Endpoint::client(bind)?;
    let conn = match endpoint.connect_with(cfg, addr, SERVER_NAME)?.await {
        Ok(c) => c,
        Err(_) if mismatch.load(Ordering::Relaxed) => return Err(QuicError::FingerprintMismatch),
        Err(e) => return Err(e.into()),
    };
    Ok(QuicConn {
        conn,
        _endpoint: Some(endpoint),
        local_fp: identity.fingerprint(),
        pinned: expected.is_some(),
    })
}

/// Writes one length-prefixed preamble message.
pub(crate) async fn write_msg<T: Serialize>(w: &mut SendStream, msg: &T) -> Result<(), QuicError> {
    w.write_all(&pair::encode_preamble(msg)?).await?;
    Ok(())
}

/// Reads one length-prefixed preamble message within [`PREAMBLE_TIMEOUT`].
pub(crate) async fn read_msg<T: DeserializeOwned>(r: &mut RecvStream) -> Result<T, QuicError> {
    let read = async {
        let mut len = [0u8; 2];
        r.read_exact(&mut len).await?;
        let len = u16::from_le_bytes(len) as usize;
        if len > MAX_PREAMBLE_LEN {
            return Err(ProtoError::MessageTooLarge.into());
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body).await?;
        Ok(pair::decode_preamble(&body)?)
    };
    tokio::time::timeout(PREAMBLE_TIMEOUT, read)
        .await
        .map_err(|_| QuicError::Timeout)?
}

/// Most time a finished session's connection stays open to deliver what is still in flight.
const LINGER: Duration = Duration::from_secs(1);

/// Owner of a session's connection. Quinn closes a connection as soon as its last handle is
/// dropped and discards unsent stream data, such as a final `Bye`. When the session drops this,
/// a task keeps the connection (and the client endpoint) open until the peer closes it or
/// [`LINGER`] passes.
struct Lingering(QuicConn);

impl Drop for Lingering {
    fn drop(&mut self) {
        let conn = self.0.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
                    let _ = tokio::time::timeout(LINGER, conn.conn.closed()).await;
                    conn.close("session ended");
                });
            }
            Err(_) => conn.close("session ended"),
        }
    }
}

/// A read half that keeps the connection (and the client endpoint) alive while in use.
struct Keep<R> {
    inner: R,
    _conn: Arc<Lingering>,
}

impl<R> Keep<R> {
    fn new(inner: R, conn: &Arc<Lingering>) -> Self {
        Keep {
            inner,
            _conn: conn.clone(),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Keep<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
