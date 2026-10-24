//! Prioritized message mux.
//!
//! [`MuxSender`] (cloneable) queues messages; a single writer task serializes
//! them onto the link with strict priority
//! `Control > Input > CursorShape > CursorState > Video`:
//!
//! * control messages and cursor shapes are reliable FIFO;
//! * input messages (pen, touch, buttons, scroll) are reliable FIFO too. When
//!   the writer takes input it drains the whole queue into one buffer and
//!   flushes once. Under a backlog (128 or more queued entries) a new pen
//!   sample with the same flags as the last queued pen sample, or a touch
//!   frame of only `MOVE` contacts with the same ids as the last queued such
//!   frame, replaces it; transitions are never dropped;
//! * [`CursorState`] is *latest wins*: a newer state replaces one that has not
//!   been written yet;
//! * video frames are fragmented into chunks and the writer re-checks the
//!   higher-priority queues between chunks, so a large keyframe delays control
//!   or cursor traffic by at most one chunk;
//! * video has a credit ([`VideoCredit`], default: 1 frame, released by the
//!   peer's ack via [`MuxSender::ack_video`]): [`MuxSender::wait_video_slot`]
//!   resolves when no video frame is queued, being written, or (with acks)
//!   outstanding beyond the window. Writing to the socket alone does not
//!   release it, since that only means the bytes sit in kernel/adb buffers. Encoded frames are never dropped (that
//!   would corrupt the reference chain); producers coalesce *before* capture by
//!   pulling the newest frame only when a slot is free.
//!
//! Reassembly on the receiving side ([`MuxReceiver`]) is keyed by
//! `(channel, kind)`, so complete frames interleaved between video fragments are fine.

use crate::link::BoxRead;
use crate::link::BoxWrite;
use ezd_proto::{
    Channel, CursorState, FrameDecoder, Incoming, Message, ProtoError, TouchContact, VideoFrame,
    encode_message, fragments, frame::put_frame,
};

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};

/// The mux (or its link) has been closed; nothing more can be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("mux closed")]
pub struct MuxClosed;

/// Errors from [`MuxSender::send`] and [`MuxReceiver::recv`].
#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    /// Transport failure.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// Protocol violation.
    #[error("protocol error: {0}")]
    Proto(#[from] ProtoError),
    /// The mux was closed.
    #[error(transparent)]
    Closed(#[from] MuxClosed),
}

/// Counters exposed by [`MuxSender::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MuxStats {
    /// Cursor states replaced before being written.
    pub cursor_states_coalesced: u64,
    /// Cursor states written.
    pub cursor_states_sent: u64,
    /// Input messages written.
    pub inputs_sent: u64,
    /// Input messages dropped by backlog coalescing (replaced by a newer one).
    pub inputs_coalesced: u64,
    /// Video frames fully written.
    pub video_frames_sent: u64,
    /// Total frames (all channels, fragments counted individually) written.
    pub frames_written: u64,
    /// Video frames whose ack did not arrive in time (credit released by timeout).
    pub ack_timeouts: u64,
}

/// When a sent video frame stops holding the video credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCredit {
    /// Released as soon as the last chunk is written to the transport. Only
    /// means "in the kernel/adb buffer"; for tests and peers that never ack.
    OnWrite,
    /// A written non-config frame stays in flight until
    /// [`MuxSender::ack_video`] is called for it (or a later id). The slot is
    /// free when fewer than `window` frames are unacked and none is queued or
    /// being written. If an ack takes longer than `timeout`, the credit is
    /// released anyway and `ack_timeouts` incremented.
    OnAck {
        /// Maximum written-but-unacked frames.
        window: u32,
        /// Ack deadline per frame.
        timeout: Duration,
    },
}

impl Default for VideoCredit {
    fn default() -> Self {
        VideoCredit::OnAck {
            window: 1,
            timeout: Duration::from_millis(500),
        }
    }
}

/// Options for starting the mux.
#[derive(Debug, Clone, Copy)]
pub struct MuxOptions {
    /// Fragment size in bytes.
    pub chunk_len: usize,
    /// Video credit policy.
    pub credit: VideoCredit,
}

impl Default for MuxOptions {
    fn default() -> Self {
        MuxOptions {
            chunk_len: ezd_proto::DEFAULT_CHUNK_LEN,
            credit: VideoCredit::default(),
        }
    }
}

impl MuxOptions {
    /// Default options with a custom fragment size.
    pub fn with_chunk(chunk_len: usize) -> Self {
        MuxOptions {
            chunk_len,
            ..Default::default()
        }
    }
}

/// Input queue length from which equivalent samples replace each other.
const INPUT_BACKLOG: usize = 128;

/// What a queued input message is, for backlog coalescing without re-decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputClass {
    /// Pen sample with these flags.
    Pen(u8),
    /// Touch frame whose contacts are all `MOVE`, with this set of ids.
    TouchMove([u64; 4]),
    /// Never coalesced.
    Other,
}

impl InputClass {
    fn of(msg: &Message) -> InputClass {
        match msg {
            Message::Pen(p) => InputClass::Pen(p.flags),
            Message::Touch(t)
                if !t.contacts.is_empty()
                    && t.contacts.iter().all(|c| c.phase == TouchContact::MOVE) =>
            {
                let mut ids = [0u64; 4];
                for c in &t.contacts {
                    ids[usize::from(c.id >> 6)] |= 1 << (c.id & 63);
                }
                InputClass::TouchMove(ids)
            }
            _ => InputClass::Other,
        }
    }
}

