//! Shared state between the receive loop and the application.

use crate::percentile;
use ezd_link::MuxSender;
use ezd_proto::{
    ClientOptions, ClientStats, ClockSync, CursorShape, CursorState, InputStatus, Message,
    StreamColor,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;

const MAX_SAMPLES: usize = 8192;
const MAX_SHAPES: usize = 1024;

#[derive(Default)]
pub(crate) struct StatsWindow {
    pub received: u32,
    pub decoded: u32,
    pub dropped: u32,
    pub bytes: u64,
    pub decode_us: Vec<u32>,
    pub e2e_us: Vec<u32>,
}

#[derive(Default)]
struct Cursor {
    shapes: HashMap<u32, Arc<CursorShape>>,
    state: Option<CursorState>,
    /// Incremented on every state update.
    seq: u64,
}

/// Latest cursor state with its update counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSnapshot {
    /// Latest `CursorState`.
    pub state: CursorState,
    /// Number of states received so far (changes on every update).
    pub seq: u64,
}

/// Last statistics window and session totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientSnapshot {
    /// The most recent `ClientStats` sent to the host (`None` before the first).
    pub last_stats: Option<ClientStats>,
    /// Video payload bytes received in that window.
    pub window_video_bytes: u64,
    /// Best ping RTT in microseconds.
    pub rtt_us: Option<i64>,
    /// `host_clock - client_clock` in microseconds.
    pub clock_offset_us: Option<i64>,
    /// Video frames received since the session started.
    pub total_received: u64,
    /// Frames reported decoded.
    pub total_decoded: u64,
    /// Frames reported dropped.
    pub total_dropped: u64,
    /// Video payload bytes received.
    pub total_video_bytes: u64,
    /// Latest `InputStatus` from the host (protocol 0.3).
    pub input_status: Option<InputStatus>,
    /// Latest `ClientOptions` from the host (protocol 0.3).
    pub client_options: Option<ClientOptions>,
    /// Input messages handed to the link by [`ClientHandle::send_input`].
    pub inputs_sent: u64,
    /// Colour description of the stream (protocol 0.5); BT.709 SDR until the host sends one.
    pub stream_color: StreamColor,
}

pub(crate) struct Inner {
    pub notify: Notify,
    stop: AtomicBool,
    keyframe: AtomicBool,
    pub window: Mutex<StatsWindow>,
    pub clock: Mutex<ClockSync>,
    cursor: Mutex<Cursor>,
    snapshot: Mutex<ClientSnapshot>,
    /// Set while connected to a host that understands input.
    input_tx: Mutex<Option<MuxSender>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Cheaply cloneable handle to a running (or about to run) client session.
///
/// Create it before [`run_client`](crate::run_client) and pass a clone to the
/// decoder side. All methods are non-blocking and callable from any thread.
#[derive(Clone)]
pub struct ClientHandle(pub(crate) Arc<Inner>);

impl Default for ClientHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ClientHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientHandle").finish_non_exhaustive()
    }
}

impl ClientHandle {
    /// A fresh handle for one session.
    pub fn new() -> Self {
        ClientHandle(Arc::new(Inner {
            notify: Notify::new(),
            stop: AtomicBool::new(false),
            keyframe: AtomicBool::new(false),
            window: Mutex::new(StatsWindow::default()),
            clock: Mutex::new(ClockSync::default()),
            cursor: Mutex::new(Cursor::default()),
            snapshot: Mutex::new(ClientSnapshot::default()),
            input_tx: Mutex::new(None),
        }))
    }

    /// Asks the host for a keyframe (sends `RequestKeyframe`). Repeated calls
    /// before the loop runs coalesce into one request.
    pub fn request_keyframe(&self) {
        self.0.keyframe.store(true, Ordering::Release);
        self.0.notify.notify_one();
    }

    /// Ends the session: the loop sends `Bye` and returns [`ClientEnd::Stopped`](crate::ClientEnd).
    pub fn stop(&self) {
        self.0.stop.store(true, Ordering::Release);
        self.0.notify.notify_one();
    }

