//! [`Link`]: a reliable ordered byte stream.

use crate::mux::{MuxOptions, MuxReceiver, MuxSender, MuxTask, spawn_mux};
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, ToSocketAddrs};

/// Boxed read half of a link.
pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
/// Boxed write half of a link.
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

/// A connected, reliable, ordered byte stream split into halves.
///
/// Transports (TCP now; AOA bulk endpoints later) only have to produce a
/// `Link`; everything above it is transport-agnostic.
pub struct Link {
    /// Read half.
    pub reader: BoxRead,
    /// Write half.
    pub writer: BoxWrite,
    /// Human readable description of the peer, for logs.
    pub peer: String,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

impl Link {
    /// Builds a link from arbitrary halves.
    pub fn new(reader: BoxRead, writer: BoxWrite, peer: impl Into<String>) -> Self {
        Link {
            reader,
            writer,
            peer: peer.into(),
        }
    }

    /// Wraps a connected TCP stream and enables `TCP_NODELAY`.
    pub fn from_tcp(stream: TcpStream) -> io::Result<Link> {
        stream.set_nodelay(true)?;
        let peer = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "tcp:?".into());
        let (r, w) = stream.into_split();
        Ok(Link::new(Box::new(r), Box::new(w), peer))
    }

    /// Connects to `addr` over TCP (with `TCP_NODELAY`).
    pub async fn connect_tcp(addr: impl ToSocketAddrs) -> io::Result<Link> {
        Link::from_tcp(TcpStream::connect(addr).await?)
    }

    /// Creates two connected in-memory links; each direction buffers `capacity` bytes.
    pub fn duplex_pair(capacity: usize) -> (Link, Link) {
        let (a, b) = tokio::io::duplex(capacity);
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        (
            Link::new(Box::new(ar), Box::new(aw), "duplex:a"),
            Link::new(Box::new(br), Box::new(bw), "duplex:b"),
        )
    }

    /// Starts the prioritized mux on this link with default options
    /// (video credit released on ack), fragmenting at `chunk_len` bytes.
    ///
    /// Must be called inside a tokio runtime. See [`crate::mux`].
    pub fn into_mux(self, chunk_len: usize) -> (MuxSender, MuxReceiver, MuxTask) {
        self.into_mux_with(MuxOptions::with_chunk(chunk_len))
    }

    /// Starts the mux with explicit [`MuxOptions`].
    pub fn into_mux_with(self, opts: MuxOptions) -> (MuxSender, MuxReceiver, MuxTask) {
        let rx = MuxReceiver::new(self.reader);
        let (tx, task) = spawn_mux(self.writer, opts);
        (tx, rx, task)
    }
}