struct Encoded {
    channel: u8,
    kind: u8,
    payload: Vec<u8>,
    /// Frame id to wait an ack for (non-config video frames only).
    ack_id: Option<u64>,
}

/// Input queue entry.
struct QueuedInput {
    enc: Encoded,
    class: InputClass,
}

#[derive(Default)]
struct Queues {
    control: VecDeque<Encoded>,
    input: VecDeque<QueuedInput>,
    cursor_shapes: VecDeque<Encoded>,
    cursor_state: Option<Encoded>,
    video: VecDeque<Encoded>,
    /// A video frame is being written (popped from `video`).
    writing: bool,
    /// Written, not yet acked video frames with their write-completion time.
    unacked: VecDeque<(u64, Instant)>,
    /// Highest ack newer than every `unacked` frame. On a fast link the peer's ack can arrive
    /// before the writer records the frame it just wrote; the next finished frame consumes it.
    early_ack: Option<u64>,
    closing: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SlotState {
    busy: bool,
    closed: bool,
}

struct Shared {
    q: Mutex<Queues>,
    wake: Notify,
    slot: watch::Sender<SlotState>,
    coalesced: AtomicU64,
    states_sent: AtomicU64,
    inputs_sent: AtomicU64,
    inputs_coalesced: AtomicU64,
    videos_sent: AtomicU64,
    frames_written: AtomicU64,
    ack_timeouts: AtomicU64,
    credit: VideoCredit,
}

impl Shared {
    /// Recomputes the slot state; call with the queue lock held.
    fn update_slot(&self, q: &Queues) {
        let window = match self.credit {
            VideoCredit::OnWrite => usize::MAX,
            VideoCredit::OnAck { window, .. } => window.max(1) as usize,
        };
        let busy = !q.video.is_empty() || q.writing || q.unacked.len() >= window;
        self.slot.send_if_modified(|s| {
            let changed = s.busy != busy;
            s.busy = busy;
            changed
        });
    }

    fn lock(&self) -> MutexGuard<'_, Queues> {
        // Queues stay consistent even if a holder panicked.
        self.q.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self) {
        self.lock().closing = true;
        self.wake.notify_one();
    }
}

/// Calls `close` when the last `MuxSender` clone is dropped.
struct CloseOnDrop(Arc<Shared>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Cloneable handle for queueing outgoing messages. Dropping the last clone
/// closes the mux gracefully (queued messages are flushed first).
#[derive(Clone)]
pub struct MuxSender {
    shared: Arc<Shared>,
    _guard: Arc<CloseOnDrop>,
}

impl std::fmt::Debug for MuxSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MuxSender").finish_non_exhaustive()
    }
}

