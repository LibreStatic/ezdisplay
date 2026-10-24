//! Converts the next capture frame while the core thread encodes and sends the previous one.
//!
//! The core asks for a frame only when the link has room for it, and encoded frames are never
//! dropped, so encoding stays on the core thread. What can run ahead is the conversion (fence
//! wait, colour conversion and scaling): a converter thread prepares one frame and parks it in a
//! one-slot mailbox. Every conversion is encoded, so the render GPU does no extra work.
//!
//! The next conversion starts after the core takes the parked frame: at once ([`Start::Early`]),
//! or timed to finish when the core is expected to ask again ([`Start::Late`]), so the frame is
//! as fresh as the serial path would give and its GPU work overlaps the link round trip rather
//! than the encoder.

use crate::encode::{Converter, Prepared};
use crate::frame::Frame;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Where the converter takes capture frames from.
pub trait FrameSource: Send + 'static {
    /// Newest frame, waiting up to `timeout`. `Err` when the stream died.
    fn next(&self, timeout: Duration) -> Result<Option<Frame>, String>;
}

/// What [`ConvertAhead::take`] hands out.
pub enum Item {
    /// A frame ready for the encoder.
    Frame(Prepared),
    /// Converting a frame failed; the converter goes on with the next one.
    Failed { dma: bool, error: String },
    /// The capture stream died or the converter stopped; nothing more will come.
    Closed(String),
}

/// When the converter starts on the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// As soon as the core took the parked frame.
    Early,
    /// So that it finishes when the core is expected to ask for the next frame.
    Late,
}

/// Exponential moving average in microseconds, following a rise and a fall at different rates.
#[derive(Clone, Copy, Default)]
struct Ema(Option<f64>);

impl Ema {
    fn add(&mut self, v: Duration, rise: f64, fall: f64) {
        let v = v.min(MAX_SAMPLE).as_micros() as f64;
        self.0 = Some(match self.0 {
            None => v,
            Some(e) => e + (v - e) * if v > e { rise } else { fall },
        });
    }

    fn get(&self) -> Option<Duration> {
        self.0.map(|e| Duration::from_micros(e as u64))
    }
}

/// Samples above this are clamped (a stalled link should not push the next start far out).
const MAX_SAMPLE: Duration = Duration::from_millis(200);
/// Safety margin of a late start.
const MARGIN: Duration = Duration::from_millis(2);

/// How long after a take a late start waits: the core's expected time to ask again (encode and
/// link round trip) minus the expected conversion time and a margin.
fn late_delay(cycle: Option<Duration>, convert: Option<Duration>) -> Duration {
    match (cycle, convert) {
        (Some(c), Some(v)) => c.saturating_sub(v + MARGIN),
        _ => Duration::ZERO,
    }
}

#[derive(Default)]
struct Slot {
    item: Option<Item>,
    /// Set by the converter thread when it ended on its own.
    closed: Option<String>,
    stop: bool,
    /// When the core last took an item.
    taken_at: Option<Instant>,
    /// The core has not asked again since its last take.
    awaiting_ask: bool,
    /// Time from a take to the core's next ask.
    cycle: Ema,
}

struct Shared {
    m: Mutex<Slot>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.m.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How often the converter thread checks for stop while the capture stream is idle.
const POLL: Duration = Duration::from_millis(50);

/// Handle to the converter thread. Dropping it stops and joins the thread.
pub struct ConvertAhead {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl ConvertAhead {
    /// Starts the converter thread on `source`.
    pub fn start(
        conv: Converter,
        source: impl FrameSource,
        start: Start,
    ) -> Result<ConvertAhead, String> {
        let shared = Arc::new(Shared {
            m: Mutex::new(Slot::default()),
            cv: Condvar::new(),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("ezd-convert".into())
                .spawn(move || {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(conv, &source, &shared, start)
                    }));
                    let closed = match r {
                        Ok(r) => r,
                        Err(_) => Some("frame converter panicked".to_string()),
                    };
                    shared.lock().closed = Some(closed.unwrap_or_else(|| "stopped".into()));
                    shared.cv.notify_all();
                })
                .map_err(|e| format!("cannot spawn converter thread: {e}"))?
        };
        Ok(ConvertAhead {
            shared,
            thread: Some(thread),
        })
    }

