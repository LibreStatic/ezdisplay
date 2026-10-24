//! Input types mirroring Android's `MotionEvent`, so the Kotlin/JNI side stays dumb.

/// One pointer at one point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerSample {
    /// Android pointer id.
    pub id: i32,
    /// `getToolType()`, see [`crate::android`].
    pub tool_type: i32,
    /// X in view pixels.
    pub x: f32,
    /// Y in view pixels.
    pub y: f32,
    /// `AXIS_PRESSURE`, 0..=1.
    pub pressure: f32,
    /// `AXIS_TILT`, radians 0..=PI/2.
    pub tilt: f32,
    /// `AXIS_ORIENTATION`, radians -PI..=PI.
    pub orientation: f32,
}

/// One time step of a `MotionEvent`: all its pointers.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionStep {
    /// Client monotonic clock, microseconds.
    pub t_us: u64,
    /// All pointers at this time.
    pub pointers: Vec<PointerSample>,
}

/// One `MotionEvent` with its history (oldest step first, the event's own
/// position last).
#[derive(Debug, Clone, PartialEq)]
pub struct Motion {
    /// `getActionMasked()`.
    pub action: i32,
    /// `getActionIndex()` (meaningful for `POINTER_DOWN`/`POINTER_UP`).
    pub action_index: usize,
    /// `getButtonState()`.
    pub button_state: i32,
    /// `getFlags()`.
    pub flags: i32,
    /// `AXIS_VSCROLL` (`ACTION_SCROLL` only), +1 = up.
    pub vscroll: f32,
    /// `AXIS_HSCROLL` (`ACTION_SCROLL` only), +1 = right.
    pub hscroll: f32,
    /// History steps, oldest first.
    pub steps: Vec<MotionStep>,
}