impl MuxSender {
    /// Queues any message on its channel.
    ///
    /// * [`CursorState`] replaces a pending one (latest wins);
    /// * [`VideoFrame`] is queued behind earlier video frames, never dropped
    ///   (the credit is advisory, see [`wait_video_slot`](Self::wait_video_slot));
    /// * input messages go to a reliable FIFO served right after control; under
    ///   a backlog, equivalent pen samples / touch moves replace the last queued
    ///   one (see the module docs);
    /// * everything else is reliable FIFO.
    pub fn send(&self, msg: &Message) -> Result<(), MuxError> {
        let (ch, kind, payload) = encode_message(msg)?;
        let ack_id = match msg {
            Message::VideoFrame(v) if !v.is_codec_config() => Some(v.frame_id),
            _ => None,
        };
        let enc = Encoded {
            channel: ch as u8,
            kind,
            payload,
            ack_id,
        };
        {
            let mut q = self.shared.lock();
            if q.closing || self.shared.slot.borrow().closed {
                return Err(MuxClosed.into());
            }
            match msg {
                Message::CursorState(_) => {
                    if q.cursor_state.replace(enc).is_some() {
                        self.shared.coalesced.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Message::Pen(_)
                | Message::Touch(_)
                | Message::PointerButton(_)
                | Message::Scroll(_)
                | Message::Key(_) => {
                    let class = InputClass::of(msg);
                    let replace = class != InputClass::Other
                        && q.input.len() >= INPUT_BACKLOG
                        && q.input.back().is_some_and(|l| l.class == class);
                    if replace {
                        if let Some(last) = q.input.back_mut() {
                            last.enc = enc;
                        }
                        self.shared.inputs_coalesced.fetch_add(1, Ordering::Relaxed);
                    } else {
                        q.input.push_back(QueuedInput { enc, class });
                    }
                }
                Message::CursorShape(_) => q.cursor_shapes.push_back(enc),
                Message::VideoFrame(_) => {
                    q.video.push_back(enc);
                    self.shared.update_slot(&q);
                }
                _ => q.control.push_back(enc),
            }
        }
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Queues a control-channel message.
    pub fn send_control(&self, msg: Message) -> Result<(), MuxError> {
        self.send(&msg)
    }

    /// Queues a cursor state, replacing any not yet written one.
    pub fn send_cursor_state(&self, st: CursorState) -> Result<(), MuxError> {
        self.send(&Message::CursorState(st))
    }

    /// Queues a video frame (see [`send`](Self::send)).
    pub fn send_video(&self, frame: VideoFrame) -> Result<(), MuxError> {
        self.send(&Message::VideoFrame(frame))
    }

    /// Resolves when no video frame is queued or in flight (the link can take a
    /// frame), or fails if the mux is closed. Producers call this *before*
    /// capturing/encoding the next frame.
    pub async fn wait_video_slot(&self) -> Result<(), MuxClosed> {
        let mut rx = self.shared.slot.subscribe();
        let st = rx
            .wait_for(|s| !s.busy || s.closed)
            .await
            .map_err(|_| MuxClosed)?;
        if st.closed { Err(MuxClosed) } else { Ok(()) }
    }

    /// Non-blocking check of the video credit.
    pub fn video_slot_free(&self) -> bool {
        let s = *self.shared.slot.borrow();
        !s.busy && !s.closed
    }

    /// True once the writer has stopped or [`close`](Self::close) was called.
    pub fn is_closed(&self) -> bool {
        self.shared.slot.borrow().closed || self.shared.lock().closing
    }

    /// Requests a graceful close: already queued messages are flushed, then the
    /// write half is shut down. Further sends fail with [`MuxClosed`].
    pub fn close(&self) {
        self.shared.close();
    }

    /// Reports that the peer received video frame `frame_id` (a `FrameAck`).
    /// Releases the credit of that frame and of every older outstanding one.
    /// Returns how many frames were released (0 for a late/unknown ack).
    pub fn ack_video(&self, frame_id: u64) -> usize {
        let mut q = self.shared.lock();
        if q.unacked.back().is_none_or(|&(id, _)| frame_id > id) {
            q.early_ack = Some(q.early_ack.map_or(frame_id, |a| a.max(frame_id)));
        }
        let before = q.unacked.len();
        q.unacked.retain(|(id, _)| *id > frame_id);
        let released = before - q.unacked.len();
        if released > 0 {
            self.shared.update_slot(&q);
        }
        released
    }

    /// Snapshot of the counters.
    pub fn stats(&self) -> MuxStats {
        MuxStats {
            cursor_states_coalesced: self.shared.coalesced.load(Ordering::Relaxed),
            cursor_states_sent: self.shared.states_sent.load(Ordering::Relaxed),
            inputs_sent: self.shared.inputs_sent.load(Ordering::Relaxed),
            inputs_coalesced: self.shared.inputs_coalesced.load(Ordering::Relaxed),
            video_frames_sent: self.shared.videos_sent.load(Ordering::Relaxed),
            frames_written: self.shared.frames_written.load(Ordering::Relaxed),
            ack_timeouts: self.shared.ack_timeouts.load(Ordering::Relaxed),
        }
    }
}

/// Handle to the writer task.
#[derive(Debug)]
pub struct MuxTask(tokio::task::JoinHandle<io::Result<()>>);

impl MuxTask {
    /// Waits for the writer to finish (after [`MuxSender::close`], all senders
    /// dropped, or a transport error).
    pub async fn join(self) -> io::Result<()> {
        match self.0.await {
            Ok(r) => r,
            Err(e) => Err(io::Error::other(e)),
        }
    }

    /// Aborts the writer immediately, discarding queued data.
    pub fn abort(&self) {
        self.0.abort();
    }
}

/// Spawns the writer task on the current tokio runtime.
pub fn spawn_mux(writer: BoxWrite, opts: MuxOptions) -> (MuxSender, MuxTask) {
    let (slot, _) = watch::channel(SlotState {
        busy: false,
        closed: false,
    });
    let shared = Arc::new(Shared {
        q: Mutex::new(Queues::default()),
        wake: Notify::new(),
        slot,
        coalesced: AtomicU64::new(0),
        states_sent: AtomicU64::new(0),
        inputs_sent: AtomicU64::new(0),
        inputs_coalesced: AtomicU64::new(0),
        videos_sent: AtomicU64::new(0),
        frames_written: AtomicU64::new(0),
        ack_timeouts: AtomicU64::new(0),
        credit: opts.credit,
    });
    let chunk_len = opts.chunk_len.clamp(1, ezd_proto::MAX_FRAME_LEN as usize);
    let task = tokio::spawn(writer_task(shared.clone(), writer, chunk_len));
    let sender = MuxSender {
        shared: shared.clone(),
        _guard: Arc::new(CloseOnDrop(shared)),
    };
    (sender, MuxTask(task))
}

enum Next {
    Whole(Encoded, bool),
    Input(Vec<Encoded>),
    VideoChunk,
    Done,
    Idle(Option<Instant>),
}

async fn writer_task(shared: Arc<Shared>, mut w: BoxWrite, chunk_len: usize) -> io::Result<()> {
    let res = writer_loop(&shared, &mut w, chunk_len).await;
    {
        let mut q = shared.lock();
        q.closing = true;
    }
    shared.slot.send_modify(|s| s.closed = true);
    let _ = w.shutdown().await;
    if let Err(e) = &res {
        tracing::debug!("mux writer stopped: {e}");
    }
    res
}

async fn writer_loop(shared: &Shared, w: &mut BoxWrite, chunk_len: usize) -> io::Result<()> {
    // Video frame currently being written and the offset of its next chunk.
    let mut in_flight: Option<(Encoded, usize)> = None;
    let mut buf: Vec<u8> = Vec::with_capacity(chunk_len + 16);
    loop {
        let next = {
            let mut q = shared.lock();
            let deadline = expire_acks(shared, &mut q);
            if let Some(m) = q.control.pop_front() {
                Next::Whole(m, false)
            } else if !q.input.is_empty() {
                Next::Input(q.input.drain(..).map(|i| i.enc).collect())
            } else if let Some(m) = q.cursor_shapes.pop_front() {
                Next::Whole(m, false)
            } else if let Some(m) = q.cursor_state.take() {
                Next::Whole(m, true)
            } else if in_flight.is_some() {
                Next::VideoChunk
            } else if let Some(v) = q.video.pop_front() {
                in_flight = Some((v, 0));
                q.writing = true;
                Next::VideoChunk
            } else if q.closing {
                Next::Done
            } else {
                Next::Idle(deadline)
            }
        };
        match next {
            Next::Idle(None) => shared.wake.notified().await,
            Next::Idle(Some(at)) => {
                let _ = tokio::time::timeout_at(at.into(), shared.wake.notified()).await;
            }
            Next::Done => return Ok(()),
            Next::Whole(m, is_state) => {
                for (flags, chunk) in fragments(&m.payload, chunk_len) {
                    buf.clear();
                    put_frame(&mut buf, m.channel, m.kind, flags, chunk);
                    w.write_all(&buf).await?;
                    shared.frames_written.fetch_add(1, Ordering::Relaxed);
                }
                w.flush().await?;
                if is_state {
                    shared.states_sent.fetch_add(1, Ordering::Relaxed);
                }
            }
            Next::Input(batch) => {
                // One contiguous buffer, one write, one flush.
                buf.clear();
                let mut n = 0u64;
                for m in &batch {
                    for (flags, chunk) in fragments(&m.payload, chunk_len) {
                        put_frame(&mut buf, m.channel, m.kind, flags, chunk);
                        shared.frames_written.fetch_add(1, Ordering::Relaxed);
                    }
                    n += 1;
                }
                w.write_all(&buf).await?;
                w.flush().await?;
                shared.inputs_sent.fetch_add(n, Ordering::Relaxed);
            }
            Next::VideoChunk => {
                let Some((enc, off)) = in_flight.as_mut() else {
                    continue;
                };
                let end = (*off + chunk_len).min(enc.payload.len());
                let flags = if end < enc.payload.len() {
                    ezd_proto::frame::FLAG_MORE
                } else {
                    0
                };
                buf.clear();
                put_frame(
                    &mut buf,
                    enc.channel,
                    enc.kind,
                    flags,
                    &enc.payload[*off..end],
                );
                *off = end;
                let finished = flags == 0;
                w.write_all(&buf).await?;
                shared.frames_written.fetch_add(1, Ordering::Relaxed);
                if finished {
                    w.flush().await?;
                    let ack_id = in_flight.take().and_then(|(e, _)| e.ack_id);
                    shared.videos_sent.fetch_add(1, Ordering::Relaxed);
                    let mut q = shared.lock();
                    q.writing = false;
                    let early = q.early_ack.take();
                    if let (Some(id), VideoCredit::OnAck { .. }) = (ack_id, shared.credit)
                        && early.is_none_or(|a| a < id)
                    {
                        q.unacked.push_back((id, Instant::now()));
                    }
                    shared.update_slot(&q);
                }
            }
        }
    }
}

/// Releases credits whose ack deadline passed; returns the next deadline.
fn expire_acks(shared: &Shared, q: &mut Queues) -> Option<Instant> {
    let VideoCredit::OnAck { timeout, .. } = shared.credit else {
        return None;
    };
    let now = Instant::now();
    let mut changed = false;
    while let Some(&(id, at)) = q.unacked.front() {
        if now.duration_since(at) < timeout {
            break;
        }
        q.unacked.pop_front();
        shared.ack_timeouts.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            frame_id = id,
            ?timeout,
            "video ack timed out, releasing credit"
        );
        changed = true;
    }
    if changed {
        shared.update_slot(q);
    }
    q.unacked.front().map(|&(_, at)| at + timeout)
}

/// Reads and decodes messages from a link's read half.
pub struct MuxReceiver {
    reader: BoxRead,
    decoder: FrameDecoder,
    buf: Vec<u8>,
    unknown: u64,
}

impl std::fmt::Debug for MuxReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MuxReceiver")
            .field("unknown_skipped", &self.unknown)
            .finish_non_exhaustive()
    }
}