    /// True once [`stop`](Self::stop) was called.
    pub fn is_stopped(&self) -> bool {
        self.0.stop.load(Ordering::Acquire)
    }

    pub(crate) fn take_keyframe_request(&self) -> bool {
        self.0.keyframe.swap(false, Ordering::AcqRel)
    }

    /// Reports one frame decoded and shown. `decode_us` is the time from
    /// queueing to the decoder until its output; `capture_us` is the frame's
    /// host capture time and `done_us` the [`now_us`](crate::now_us) of the
    /// output. Returns the end-to-end latency in microseconds when the clock
    /// offset is known.
    pub fn report_decoded(&self, decode_us: u32, capture_us: u64, done_us: u64) -> Option<u32> {
        let offset = lock(&self.0.clock).offset_us();
        let mut w = lock(&self.0.window);
        w.decoded += 1;
        if w.decode_us.len() < MAX_SAMPLES {
            w.decode_us.push(decode_us);
        }
        // offset = host - client, so host time maps to client time as t - offset.
        let e2e = offset.map(|off| (done_us as i64 - (capture_us as i64 - off)).max(0) as u32);
        if let Some(e) = e2e
            && w.e2e_us.len() < MAX_SAMPLES
        {
            w.e2e_us.push(e);
        }
        e2e
    }

    /// Reports `n` frames that were not decoded.
    pub fn report_dropped(&self, n: u32) {
        lock(&self.0.window).dropped += n;
    }

    /// Estimated `host_clock - client_clock` in microseconds.
    pub fn clock_offset_us(&self) -> Option<i64> {
        lock(&self.0.clock).offset_us()
    }

    /// Best ping RTT in microseconds.
    pub fn rtt_us(&self) -> Option<i64> {
        lock(&self.0.clock).rtt_us()
    }

    /// Latest cursor state, if any was received.
    pub fn cursor(&self) -> Option<CursorSnapshot> {
        let c = lock(&self.0.cursor);
        c.state.map(|state| CursorSnapshot { state, seq: c.seq })
    }

    /// A cached cursor shape.
    pub fn cursor_shape(&self, shape_id: u32) -> Option<Arc<CursorShape>> {
        lock(&self.0.cursor).shapes.get(&shape_id).cloned()
    }

    /// Sends an input message (`Pen`, `Touch`, `PointerButton`, `Scroll`, `Key`) to the host.
    ///
    /// Returns false, dropping the message, if the session is not connected, the host speaks
    /// protocol older than 0.3, `msg` is not an input message, the link is closed or `msg` is a
    /// `Key` and the host has not announced [`InputStatus::KEYS`]. Callable from any thread;
    /// never blocks.
    pub fn send_input(&self, msg: &Message) -> bool {
        match msg {
            Message::Pen(_)
            | Message::Touch(_)
            | Message::PointerButton(_)
            | Message::Scroll(_) => {}
            Message::Key(_) => {
                let keys = lock(&self.0.snapshot)
                    .input_status
                    .as_ref()
                    .is_some_and(|s| s.caps & InputStatus::KEYS != 0);
                if !keys {
                    return false;
                }
            }
            _ => return false,
        }
        let Some(tx) = lock(&self.0.input_tx).clone() else {
            return false;
        };
        if tx.send(msg).is_err() {
            return false;
        }
        lock(&self.0.snapshot).inputs_sent += 1;
        true
    }

    /// Latest `InputStatus` the host sent, if any.
    pub fn input_status(&self) -> Option<InputStatus> {
        lock(&self.0.snapshot).input_status.clone()
    }

    /// Latest `ClientOptions` the host sent, if any.
    pub fn client_options(&self) -> Option<ClientOptions> {
        lock(&self.0.snapshot).client_options
    }

    /// Colour description of the current stream; set before the decoder is created, BT.709 8-bit
    /// when the host sent no `StreamColor`.
    pub fn stream_color(&self) -> StreamColor {
        lock(&self.0.snapshot).stream_color
    }

    pub(crate) fn set_input_sender(&self, tx: Option<MuxSender>) {
        *lock(&self.0.input_tx) = tx;
    }

