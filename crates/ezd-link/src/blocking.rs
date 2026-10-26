//! A [`Link`] over blocking reader and writer halves, such as USB bulk endpoints or an Android
//! accessory file descriptor.
//!
//! Each half gets its own thread, bridged to the async side through an in-memory pipe (a
//! [`DuplexStream`] per direction: dropping either end closes it, which a split simplex pipe does
//! not do). The read
//! thread forwards whatever one `read` returns, so a transport with a fixed transfer size picks it
//! with [`BlockingOptions::read_buf`].

use crate::link::{BoxRead, Link};
use std::io::{self, ErrorKind, Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::runtime::Handle;

/// How [`Link::from_blocking`] drives the halves.
#[derive(Debug, Clone)]
pub struct BlockingOptions {
    /// Bytes asked for in each `read` call.
    pub read_buf: usize,
    /// Most bytes handed to one `write` call.
    pub write_buf: usize,
    /// Bytes buffered between each thread and the async side.
    pub pipe_capacity: usize,
    /// Prefix of the thread names (`<name>-rx`, `<name>-tx`).
    pub thread_name: String,
}

impl Default for BlockingOptions {
    fn default() -> Self {
        BlockingOptions {
            read_buf: 16 * 1024,
            write_buf: 16 * 1024,
            pipe_capacity: 256 * 1024,
            thread_name: "ezd-link".into(),
        }
    }
}

impl Link {
    /// Builds a link whose bytes come from `reader` and go to `writer`, each on its own thread.
    ///
    /// The reader may return [`ErrorKind::TimedOut`] or [`ErrorKind::WouldBlock`] to wake up
    /// periodically: the read thread then checks whether the link was dropped and reads again.
    /// `Ok(0)` from the reader ends the stream. The write thread ends when the link's writer is
    /// shut down or dropped, and drops `writer` then.
    ///
    /// Must be called inside a tokio runtime.
    pub fn from_blocking<R, W>(
        reader: R,
        writer: W,
        peer: impl Into<String>,
        opts: BlockingOptions,
    ) -> io::Result<Link>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let handle = Handle::try_current().map_err(io::Error::other)?;
        let (in_rx, in_tx) = tokio::io::duplex(opts.pipe_capacity);
        let (out_rx, out_tx) = tokio::io::duplex(opts.pipe_capacity);
        let dropped = Arc::new(AtomicBool::new(false));

        let read_dropped = dropped.clone();
        let read_handle = handle.clone();
        let read_buf = opts.read_buf.max(1);
        std::thread::Builder::new()
            .name(format!("{}-rx", opts.thread_name))
            .spawn(move || read_loop(reader, in_tx, read_buf, &read_dropped, &read_handle))?;

        let write_buf = opts.write_buf.max(1);
        std::thread::Builder::new()
            .name(format!("{}-tx", opts.thread_name))
            .spawn(move || write_loop(writer, out_rx, write_buf, &handle))?;

        let reader: BoxRead = Box::new(DropFlagRead {
            inner: in_rx,
            dropped,
        });
        Ok(Link::new(reader, Box::new(out_tx), peer))
    }
}

fn read_loop<R: Read>(
    mut reader: R,
    mut pipe: DuplexStream,
    buf_len: usize,
    dropped: &AtomicBool,
    handle: &Handle,
) {
    let mut buf = vec![0u8; buf_len];
    loop {
        if dropped.load(Ordering::Acquire) {
            return;
        }
        match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if handle.block_on(pipe.write_all(&buf[..n])).is_err() {
                    return;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::Interrupted | ErrorKind::TimedOut | ErrorKind::WouldBlock
                ) => {}
            Err(e) => {
                tracing::debug!("blocking link read ended: {e}");
                return;
            }
        }
    }
}

fn write_loop<W: Write>(mut writer: W, mut pipe: DuplexStream, buf_len: usize, handle: &Handle) {
    let mut buf = vec![0u8; buf_len];
    loop {
        let n = match handle.block_on(pipe.read(&mut buf)) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if let Err(e) = writer.write_all(&buf[..n]).and_then(|()| writer.flush()) {
            tracing::debug!("blocking link write ended: {e}");
            return;
        }
    }
}