impl MuxReceiver {
    /// Wraps a read half.
    pub fn new(reader: BoxRead) -> Self {
        MuxReceiver {
            reader,
            decoder: FrameDecoder::new(),
            buf: vec![0; 64 * 1024],
            unknown: 0,
        }
    }

    /// Number of messages with unknown `(channel, kind)` skipped so far.
    pub fn unknown_skipped(&self) -> u64 {
        self.unknown
    }

    /// Receives the next known message; `Ok(None)` on a clean EOF between messages.
    /// Unknown message kinds are skipped transparently. Cancel-safe.
    pub async fn recv(&mut self) -> Result<Option<Message>, MuxError> {
        loop {
            match self.decoder.pop()? {
                Some(Incoming::Message(m)) => return Ok(Some(m)),
                Some(Incoming::Unknown { channel, kind, len }) => {
                    self.unknown += 1;
                    tracing::debug!(channel, kind, len, "skipping unknown message");
                    continue;
                }
                None => {}
            }
            let n = self.reader.read(&mut self.buf).await?;
            if n == 0 {
                return if self.decoder.buffered() == 0 {
                    Ok(None)
                } else {
                    Err(io::Error::from(io::ErrorKind::UnexpectedEof).into())
                };
            }
            self.decoder.push(&self.buf[..n]);
        }
    }
}

