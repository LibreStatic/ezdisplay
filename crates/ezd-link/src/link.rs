//! [`Link`]: a reliable ordered byte stream.

use crate::mux::{MuxOptions, MuxReceiver, MuxSender, MuxTask, spawn_mux, spawn_mux_split};
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, ToSocketAddrs};

/// Boxed read half of a link.
pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
/// Boxed write half of a link.
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

/// A connected, reliable, ordered byte stream split into halves.
///
/// Transports (TCP and QUIC now; AOA bulk endpoints later) only have to produce a
/// `Link`; everything above it is transport-agnostic. A link may carry a second
/// stream for video (see [`Link::with_video`]), which the mux then uses so video
/// cannot delay control, input or cursor traffic.
pub struct Link {
    /// Read half.
    pub reader: BoxRead,
    /// Write half.
    pub writer: BoxWrite,
    /// Human readable description of the peer, for logs.
    pub peer: String,
    /// Separate video stream (read, write), if the transport has one.
    video: Option<(BoxRead, BoxWrite)>,
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
            video: None,
        }
    }

    /// Adds a separate video stream. A side that only receives video passes
    /// `tokio::io::sink()` as the write half; one that only sends passes `tokio::io::empty()`.
    pub fn with_video(mut self, reader: BoxRead, writer: BoxWrite) -> Self {
        self.video = Some((reader, writer));
        self
    }

    /// True when the link has a separate video stream.
    pub fn has_video_stream(&self) -> bool {
        self.video.is_some()
    }

    /// Wraps a connected TCP stream and enables `TCP_NODELAY`.
    pub fn from_tcp(stream: TcpStream) -> io::Result<Link> {
        stream.set_nodelay(true)?;
        let peer = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "tcp:?".into());
        let (r, w) = stream.into_split();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let r = QuickAckRead(r);
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
        match self.video {
            None => {
                let rx = MuxReceiver::new(self.reader);
                let (tx, task) = spawn_mux(self.writer, opts);
                (tx, rx, task)
            }
            Some((video_r, video_w)) => {
                let rx = MuxReceiver::new_split(self.reader, video_r);
                let (tx, task) = spawn_mux_split(self.writer, video_w, opts);
                (tx, rx, task)
            }
        }
    }
}

/// Read half of a TCP link that acknowledges what it reads right away.
///
/// With `adb reverse`, the app talks to a socket that adbd accepted with Nagle's algorithm on, so
/// adbd holds each message until the app has acknowledged the previous one. The app's kernel
/// delays that acknowledgement by up to 40 ms when the app has nothing to send back. While the
/// mouse moved, each small cursor update held the next video frame back by about 30 ms.
/// `TCP_QUICKACK` sends the pending acknowledgement at once. The kernel drops back to delayed
/// acknowledgements on its own, so it is set again after every read.
#[cfg(any(target_os = "linux", target_os = "android"))]
struct QuickAckRead(tokio::net::tcp::OwnedReadHalf);

#[cfg(any(target_os = "linux", target_os = "android"))]
impl AsyncRead for QuickAckRead {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let before = buf.filled().len();
        let r = std::pin::Pin::new(&mut self.0).poll_read(cx, buf);
        if matches!(r, std::task::Poll::Ready(Ok(()))) && buf.filled().len() > before {
            // Best effort: if it fails, the acknowledgement is only delayed as before.
            let _ = socket2::SockRef::from(self.0.as_ref()).set_tcp_quickack(true);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tcp_link_reads_everything_in_order() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            for i in 0..50u8 {
                s.write_all(&[i; 300]).await.unwrap();
            }
        });
        let mut link = Link::connect_tcp(addr).await.unwrap();
        let mut got = Vec::new();
        link.reader.read_to_end(&mut got).await.unwrap();
        server.await.unwrap();
        let want: Vec<u8> = (0..50u8).flat_map(|i| [i; 300]).collect();
        assert_eq!(got, want);
    }
}
