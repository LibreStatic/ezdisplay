//! Input injection: the traits platform backends implement and the per-session input thread.
//!
//! The session owns one thread (`ezd-input`) per client that supports input. The control loop
//! forwards input messages to it; it creates the platform [`InputSink`] once the video source
//! reports the output it shows, injects the messages, ticks the sink and releases everything at
//! the end of the session.

use crate::source::OutputInfo;
use crate::time::now_us;
use ezd_proto::{
    ClientCaps, InputStatus, Message, PenSample, PointerButton, Scroll, StreamConfig, TouchContact,
    TouchFrame,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Injects client input into the host OS. Coordinates are normalized over the stream
/// ([`ezd_proto::denormalize`]). Owned by the session's input thread.
pub trait InputSink: Send {
    /// A pen sample.
    fn pen(&mut self, s: &PenSample);
    /// A touch frame (all active contacts).
    fn touch(&mut self, f: &TouchFrame);
    /// A pointer button press or release.
    fn button(&mut self, b: &PointerButton);
    /// A scroll / zoom event.
    fn scroll(&mut self, s: &Scroll);
    /// Called at least every 50 ms while the session runs (e.g. to refresh stationary touches).
    fn tick(&mut self) {}
    /// Releases everything that is down (pen out, touches up, buttons up). Called at session end.
    fn release_all(&mut self);
    /// What this sink can inject, sent to the client as `InputStatus`.
    fn status(&self) -> InputStatus;
}

/// How the pen is injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PenMode {
    /// A graphics tablet (pressure, tilt).
    #[default]
    Tablet,
    /// An absolute pointer (the backend's job).
    Mouse,
}

/// Everything a backend needs to create a sink for one session.
#[derive(Debug, Clone)]
pub struct InputTarget {
    /// The virtual output the stream shows, if the source knows it.
    pub output: Option<OutputInfo>,
    /// Physical size / dpi / touch points of the tablet.
    pub client: ClientCaps,
    /// The negotiated stream.
    pub stream: StreamConfig,
    /// How to inject the pen.
    pub pen_mode: PenMode,
}

/// Creates per-session sinks. Shared by all sessions of a host process.
pub trait InputBackend: Send + Sync {
    /// May block (e.g. waiting up to ~2 s for the compositor to register devices); it runs on
    /// the session's input thread. `Err(note)` = input unavailable, the note goes to the client.
    fn create(&self, target: &InputTarget) -> Result<Box<dyn InputSink>, String>;
}

impl std::fmt::Debug for dyn InputBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InputBackend")
    }
}

/// A backend that is not available: `create` always fails with its note.
#[derive(Debug, Clone)]
pub struct UnavailableBackend(pub String);

impl InputBackend for UnavailableBackend {
    fn create(&self, _target: &InputTarget) -> Result<Box<dyn InputSink>, String> {
        Err(self.0.clone())
    }
}

/// Commands from the control and video threads to the input thread.
pub(crate) enum InputCmd {
    /// An input message from the client.
    Msg(Message),
    /// The output the stream shows (`None` = unknown); sent once after the source started.
    Output(Option<OutputInfo>),
    /// End of session.
    Stop,
}

/// Latest client−host clock offset, shared with the input thread.
#[derive(Default)]
pub(crate) struct SharedOffset {
    valid: AtomicBool,
    offset_us: AtomicI64,
}

impl SharedOffset {
    pub(crate) fn set(&self, offset_us: i64) {
        self.offset_us.store(offset_us, Ordering::Relaxed);
        self.valid.store(true, Ordering::Release);
    }

    fn get(&self) -> Option<i64> {
        self.valid
            .load(Ordering::Acquire)
            .then(|| self.offset_us.load(Ordering::Relaxed))
    }
}

/// What the input thread reports when it exits.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InputStats {
    pub events: u64,
    pub latency_p50_us: Option<u32>,
    pub latency_p95_us: Option<u32>,
}

const BUCKET_US: u32 = 250;
const BUCKETS: usize = 1000; // 0..250 ms, plus one overflow bucket

struct Hist {
    buckets: Vec<u32>,
    count: u64,
}

impl Hist {
    fn new() -> Self {
        Hist {
            buckets: vec![0; BUCKETS + 1],
            count: 0,
        }
    }

    fn add(&mut self, us: u64) {
        let i = ((us / u64::from(BUCKET_US)) as usize).min(BUCKETS);
        self.buckets[i] += 1;
        self.count += 1;
    }