/// Priority rank of a channel (lower is more urgent); documents the writer
/// order: control, input, cursor (shapes before states), audio, video.
pub fn priority(ch: Channel) -> u8 {
    match ch {
        Channel::Control => 0,
        Channel::Input => 1,
        Channel::Cursor => 2,
        Channel::Audio => 3,
        Channel::Video => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Link;
    use ezd_proto::{Bye, CursorShape, Key, PenSample, Ping, PointerButton, Scroll, TouchFrame};
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    fn onwrite(chunk_len: usize) -> MuxOptions {
        MuxOptions {
            chunk_len,
            credit: VideoCredit::OnWrite,
        }
    }
    fn acked(window: u32, timeout_ms: u64) -> MuxOptions {
        MuxOptions {
            chunk_len: 4096,
            credit: VideoCredit::OnAck {
                window,
                timeout: Duration::from_millis(timeout_ms),
            },
        }
    }

    fn vf(id: u64, n: usize) -> VideoFrame {
        VideoFrame {
            frame_id: id,
            flags: 0,
            capture_us: 0,
            encoded_us: 0,
            data: (0..n).map(|i| (i.wrapping_mul(31) % 256) as u8).collect(),
        }
    }
    fn cs(x: i32) -> CursorState {
        CursorState {
            x,
            y: 0,
            visible: true,
            shape_id: 1,
            ts_us: x as u64,
        }
    }
    fn pen(t: u64, flags: u8) -> Message {
        Message::Pen(PenSample {
            t_us: t,
            x: 1,
            y: 2,
            pressure: 3,
            tilt_x: 0,
            tilt_y: 0,
            flags,
        })
    }
    fn touch(t: u64, contacts: &[(u8, u8)]) -> Message {
        Message::Touch(TouchFrame {
            t_us: t,
            contacts: contacts
                .iter()
                .map(|&(id, phase)| TouchContact {
                    id,
                    x: 1,
                    y: 2,
                    phase,
                })
                .collect(),
        })
    }
    /// Writer that records the size of each write and counts flushes.
    struct Recorder {
        writes: Arc<Mutex<Vec<usize>>>,
        flushes: Arc<AtomicU64>,
    }
    impl tokio::io::AsyncWrite for Recorder {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            b: &[u8],
        ) -> std::task::Poll<io::Result<usize>> {
            self.writes.lock().unwrap().push(b.len());
            std::task::Poll::Ready(Ok(b.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    async fn recv(rx: &mut MuxReceiver) -> Message {
        timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn cursor_overtakes_big_video() {
        let (a, b) = Link::duplex_pair(8 * 1024);
        let (tx, _arx, _t) = a.into_mux_with(onwrite(16 * 1024));
        let (_btx, mut rx, _t2) = b.into_mux(16 * 1024);
        tx.send_video(vf(1, 2 * 1024 * 1024)).unwrap();
        sleep(Duration::from_millis(50)).await; // writer is now blocked mid-frame
        tx.send_control(Message::Ping(Ping { seq: 1, t0_us: 0 }))
            .unwrap();
        tx.send_cursor_state(cs(5)).unwrap();
        // Control first, then cursor, both before the video frame completes.
        assert!(matches!(recv(&mut rx).await, Message::Ping(_)));
        assert!(matches!(recv(&mut rx).await, Message::CursorState(s) if s.x == 5));
        assert!(
            matches!(recv(&mut rx).await, Message::VideoFrame(v) if v.frame_id == 1 && v.data.len() == 2 * 1024 * 1024)
        );
    }

    #[tokio::test]
    async fn input_overtakes_big_video_but_not_control() {
        let (a, b) = Link::duplex_pair(8 * 1024);
        let (tx, _arx, _t) = a.into_mux_with(onwrite(16 * 1024));
        let (_btx, mut rx, _t2) = b.into_mux(16 * 1024);
        tx.send_video(vf(1, 2 * 1024 * 1024)).unwrap();
        sleep(Duration::from_millis(50)).await; // writer is blocked mid-frame
        tx.send_cursor_state(cs(5)).unwrap();
        tx.send(&pen(1, PenSample::IN_RANGE)).unwrap();
        tx.send_control(Message::Ping(Ping { seq: 1, t0_us: 0 }))
            .unwrap();
        // Control, then input, then cursor, all before the video frame completes.
        assert!(matches!(recv(&mut rx).await, Message::Ping(_)));
        assert!(matches!(recv(&mut rx).await, Message::Pen(p) if p.t_us == 1));
        assert!(matches!(recv(&mut rx).await, Message::CursorState(_)));
        assert!(matches!(recv(&mut rx).await, Message::VideoFrame(_)));
        assert_eq!(tx.stats().inputs_sent, 1);
    }

    #[tokio::test]
    async fn queued_input_is_written_in_one_flush() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let flushes = Arc::new(AtomicU64::new(0));
        let w = Recorder {
            writes: writes.clone(),
            flushes: flushes.clone(),
        };
        let (tx, task) = spawn_mux(Box::new(w), onwrite(1024));
        // Queue everything before the writer task gets to run (current-thread runtime).
        for i in 0..5 {
            tx.send(&pen(i, PenSample::IN_RANGE)).unwrap();
        }
        tx.send(&Message::PointerButton(PointerButton {
            t_us: 9,
            x: 0,
            y: 0,
            button: PointerButton::LEFT,
            pressed: true,
        }))
        .unwrap();
        tx.send(&Message::Scroll(Scroll {
            t_us: 10,
            x: 0,
            y: 0,
            dx: 0,
            dy: 120,
            modifiers: 0,
        }))
        .unwrap();
        tx.close();
        task.join().await.unwrap();
        assert_eq!(writes.lock().unwrap().len(), 1, "one write for the batch");
        assert_eq!(flushes.load(Ordering::SeqCst), 1, "one flush for the batch");
        let s = tx.stats();
        assert_eq!(
            (s.inputs_sent, s.inputs_coalesced, s.frames_written),
            (7, 0, 7)
        );
    }

    #[tokio::test]
    async fn input_coalesces_only_under_backlog_and_keeps_transitions() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let flushes = Arc::new(AtomicU64::new(0));
        let w = Recorder { writes, flushes };
        let (tx, task) = spawn_mux(Box::new(w), onwrite(1024));
        let _ = task;
        // Below the backlog threshold nothing is replaced.
        for i in 0..INPUT_BACKLOG as u64 {
            tx.send(&pen(i, PenSample::IN_RANGE)).unwrap();
        }
        assert_eq!(tx.stats().inputs_coalesced, 0);
        // Now at the threshold: equal-flag pens replace the last entry.
        tx.send(&pen(1000, PenSample::IN_RANGE)).unwrap();
        tx.send(&pen(1001, PenSample::IN_RANGE)).unwrap();
        assert_eq!(tx.stats().inputs_coalesced, 2);
        // A flag change is a transition and is kept, as are later equal-flag
        // pens replacing only entries of their own flags.
        tx.send(&pen(1002, PenSample::IN_RANGE | PenSample::CONTACT))
            .unwrap();
        tx.send(&pen(1003, PenSample::IN_RANGE | PenSample::CONTACT))
            .unwrap();
        tx.send(&pen(1004, PenSample::IN_RANGE)).unwrap();
        assert_eq!(tx.stats().inputs_coalesced, 3);
        // Touch: DOWN is kept, same-id MOVEs replace, id change / UP are kept.
        tx.send(&touch(2000, &[(1, TouchContact::DOWN)])).unwrap();
        tx.send(&touch(2001, &[(1, TouchContact::MOVE)])).unwrap();
        tx.send(&touch(2002, &[(1, TouchContact::MOVE)])).unwrap();
        tx.send(&touch(
            2003,
            &[(1, TouchContact::MOVE), (2, TouchContact::MOVE)],
        ))
        .unwrap();
        tx.send(&touch(
            2004,
            &[(2, TouchContact::MOVE), (1, TouchContact::MOVE)],
        ))
        .unwrap();
        tx.send(&touch(
            2005,
            &[(1, TouchContact::UP), (2, TouchContact::MOVE)],
        ))
        .unwrap();
        assert_eq!(tx.stats().inputs_coalesced, 5);
        tx.close();
        // Inspect the queue directly (order preserved, transitions present).
        let q = tx.shared.lock();
        let ts: Vec<u64> = q
            .input
            .iter()
            .map(|i| {
                match ezd_proto::decode_message(i.enc.channel, i.enc.kind, &i.enc.payload)
                    .unwrap()
                    .unwrap()
                {
                    Message::Pen(p) => p.t_us,
                    Message::Touch(t) => t.t_us,
                    m => panic!("{m:?}"),
                }
            })
            .collect();
        let tail: Vec<u64> = ts.iter().copied().filter(|t| *t >= 1000).collect();
        assert_eq!(
            tail,
            vec![1001, 1003, 1004, 2000, 2002, 2004, 2005],
            "latest of each run survives"
        );
        assert!(ts.windows(2).all(|w| w[0] < w[1]), "order preserved");
        assert_eq!(ts.len(), INPUT_BACKLOG + 6);
    }

    #[tokio::test]
    async fn key_messages_queue_in_order_and_are_never_coalesced() {
        let w = Recorder {
            writes: Arc::new(Mutex::new(Vec::new())),
            flushes: Arc::new(AtomicU64::new(0)),
        };
        let (tx, task) = spawn_mux(Box::new(w), onwrite(1024));
        let _ = task;
        for i in 0..INPUT_BACKLOG as u64 {
            tx.send(&pen(i, PenSample::IN_RANGE)).unwrap();
        }
        // At the backlog threshold: identical consecutive key edges are all kept.
        let key = |t_us, usage, pressed| {
            Message::Key(Key {
                t_us,
                usage,
                pressed,
            })
        };
        let sent = [
            key(1000, Key::LEFT_CTRL, true),
            key(1001, Key::SPACE, true),
            key(1002, Key::SPACE, true),
            key(1003, Key::SPACE, false),
            key(1004, Key::LEFT_CTRL, false),
        ];
        for m in &sent {
            tx.send(m).unwrap();
        }
        assert_eq!(tx.stats().inputs_coalesced, 0);
        tx.close();
        let q = tx.shared.lock();
        let keys: Vec<Message> = q
            .input
            .iter()
            .map(|i| {
                ezd_proto::decode_message(i.enc.channel, i.enc.kind, &i.enc.payload)
                    .unwrap()
                    .unwrap()
            })
            .filter(|m| matches!(m, Message::Key(_)))
            .collect();
        assert_eq!(keys, sent);
        assert_eq!(q.input.len(), INPUT_BACKLOG + sent.len());
    }

    #[tokio::test]
    async fn video_credit() {
        let (a, b) = Link::duplex_pair(4 * 1024);
        let (tx, _arx, _t) = a.into_mux_with(onwrite(4 * 1024));
        let (_btx, mut rx, _t2) = b.into_mux(4 * 1024);
        assert!(tx.video_slot_free());
        tx.wait_video_slot().await.unwrap();
        tx.send_video(vf(1, 256 * 1024)).unwrap();
        assert!(!tx.video_slot_free());
        assert!(
            timeout(Duration::from_millis(100), tx.wait_video_slot())
                .await
                .is_err()
        );
        // Draining the receiver frees the slot.
        let waiter = {
            let tx = tx.clone();
            tokio::spawn(async move { tx.wait_video_slot().await })
        };
        assert!(matches!(recv(&mut rx).await, Message::VideoFrame(_)));
        timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(tx.video_slot_free());
        // Two frames queued back to back (config + keyframe): slot frees after both.
        tx.send_video(vf(2, 10)).unwrap();
        tx.send_video(vf(3, 10)).unwrap();
        assert!(matches!(recv(&mut rx).await, Message::VideoFrame(v) if v.frame_id == 2));
        assert!(matches!(recv(&mut rx).await, Message::VideoFrame(v) if v.frame_id == 3));
        timeout(Duration::from_secs(5), tx.wait_video_slot())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tx.stats().video_frames_sent, 3);
    }

    #[tokio::test]
    async fn cursor_state_is_latest_wins_but_shapes_are_reliable() {
        let (a, b) = Link::duplex_pair(4 * 1024);
        let (tx, _arx, _t) = a.into_mux_with(onwrite(4 * 1024));
        let (_btx, mut rx, _t2) = b.into_mux(4 * 1024);
        tx.send_video(vf(1, 256 * 1024)).unwrap();
        sleep(Duration::from_millis(50)).await;
        for i in 0..100 {
            tx.send_cursor_state(cs(i)).unwrap();
        }
        for id in 0..3 {
            tx.send(&Message::CursorShape(CursorShape {
                shape_id: id,
                width: 1,
                height: 1,
                hot_x: 0,
                hot_y: 0,
                rgba: vec![0; 4],
            }))
            .unwrap();
        }
        let mut shapes = vec![];
        let mut states = vec![];
        let mut video = false;
        while !video {
            match recv(&mut rx).await {
                Message::CursorShape(s) => shapes.push(s.shape_id),
                Message::CursorState(s) => states.push(s.x),
                Message::VideoFrame(_) => video = true,
                m => panic!("{m:?}"),
            }
        }
        assert_eq!(shapes, vec![0, 1, 2]);
        assert_eq!(states, vec![99]);
        assert!(tx.stats().cursor_states_coalesced >= 99);
    }

    #[tokio::test]
    async fn shapes_precede_states_and_close_drains() {
        let (a, b) = Link::duplex_pair(64 * 1024);
        let (tx, _arx, task) = a.into_mux_with(onwrite(1024));
        let (_btx, mut rx, _t2) = b.into_mux(1024);
        tx.send_cursor_state(cs(1)).unwrap();
        tx.send(&Message::CursorShape(CursorShape {
            shape_id: 1,
            width: 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            rgba: vec![1; 4],
        }))
        .unwrap();
        tx.send_control(Message::Bye(Bye {
            reason: "done".into(),
        }))
        .unwrap();
        tx.close();
        assert!(tx.send_control(Message::ConfigAck).is_err());
        assert!(matches!(recv(&mut rx).await, Message::Bye(_)));
        assert!(matches!(recv(&mut rx).await, Message::CursorShape(_)));
        assert!(matches!(recv(&mut rx).await, Message::CursorState(_)));
        assert!(rx.recv().await.unwrap().is_none());
        task.join().await.unwrap();
        assert!(tx.wait_video_slot().await.is_err());
    }

    #[tokio::test]
    async fn dropping_last_sender_closes() {
        let (a, b) = Link::duplex_pair(4096);
        let (tx, _arx, task) = a.into_mux_with(onwrite(1024));
        let (_btx, mut rx, _t2) = b.into_mux(1024);
        let tx2 = tx.clone();
        tx.send_control(Message::ConfigAck).unwrap();
        drop(tx);
        sleep(Duration::from_millis(20)).await;
        assert!(!tx2.is_closed());
        drop(tx2);
        assert!(matches!(recv(&mut rx).await, Message::ConfigAck));
        assert!(rx.recv().await.unwrap().is_none());
        task.join().await.unwrap();
    }

    async fn exchange(a: Link, b: Link) {
        let (tx, mut arx, _t) = a.into_mux_with(onwrite(8 * 1024));
        let (btx, mut brx, _t2) = b.into_mux(8 * 1024);
        let sender = tokio::spawn(async move {
            for i in 0..20u64 {
                tx.wait_video_slot().await.unwrap();
                tx.send_video(vf(i, 50_000 + i as usize * 1000)).unwrap();
                tx.send_cursor_state(cs(i as i32)).unwrap();
            }
            // Control outranks video, so wait for the last frame before saying goodbye.
            tx.wait_video_slot().await.unwrap();
            tx.send_control(Message::Bye(Bye {
                reason: "end".into(),
            }))
            .unwrap();
            tx.close();
        });
        let mut next = 0;
        let mut last_cursor = -1;
        loop {
            match recv(&mut brx).await {
                Message::VideoFrame(v) => {
                    assert_eq!(v.frame_id, next);
                    assert_eq!(v.data, vf(next, 50_000 + next as usize * 1000).data);
                    next += 1;
                }
                Message::CursorState(s) => {
                    assert!(s.x > last_cursor);
                    last_cursor = s.x;
                }
                Message::Bye(_) => break,
                m => panic!("{m:?}"),
            }
        }
        assert_eq!(next, 20);
        sender.await.unwrap();
        drop(btx);
        assert!(arx.recv().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn end_to_end_duplex() {
        let (a, b) = Link::duplex_pair(32 * 1024);
        exchange(a, b).await;
    }

    #[tokio::test]
    async fn end_to_end_tcp_loopback() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (c, s) = tokio::join!(Link::connect_tcp(addr), l.accept());
        let a = c.unwrap();
        let b = Link::from_tcp(s.unwrap().0).unwrap();
        exchange(a, b).await;
    }

    #[test]
    fn priority_order() {
        assert!(priority(Channel::Control) < priority(Channel::Input));
        assert!(priority(Channel::Input) < priority(Channel::Cursor));
        assert!(priority(Channel::Cursor) < priority(Channel::Audio));
        assert!(priority(Channel::Audio) < priority(Channel::Video));
    }

    #[tokio::test]
    async fn credit_is_held_until_ack() {
        let (a, b) = Link::duplex_pair(1 << 20);
        let (tx, _arx, _t) = a.into_mux_with(acked(1, 5000));
        let (_btx, mut rx, _t2) = b.into_mux(4096);
        tx.send_video(vf(7, 100)).unwrap();
        assert!(matches!(recv(&mut rx).await, Message::VideoFrame(_)));
        sleep(Duration::from_millis(50)).await;
        // Written and received, but not acked: still busy.
        assert!(!tx.video_slot_free());
        assert!(
            timeout(Duration::from_millis(100), tx.wait_video_slot())
                .await
                .is_err()
        );
        assert_eq!(tx.ack_video(7), 1);
        timeout(Duration::from_secs(1), tx.wait_video_slot())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tx.ack_video(7), 0, "late ack is harmless");
        assert_eq!(tx.stats().ack_timeouts, 0);
    }

    #[tokio::test]
    async fn ack_before_bookkeeping_releases_the_frame() {
        let (a, b) = Link::duplex_pair(1 << 20);
        let (tx, _arx, _t) = a.into_mux_with(acked(1, 5000));
        let (_btx, mut rx, _t2) = b.into_mux(4096);
        // The peer's ack for 3 is handled before the writer records frame 3 as written.
        assert_eq!(tx.ack_video(3), 0);
        tx.send_video(vf(3, 100)).unwrap();
        recv(&mut rx).await;
        timeout(Duration::from_secs(1), tx.wait_video_slot())
            .await
            .expect("an early ack must not wait for the ack timeout")
            .unwrap();
        // The early ack was consumed: the next frame waits for its own ack.
        tx.send_video(vf(4, 100)).unwrap();
        recv(&mut rx).await;
        sleep(Duration::from_millis(50)).await;
        assert!(!tx.video_slot_free());
        assert_eq!(tx.ack_video(4), 1);
        assert!(tx.video_slot_free());
        assert_eq!(tx.stats().ack_timeouts, 0);
    }

    #[tokio::test]
    async fn later_ack_releases_earlier_and_window_two() {
        let (a, b) = Link::duplex_pair(1 << 20);
        let (tx, _arx, _t) = a.into_mux_with(acked(2, 5000));
        let (_btx, mut rx, _t2) = b.into_mux(4096);
        tx.send_video(vf(1, 100)).unwrap();
        recv(&mut rx).await;
        sleep(Duration::from_millis(30)).await;
        assert!(tx.video_slot_free(), "window 2 allows a second frame");
        tx.send_video(vf(2, 100)).unwrap();
        recv(&mut rx).await;
        sleep(Duration::from_millis(30)).await;
        assert!(!tx.video_slot_free(), "two frames in flight");
        assert_eq!(tx.ack_video(2), 2, "acking 2 releases 1 as well");
        assert!(tx.video_slot_free());
    }

    #[tokio::test]
    async fn partial_ack_with_window_two() {
        let (a, b) = Link::duplex_pair(1 << 20);
        let (tx, _arx, _t) = a.into_mux_with(acked(2, 5000));
        let (_btx, mut rx, _t2) = b.into_mux(4096);
        for id in [1, 2] {
            tx.send_video(vf(id, 100)).unwrap();
            recv(&mut rx).await;
            sleep(Duration::from_millis(30)).await;
        }
        assert_eq!(tx.ack_video(1), 1);
        assert!(tx.video_slot_free());
    }

    #[tokio::test]
    async fn ack_timeout_frees_slot() {
        let (a, b) = Link::duplex_pair(1 << 20);
        let (tx, _arx, _t) = a.into_mux_with(acked(1, 100));
        let (_btx, mut rx, _t2) = b.into_mux(4096);
        tx.send_video(vf(1, 100)).unwrap();
        recv(&mut rx).await;
        assert!(
            timeout(Duration::from_secs(2), tx.wait_video_slot())
                .await
                .is_ok()
        );
        assert_eq!(tx.stats().ack_timeouts, 1);
    }

    #[tokio::test]
    async fn config_frames_need_no_ack() {
        let (a, b) = Link::duplex_pair(1 << 20);
        let (tx, _arx, _t) = a.into_mux_with(acked(1, 5000));
        let (_btx, mut rx, _t2) = b.into_mux(4096);
        let mut cfg = vf(1, 10);
        cfg.flags = VideoFrame::CODEC_CONFIG;
        tx.send_video(cfg).unwrap();
        recv(&mut rx).await;
        timeout(Duration::from_secs(1), tx.wait_video_slot())
            .await
            .unwrap()
            .unwrap();
        // Config + keyframe queued back to back: only the keyframe waits for an ack.
        let mut cfg = vf(2, 10);
        cfg.flags = VideoFrame::CODEC_CONFIG;
        tx.send_video(cfg).unwrap();
        tx.send_video(vf(2, 100)).unwrap();
        recv(&mut rx).await;
        recv(&mut rx).await;
        sleep(Duration::from_millis(30)).await;
        assert!(!tx.video_slot_free());
        tx.ack_video(2);
        assert!(tx.video_slot_free());
    }
}
