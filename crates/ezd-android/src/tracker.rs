//! Maps a frame's presentation timestamp (its host `capture_us`) to the time it was queued
//! to the decoder, so the output thread can compute decode latency.

use std::collections::BTreeMap;
use std::sync::Mutex;

const MAX_PENDING: usize = 256;

/// Thread-safe `pts -> queue time` table; bounded, oldest entries evicted first.
#[derive(Default)]
pub struct Tracker(Mutex<BTreeMap<u64, u64>>);

impl Tracker {
    /// Records that the frame with `pts` was queued at `queued_us`.
    pub fn insert(&self, pts: u64, queued_us: u64) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.insert(pts, queued_us);
        while m.len() > MAX_PENDING {
            m.pop_first();
        }
    }

    /// Removes and returns the queue time of `pts`. Entries older than `pts` are discarded:
    /// the decoder emits in order, so they were dropped inside the codec.
    pub fn take(&self, pts: u64) -> Option<u64> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let v = m.remove(&pts)?;
        let keep = m.split_off(&pts);
        *m = keep;
        Some(v)
    }

    /// Forgets everything (decoder recreated).
    pub fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Pending entries.
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// True when nothing is pending.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_queue_time_and_drops_older() {
        let t = Tracker::default();
        t.insert(10, 100);
        t.insert(20, 200);
        t.insert(30, 300);
        assert_eq!(t.take(20), Some(200));
        assert_eq!(t.len(), 1, "older entry 10 was discarded");
        assert_eq!(t.take(10), None);
        assert_eq!(t.take(30), Some(300));
        assert!(t.is_empty());
    }

    #[test]
    fn bounded() {
        let t = Tracker::default();
        for i in 0..1000 {
            t.insert(i, i);
        }
        assert_eq!(t.len(), MAX_PENDING);
        assert_eq!(t.take(0), None);
        assert_eq!(t.take(999), Some(999));
    }
}
