//! Platform-independent host core.
//!
//! Platform crates (Linux KDE, Windows) implement [`VideoSource`] (capture +
//! encode of a virtual display) and [`VirtualDisplay`]; this crate provides the
//! session state machine ([`run_host_session`]) that drives them over a
//! [`ezd_link::Link`].
//!
//! Threading model: the session is async, but the source is owned by one
//! dedicated OS thread that loops `wait for mux video slot -> next_frame -> send`.
//! `next_frame` therefore always runs *after* the link has signalled it can
//! take a frame and must return the newest content (coalescing).

pub mod input;
pub mod platform;
pub mod session;
pub mod source;
pub mod synthetic;
pub mod testing;
pub mod time;

pub use input::{InputBackend, InputSink, InputTarget, PenMode, UnavailableBackend};
pub use platform::{PlatformError, PlatformOptions};
pub use session::{
    EndReason, LiveStats, SessionError, SessionEvent, SessionOptions, SessionSummary,
    run_host_session, run_host_session_with,
};
pub use source::{
    CursorSink, DisplayMode, EncodedFrame, OutputInfo, SourceError, VideoSource, VirtualDisplay,
};
pub use synthetic::{PixelFormat, SyntheticFrames, read_frame_number};
pub use time::now_us;
