//! Process-wide monotonic clock.

use std::sync::OnceLock;
use std::time::Instant;

static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Microseconds since a process-wide monotonic epoch (first call). Used for
/// all `capture_us`/`encoded_us`/ping timestamps on this side of the link.
pub fn now_us() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

#[cfg(test)]
mod tests {
    #[test]
    fn monotonic() {
        let a = super::now_us();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(super::now_us() >= a + 1500);
    }
}