    pub(crate) fn store_stream_color(&self, c: StreamColor) {
        lock(&self.0.snapshot).stream_color = c;
    }

    pub(crate) fn store_input_status(&self, st: InputStatus) {
        lock(&self.0.snapshot).input_status = Some(st);
    }

    pub(crate) fn store_client_options(&self, o: ClientOptions) {
        lock(&self.0.snapshot).client_options = Some(o);
    }

    /// Last statistics window and totals.
    pub fn snapshot(&self) -> ClientSnapshot {
        let mut s = lock(&self.0.snapshot).clone();
        let clock = lock(&self.0.clock);
        s.rtt_us = clock.rtt_us();
        s.clock_offset_us = clock.offset_us();
        s
    }

    pub(crate) fn store_shape(&self, shape: CursorShape) -> Arc<CursorShape> {
        let mut c = lock(&self.0.cursor);
        if c.shapes.len() >= MAX_SHAPES && !c.shapes.contains_key(&shape.shape_id) {
            c.shapes.clear();
        }
        let shape = Arc::new(shape);
        c.shapes.insert(shape.shape_id, shape.clone());
        shape
    }

    pub(crate) fn store_cursor_state(&self, st: CursorState) {
        let mut c = lock(&self.0.cursor);
        c.state = Some(st);
        c.seq += 1;
    }

    pub(crate) fn add_clock_sample(&self, t0: u64, t1: u64, t2: u64, t3: u64) {
        lock(&self.0.clock).add_sample(t0, t1, t2, t3);
    }

    pub(crate) fn count_received(&self, bytes: usize) {
        let mut w = lock(&self.0.window);
        w.received += 1;
        w.bytes += bytes as u64;
        drop(w);
        let mut s = lock(&self.0.snapshot);
        s.total_received += 1;
        s.total_video_bytes += bytes as u64;
    }

    /// Closes the current statistics window and returns the stats to send.
    pub(crate) fn roll_window(&self, window_ms: u32) -> ClientStats {
        let w = std::mem::take(&mut *lock(&self.0.window));
        let st = ClientStats {
            window_ms,
            frames_received: w.received,
            frames_decoded: w.decoded,
            frames_dropped: w.dropped,
            decode_us_p50: percentile(&w.decode_us, 50),
            decode_us_p95: percentile(&w.decode_us, 95),
            e2e_us_p50: percentile(&w.e2e_us, 50),
            e2e_us_p95: percentile(&w.e2e_us, 95),
        };
        let mut s = lock(&self.0.snapshot);
        s.last_stats = Some(st);
        s.window_video_bytes = w.bytes;
        s.total_decoded += w.decoded as u64;
        s.total_dropped += w.dropped as u64;
        st
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezd_link::MuxOptions;
    use ezd_proto::{Key, PenSample};

    fn key() -> Message {
        Message::Key(Key {
            t_us: 1,
            usage: Key::SPACE,
            pressed: true,
        })
    }

    fn status(caps: u32) -> InputStatus {
        InputStatus {
            caps,
            note: String::new(),
        }
    }

    #[tokio::test]
    async fn keys_are_sent_only_when_the_host_announced_them() {
        let h = ClientHandle::new();
        let (tx, _task) =
            ezd_link::mux::spawn_mux(Box::new(tokio::io::sink()), MuxOptions::default());
        h.set_input_sender(Some(tx));
        let pen = Message::Pen(PenSample {
            t_us: 1,
            x: 0,
            y: 0,
            pressure: 0,
            tilt_x: 0,
            tilt_y: 0,
            flags: 0,
        });
        assert!(h.send_input(&pen));
        assert!(!h.send_input(&key()), "no InputStatus yet");
        h.store_input_status(status(InputStatus::PEN | InputStatus::TOUCH));
        assert!(!h.send_input(&key()), "host without KEYS");
        h.store_input_status(status(InputStatus::PEN | InputStatus::KEYS));
        assert!(h.send_input(&key()));
        assert!(h.send_input(&pen));
        assert_eq!(h.snapshot().inputs_sent, 3);
    }
}