    /// Takes the parked item, waiting up to `timeout`; the converter then starts on the next
    /// frame. `None` when nothing was ready in time.
    pub fn take(&self, timeout: Duration) -> Option<Item> {
        let asked = Instant::now();
        let deadline = asked + timeout;
        let mut s = self.shared.lock();
        if std::mem::take(&mut s.awaiting_ask)
            && let Some(t) = s.taken_at
        {
            // Follow drops quickly: starting late costs more than starting early.
            s.cycle.add(asked - t, 0.125, 0.5);
        }
        loop {
            if let Some(item) = s.item.take() {
                s.taken_at = Some(Instant::now());
                s.awaiting_ask = true;
                drop(s);
                self.shared.cv.notify_all();
                return Some(item);
            }
            if let Some(e) = &s.closed {
                return Some(Item::Closed(e.clone()));
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            s = self
                .shared
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for ConvertAhead {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Converter loop. Returns why it ended on its own, or `None` when asked to stop.
fn run(
    mut conv: Converter,
    source: &impl FrameSource,
    shared: &Shared,
    start: Start,
) -> Option<String> {
    let mut convert = Ema::default();
    loop {
        // Wait until the core took the parked frame, then until the start time.
        {
            let mut s = shared.lock();
            while s.item.is_some() && !s.stop {
                s = shared.cv.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            let at = match (start, s.taken_at) {
                (Start::Late, Some(t)) => t + late_delay(s.cycle.get(), convert.get()),
                _ => Instant::now(),
            };
            loop {
                if s.stop {
                    return None;
                }
                let now = Instant::now();
                if now >= at {
                    break;
                }
                s = shared
                    .cv
                    .wait_timeout(s, at - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        let frame = match source.next(POLL) {
            Ok(Some(f)) => f,
            Ok(None) => {
                if shared.lock().stop {
                    return None;
                }
                continue;
            }
            Err(e) => return Some(e),
        };
        let dma = frame.is_dma();
        let t = Instant::now();
        let r = conv.convert(frame, true);
        // Follow rises quickly, for the same reason.
        convert.add(t.elapsed(), 0.5, 0.125);
        let item = match r {
            Ok(Some(p)) => Item::Frame(p),
            Ok(None) => continue,
            Err(error) => Item::Failed { dma, error },
        };
        shared.lock().item = Some(item);
        shared.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{Encoder, Kind, Plan, test_shm_frame};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Hands out `n` shared-memory frames, then reports the stream as closed.
    struct Fake {
        left: AtomicU64,
        taken: Arc<AtomicU64>,
    }

    impl FrameSource for Fake {
        fn next(&self, _timeout: Duration) -> Result<Option<Frame>, String> {
            if self.left.load(Ordering::SeqCst) == 0 {
                return Err("fake stream ended".into());
            }
            self.left.fetch_sub(1, Ordering::SeqCst);
            let seq = self.taken.fetch_add(1, Ordering::SeqCst);
            let mut f = test_shm_frame(
                320,
                240,
                [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)],
            );
            f.seq = seq;
            Ok(Some(f))
        }
    }

    fn x264() -> Plan {
        Plan {
            name: "libx264",
            kind: Kind::X264,
            node: None,
        }
    }

    #[test]
    fn converts_one_frame_per_take_and_reports_close() {
        let taken = Arc::new(AtomicU64::new(0));
        let fake = Fake {
            left: AtomicU64::new(3),
            taken: taken.clone(),
        };
        let ahead =
            ConvertAhead::start(Converter::new(x264(), 60_000), fake, Start::Early).unwrap();
        let mut enc = Encoder::new("libx264", 60_000, 4_000);
        let mut seqs = Vec::new();
        let mut packets = 0;
        loop {
            match ahead
                .take(Duration::from_secs(5))
                .expect("converter stalled")
            {
                Item::Frame(p) => {
                    // One frame parked, none converted beyond it.
                    std::thread::sleep(Duration::from_millis(20));
                    assert!(taken.load(Ordering::SeqCst) <= p.seq + 2);
                    seqs.push(p.seq);
                    let first = seqs.len() == 1;
                    if let Some(o) = enc.encode(p, false).unwrap() {
                        assert_eq!(o.keyframe, first);
                        assert_eq!(o.codec_config.is_some(), first);
                        assert_eq!(o.path, "x264-swscale");
                        packets += 1;
                    }
                }
                Item::Failed { error, .. } => panic!("conversion failed: {error}"),
                Item::Closed(e) => {
                    assert_eq!(e, "fake stream ended");
                    break;
                }
            }
        }
        assert_eq!(seqs, [0, 1, 2]);
        assert_eq!(packets, 3);
        // Closed stays closed.
        assert!(matches!(
            ahead.take(Duration::from_millis(10)),
            Some(Item::Closed(_))
        ));
    }

    #[test]
    fn late_start_converts_fresher_frames() {
        // A source with a new frame every 5 ms, and a core that takes 30 ms per frame.
        struct Ticking(Instant);
        impl FrameSource for Ticking {
            fn next(&self, _timeout: Duration) -> Result<Option<Frame>, String> {
                std::thread::sleep(Duration::from_millis(5));
                let mut f = test_shm_frame(
                    64,
                    64,
                    [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)],
                );
                f.capture_us = self.0.elapsed().as_micros() as u64;
                Ok(Some(f))
            }
        }
        let age = |start: Start| {
            let t0 = Instant::now();
            let ahead =
                ConvertAhead::start(Converter::new(x264(), 60_000), Ticking(t0), start).unwrap();
            let mut ages = Vec::new();
            for _ in 0..12 {
                let Some(Item::Frame(p)) = ahead.take(Duration::from_secs(5)) else {
                    panic!("no frame");
                };
                ages.push(t0.elapsed().as_micros() as u64 - p.capture_us);
                std::thread::sleep(Duration::from_millis(30));
            }
            // Skip the first takes while the averages settle.
            ages[6..].iter().sum::<u64>() / 6
        };
        let (early, late) = (age(Start::Early), age(Start::Late));
        assert!(
            early > 20_000,
            "early start should hand out old frames: {early} us"
        );
        assert!(
            late < 15_000,
            "late start should hand out fresh frames: {late} us"
        );
    }

    #[test]
    fn late_delay_leaves_room_for_the_conversion() {
        let ms = Duration::from_millis;
        assert_eq!(late_delay(None, Some(ms(5))), Duration::ZERO);
        assert_eq!(late_delay(Some(ms(20)), None), Duration::ZERO);
        assert_eq!(late_delay(Some(ms(20)), Some(ms(5))), ms(13));
        assert_eq!(late_delay(Some(ms(5)), Some(ms(20))), Duration::ZERO);
    }

    #[test]
    fn drop_stops_an_idle_converter() {
        struct Idle;
        impl FrameSource for Idle {
            fn next(&self, timeout: Duration) -> Result<Option<Frame>, String> {
                std::thread::sleep(timeout);
                Ok(None)
            }
        }
        let ahead = ConvertAhead::start(Converter::new(x264(), 60_000), Idle, Start::Late).unwrap();
        assert!(ahead.take(Duration::from_millis(30)).is_none());
        let t = Instant::now();
        drop(ahead);
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}
