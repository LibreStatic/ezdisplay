//! NTP-style clock offset estimation from Ping/Pong exchanges.
//!
//! The pinger records `t0` (send) and `t3` (receive) on its own clock; the
//! responder reports `t1` (receive) and `t2` (send) on its clock. Then
//! `rtt = (t3 - t0) - (t2 - t1)` and `offset = ((t1 - t0) + (t2 - t3)) / 2`,
//! where `offset = peer_clock - local_clock`. The estimate is taken from the
//! minimum-RTT sample in a sliding window, which is the least affected by queueing.
//!
//! The crate is pure: callers supply timestamps from their own monotonic clocks.
//! Either side may be the pinger.

use std::collections::VecDeque;

/// One processed Ping/Pong exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Round-trip time excluding responder processing, microseconds.
    pub rtt_us: i64,
    /// `peer_clock - local_clock`, microseconds.
    pub offset_us: i64,
}

/// Sliding-window clock offset estimator.
#[derive(Debug, Clone)]
pub struct ClockSync {
    window: VecDeque<Sample>,
    capacity: usize,
}

impl Default for ClockSync {
    fn default() -> Self {
        Self::new(16)
    }
}

impl ClockSync {
    /// Creates an estimator keeping the last `window` samples (at least 1).
    pub fn new(window: usize) -> Self {
        let capacity = window.max(1);
        Self {
            window: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Adds an exchange. Returns the sample, or `None` (ignored) if the
    /// timestamps are inconsistent (negative RTT or responder time).
    pub fn add_sample(&mut self, t0: u64, t1: u64, t2: u64, t3: u64) -> Option<Sample> {
        let (t0, t1, t2, t3) = (t0 as i128, t1 as i128, t2 as i128, t3 as i128);
        if t3 < t0 || t2 < t1 {
            return None;
        }
        let rtt = (t3 - t0) - (t2 - t1);
        if rtt < 0 {
            return None;
        }
        let offset = ((t1 - t0) + (t2 - t3)) / 2;
        let s = Sample {
            rtt_us: i64::try_from(rtt).ok()?,
            offset_us: i64::try_from(offset).ok()?,
        };
        if self.window.len() == self.capacity {
            self.window.pop_front();
        }
        self.window.push_back(s);
        Some(s)
    }

    /// Minimum-RTT sample of the window (the most recent one on ties).
    pub fn best(&self) -> Option<Sample> {
        self.window.iter().rev().min_by_key(|s| s.rtt_us).copied()
    }

    /// Current offset estimate, `peer_clock - local_clock`, microseconds.
    pub fn offset_us(&self) -> Option<i64> {
        self.best().map(|s| s.offset_us)
    }

    /// RTT of the best sample, microseconds.
    pub fn rtt_us(&self) -> Option<i64> {
        self.best().map(|s| s.rtt_us)
    }

    /// Number of samples currently held.
    pub fn len(&self) -> usize {
        self.window.len()
    }

    /// True when no sample has been added yet.
    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }

    /// Converts a peer timestamp to the local clock (may be negative).
    pub fn peer_to_local(&self, peer_us: u64) -> Option<i64> {
        Some(peer_us as i64 - self.offset_us()?)
    }

    /// Converts a local timestamp to the peer clock.
    pub fn local_to_peer(&self, local_us: u64) -> Option<i64> {
        Some(local_us as i64 + self.offset_us()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates an exchange with the given true offset and one-way delays.
    fn exchange(
        cs: &mut ClockSync,
        t0: u64,
        offset: i64,
        d_out: u64,
        d_back: u64,
        proc: u64,
    ) -> Option<Sample> {
        let t1 = (t0 + d_out) as i64 + offset;
        let t2 = t1 + proc as i64;
        let t3 = t0 + d_out + proc + d_back;
        cs.add_sample(t0, t1 as u64, t2 as u64, t3)
    }

    #[test]
    fn symmetric_delay_is_exact() {
        let mut cs = ClockSync::default();
        let s = exchange(&mut cs, 1_000_000, 5_000_000, 500, 500, 100).unwrap();
        assert_eq!(s.offset_us, 5_000_000);
        assert_eq!(s.rtt_us, 1000);
    }

    #[test]
    fn negative_offset() {
        let mut cs = ClockSync::default();
        exchange(&mut cs, 9_000_000, -3_000_000, 200, 200, 0).unwrap();
        assert_eq!(cs.offset_us(), Some(-3_000_000));
        assert_eq!(cs.peer_to_local(1_000_000), Some(4_000_000));
        assert_eq!(cs.local_to_peer(4_000_000), Some(1_000_000));
    }

    #[test]
    fn asymmetric_delay_error_bounded_by_half_rtt() {
        let mut cs = ClockSync::default();
        let s = exchange(&mut cs, 0, 1_000, 900, 100, 0).unwrap();
        assert!((s.offset_us - 1_000).abs() <= s.rtt_us / 2);
    }

    #[test]
    fn min_rtt_sample_wins() {
        let mut cs = ClockSync::default();
        exchange(&mut cs, 0, 1000, 5000, 9000, 0); // congested, biased
        exchange(&mut cs, 1_000_000, 1000, 300, 300, 0); // clean
        exchange(&mut cs, 2_000_000, 1000, 4000, 100, 0);
        assert_eq!(cs.offset_us(), Some(1000));
        assert_eq!(cs.rtt_us(), Some(600));
    }

    #[test]
    fn window_slides() {
        let mut cs = ClockSync::new(2);
        exchange(&mut cs, 0, 1000, 100, 100, 0); // best, will be evicted
        exchange(&mut cs, 1_000_000, 2000, 500, 500, 0);
        exchange(&mut cs, 2_000_000, 2000, 600, 600, 0);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs.rtt_us(), Some(1000));
        assert_eq!(cs.offset_us(), Some(2000));
    }

    #[test]
    fn invalid_samples_ignored() {
        let mut cs = ClockSync::default();
        assert!(cs.add_sample(100, 0, 0, 50).is_none()); // t3 < t0
        assert!(cs.add_sample(0, 100, 50, 200).is_none()); // t2 < t1
        assert!(cs.add_sample(0, 0, 500, 100).is_none()); // processing longer than rtt
        assert!(cs.is_empty());
        assert_eq!(cs.offset_us(), None);
    }
}