    /// Upper bound of the bucket holding the `p`th percentile.
    fn percentile(&self, p: u64) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        let target = ((self.count - 1) * p / 100) + 1;
        let mut seen = 0u64;
        for (i, &n) in self.buckets.iter().enumerate() {
            seen += u64::from(n);
            if seen >= target {
                return Some((i as u32 + 1) * BUCKET_US);
            }
        }
        None
    }

    fn clear(&mut self) {
        self.buckets.iter_mut().for_each(|b| *b = 0);
        self.count = 0;
    }
}

struct Recorder {
    total: Hist,
    window: Hist,
    events: u64,
}

fn msg_time(m: &Message) -> Option<u64> {
    match m {
        Message::Pen(s) => Some(s.t_us),
        Message::Touch(f) => Some(f.t_us),
        Message::PointerButton(b) => Some(b.t_us),
        Message::Scroll(s) => Some(s.t_us),
        _ => None,
    }
}

fn touch_ids(f: &TouchFrame) -> Vec<u8> {
    let mut ids: Vec<u8> = f.contacts.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    ids
}

/// Compacts the messages that queued up while the sink was being created: keeps every transition
/// (any button, any scroll, a pen sample whose flags differ from the previous one, a touch frame
/// with a non-MOVE contact or a different id set than the previous one) and the last pen sample
/// and touch frame; drops the other pure moves.
pub(crate) fn compact_backlog(msgs: Vec<Message>) -> Vec<Message> {
    let last_pen = msgs.iter().rposition(|m| matches!(m, Message::Pen(_)));
    let last_touch = msgs.iter().rposition(|m| matches!(m, Message::Touch(_)));
    let mut prev_flags: Option<u8> = None;
    let mut prev_ids: Option<Vec<u8>> = None;
    let mut out = Vec::with_capacity(msgs.len());
    for (i, m) in msgs.into_iter().enumerate() {
        let keep = match &m {
            Message::Pen(s) => {
                let transition = prev_flags != Some(s.flags);
                prev_flags = Some(s.flags);
                transition || last_pen == Some(i)
            }
            Message::Touch(f) => {
                let ids = touch_ids(f);
                let transition = f.contacts.iter().any(|c| c.phase != TouchContact::MOVE)
                    || prev_ids.as_ref() != Some(&ids);
                prev_ids = Some(ids);
                transition || last_touch == Some(i)
            }
            Message::PointerButton(_) | Message::Scroll(_) => true,
            _ => false,
        };
        if keep {
            out.push(m);
        }
    }
    out
}

fn inject(sink: &mut dyn InputSink, m: &Message) {
    match m {
        Message::Pen(s) => sink.pen(s),
        Message::Touch(f) => sink.touch(f),
        Message::PointerButton(b) => sink.button(b),
        Message::Scroll(s) => sink.scroll(s),
        _ => {}
    }
}

/// Everything the input thread needs from the session.
pub(crate) struct InputContext {
    pub backend: Arc<dyn InputBackend>,
    pub client: ClientCaps,
    pub stream: StreamConfig,
    pub pen_mode: PenMode,
    pub offset: Arc<SharedOffset>,
    pub status_tx: tokio::sync::mpsc::UnboundedSender<InputStatus>,
}

const TICK: Duration = Duration::from_millis(50);
const LOG_EVERY: Duration = Duration::from_secs(5);