/// Read half that tells the read thread when the link is gone.
struct DropFlagRead {
    inner: DuplexStream,
    dropped: Arc<AtomicBool>,
}

impl AsyncRead for DropFlagRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl Drop for DropFlagRead {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Reader fed by a channel; times out when nothing arrives, like a USB endpoint.
    struct ChanRead {
        rx: mpsc::Receiver<Vec<u8>>,
        reads: Arc<Mutex<Vec<usize>>>,
    }

    impl Read for ChanRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reads.lock().unwrap().push(buf.len());
            match self.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(data) => {
                    buf[..data.len()].copy_from_slice(&data);
                    Ok(data.len())
                }
                Err(mpsc::RecvTimeoutError::Timeout) => Err(ErrorKind::TimedOut.into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => Ok(0),
            }
        }
    }

    #[derive(Clone, Default)]
    struct SharedWrite {
        data: Arc<Mutex<Vec<u8>>>,
        calls: Arc<Mutex<Vec<usize>>>,
        /// Set when the write thread drops its copy.
        dropped: Arc<AtomicBool>,
    }

    impl Write for SharedWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.calls.lock().unwrap().push(buf.len());
            self.data.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The copy handed to the link.
    struct ThreadWrite(SharedWrite);

    impl Write for ThreadWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for ThreadWrite {
        fn drop(&mut self) {
            self.0.dropped.store(true, Ordering::Release);
        }
    }

    fn opts() -> BlockingOptions {
        BlockingOptions {
            read_buf: 64,
            write_buf: 100,
            pipe_capacity: 1024,
            thread_name: "test".into(),
        }
    }

    async fn eventually(what: &str, f: impl Fn() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bytes_flow_both_ways_in_order() {
        let (tx, rx) = mpsc::channel();
        let reads = Arc::new(Mutex::new(Vec::new()));
        let w = SharedWrite::default();
        let mut link = Link::from_blocking(
            ChanRead {
                rx,
                reads: reads.clone(),
            },
            ThreadWrite(w.clone()),
            "test",
            opts(),
        )
        .unwrap();

        for i in 0..20u8 {
            tx.send(vec![i; 50]).unwrap();
        }
        drop(tx);
        let mut got = Vec::new();
        link.reader.read_to_end(&mut got).await.unwrap();
        let want: Vec<u8> = (0..20u8).flat_map(|i| [i; 50]).collect();
        assert_eq!(got, want);
        assert!(reads.lock().unwrap().iter().all(|&n| n == 64));

        let out: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        link.writer.write_all(&out).await.unwrap();
        link.writer.shutdown().await.unwrap();
        eventually("the writer to be dropped", || {
            w.dropped.load(Ordering::Acquire)
        })
        .await;
        assert_eq!(*w.data.lock().unwrap(), out);
        assert!(w.calls.lock().unwrap().iter().all(|&n| n <= 100));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_link_stops_a_timing_out_reader() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let reads = Arc::new(Mutex::new(Vec::new()));
        let link = Link::from_blocking(
            ChanRead {
                rx,
                reads: reads.clone(),
            },
            SharedWrite::default(),
            "test",
            opts(),
        )
        .unwrap();
        eventually("a first read", || !reads.lock().unwrap().is_empty()).await;
        drop(link);
        // The read thread owns the receiver: once it stops, sending fails.
        eventually("the read thread to stop", || tx.send(Vec::new()).is_err()).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_writer_closes_the_write_half() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (_tx, rx) = mpsc::channel();
        let mut link = Link::from_blocking(
            ChanRead {
                rx,
                reads: Arc::default(),
            },
            Broken,
            "test",
            opts(),
        )
        .unwrap();
        let mut failed = false;
        for _ in 0..200 {
            if link.writer.write_all(&[1; 64]).await.is_err() {
                failed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(failed, "writes must fail once the device is gone");
    }
}
