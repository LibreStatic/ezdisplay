//! Decoder-agnostic ezdisplay client.
//!
//! [`run_client`] performs the client side of the handshake, runs the receive
//! loop, acknowledges every video frame on reassembly (before any decoding, so
//! the host's video credit is released), answers and sends pings, keeps the
//! cursor cache and sends periodic `ClientStats`. Decoding is left to a
//! [`ClientHandler`]; the app talks back through a [`ClientHandle`].
//!
//! Handler callbacks run on the receive loop and must not block: hand the work
//! to another thread or channel.

mod handle;
mod session;

pub use handle::{ClientHandle, ClientSnapshot, CursorSnapshot};
pub use session::{
    ClientConfig, ClientEnd, ClientError, ClientHandler, ClientOutcome, Flow, HandlerError,
    HandlerResult, run_client,
};

use std::sync::OnceLock;
use std::time::Instant;

/// Microseconds since a process-wide monotonic epoch (first call). All
/// timestamps this client sends (`Ping.t0_us`, `FrameAck.received_us`) use it.
pub fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// Percentile (0..=100) of an unsorted sample set; 0 when empty.
pub fn percentile(samples: &[u32], p: u32) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    let mut v = samples.to_vec();
    v.sort_unstable();
    v[((v.len() - 1) * p as usize) / 100]
}