/// Body of the `ezd-input` thread.
pub(crate) fn input_thread(rx: Receiver<InputCmd>, ctx: InputContext) -> InputStats {
    // Wait for the output, collecting what the client already sent.
    let mut backlog = Vec::new();
    let output = loop {
        match rx.recv() {
            Ok(InputCmd::Msg(m)) => backlog.push(m),
            Ok(InputCmd::Output(o)) => break o,
            Ok(InputCmd::Stop) | Err(_) => return InputStats::default(),
        }
    };
    let target = InputTarget {
        output,
        client: ctx.client.clone(),
        stream: ctx.stream.clone(),
        pen_mode: ctx.pen_mode,
    };
    let mut sink = match ctx.backend.create(&target) {
        Ok(s) => {
            let _ = ctx.status_tx.send(s.status());
            s
        }
        Err(note) => {
            tracing::warn!(%note, "input unavailable");
            let _ = ctx.status_tx.send(InputStatus { caps: 0, note });
            while !matches!(rx.recv(), Ok(InputCmd::Stop) | Err(_)) {}
            return InputStats::default();
        }
    };
    // Anything that arrived while the sink was being created is in the channel already.
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            InputCmd::Msg(m) => backlog.push(m),
            InputCmd::Output(_) => {}
            InputCmd::Stop => {
                sink.release_all();
                return InputStats::default();
            }
        }
    }

    let mut st = Recorder {
        total: Hist::new(),
        window: Hist::new(),
        events: 0,
    };
    let mut last_log = Instant::now();
    let mut last_tick = Instant::now();
    let record = |st: &mut Recorder, sink: &mut dyn InputSink, m: &Message| {
        inject(sink, m);
        st.events += 1;
        if let (Some(t), Some(off)) = (msg_time(m), ctx.offset.get()) {
            // offset = client - host, so client time maps to host time as t - offset.
            let us = (now_us() as i64 - (t as i64 - off)).max(0) as u64;
            st.total.add(us);
            st.window.add(us);
        }
    };
    for m in compact_backlog(backlog) {
        record(&mut st, sink.as_mut(), &m);
    }

    loop {
        let wait = TICK.saturating_sub(last_tick.elapsed());
        match rx.recv_timeout(wait) {
            Ok(InputCmd::Msg(m)) => record(&mut st, sink.as_mut(), &m),
            Ok(InputCmd::Output(_)) | Err(RecvTimeoutError::Timeout) => {}
            Ok(InputCmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_tick.elapsed() >= TICK {
            sink.tick();
            last_tick = Instant::now();
        }
        if last_log.elapsed() >= LOG_EVERY {
            last_log = Instant::now();
            if st.window.count > 0 {
                tracing::info!(
                    count = st.window.count,
                    p50_us = st.window.percentile(50),
                    p95_us = st.window.percentile(95),
                    "input latency"
                );
                st.window.clear();
            }
        }
    }
    sink.release_all();
    drop(sink);
    InputStats {
        events: st.events,
        latency_p50_us: st.total.percentile(50),
        latency_p95_us: st.total.percentile(95),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen(t: u64, flags: u8) -> Message {
        Message::Pen(PenSample {
            t_us: t,
            x: t as u16,
            y: 0,
            pressure: 0,
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
                    y: 1,
                    phase,
                })
                .collect(),
        })
    }

    fn times(v: &[Message]) -> Vec<u64> {
        v.iter().filter_map(msg_time).collect()
    }

    #[test]
    fn compaction_keeps_transitions_and_last() {
        let hover = PenSample::IN_RANGE;
        let contact = PenSample::IN_RANGE | PenSample::CONTACT;
        let msgs = vec![
            pen(1, hover), // first: flags differ from "none"
            pen(2, hover),
            pen(3, hover),
            pen(4, contact), // transition
            pen(5, contact),
            pen(6, contact),
            pen(7, hover), // transition
            pen(8, hover), // last pen
            Message::PointerButton(PointerButton {
                t_us: 9,
                x: 0,
                y: 0,
                button: PointerButton::LEFT,
                pressed: true,
            }),
            Message::Scroll(Scroll {
                t_us: 10,
                x: 0,
                y: 0,
                dx: 0,
                dy: 120,
                modifiers: 0,
            }),
            touch(11, &[(0, TouchContact::DOWN)]),
            touch(12, &[(0, TouchContact::MOVE)]), // id set unchanged, pure move: dropped
            touch(13, &[(0, TouchContact::MOVE)]), // dropped
            touch(14, &[(0, TouchContact::MOVE), (1, TouchContact::DOWN)]),
            touch(15, &[(0, TouchContact::MOVE), (1, TouchContact::MOVE)]), // dropped
            touch(16, &[(0, TouchContact::MOVE), (1, TouchContact::MOVE)]), // last touch
        ];
        let out = compact_backlog(msgs);
        assert_eq!(times(&out), vec![1, 4, 7, 8, 9, 10, 11, 14, 16]);
    }

    #[test]
    fn compaction_of_nothing_or_one() {
        assert!(compact_backlog(vec![]).is_empty());
        assert_eq!(compact_backlog(vec![pen(1, 0)]).len(), 1);
    }

    #[test]
    fn histogram_percentiles() {
        let mut h = Hist::new();
        assert_eq!(h.percentile(50), None);
        for us in [100, 100, 100, 600, 10_000_000] {
            h.add(us);
        }
        assert_eq!(h.percentile(50), Some(250));
        assert_eq!(h.percentile(95), Some(750));
        assert_eq!(h.percentile(100), Some(250_250));
        h.clear();
        assert_eq!(h.count, 0);
    }
}
