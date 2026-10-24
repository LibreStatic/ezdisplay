//! Android `MotionEvent` constants, so callers can pass raw values.

/// `ACTION_DOWN`.
pub const ACTION_DOWN: i32 = 0;
/// `ACTION_UP`.
pub const ACTION_UP: i32 = 1;
/// `ACTION_MOVE`.
pub const ACTION_MOVE: i32 = 2;
/// `ACTION_CANCEL`.
pub const ACTION_CANCEL: i32 = 3;
/// `ACTION_OUTSIDE` (ignored by the engine).
pub const ACTION_OUTSIDE: i32 = 4;
/// `ACTION_POINTER_DOWN`.
pub const ACTION_POINTER_DOWN: i32 = 5;
/// `ACTION_POINTER_UP`.
pub const ACTION_POINTER_UP: i32 = 6;
/// `ACTION_HOVER_MOVE`.
pub const ACTION_HOVER_MOVE: i32 = 7;
/// `ACTION_SCROLL`.
pub const ACTION_SCROLL: i32 = 8;
/// `ACTION_HOVER_ENTER`.
pub const ACTION_HOVER_ENTER: i32 = 9;
/// `ACTION_HOVER_EXIT`.
pub const ACTION_HOVER_EXIT: i32 = 10;
/// `ACTION_BUTTON_PRESS`.
pub const ACTION_BUTTON_PRESS: i32 = 11;
/// `ACTION_BUTTON_RELEASE`.
pub const ACTION_BUTTON_RELEASE: i32 = 12;

/// `TOOL_TYPE_UNKNOWN`.
pub const TOOL_TYPE_UNKNOWN: i32 = 0;
/// `TOOL_TYPE_FINGER`.
pub const TOOL_TYPE_FINGER: i32 = 1;
/// `TOOL_TYPE_STYLUS`.
pub const TOOL_TYPE_STYLUS: i32 = 2;
/// `TOOL_TYPE_MOUSE`.
pub const TOOL_TYPE_MOUSE: i32 = 3;
/// `TOOL_TYPE_ERASER`.
pub const TOOL_TYPE_ERASER: i32 = 4;
/// `TOOL_TYPE_PALM`.
pub const TOOL_TYPE_PALM: i32 = 5;

/// `BUTTON_PRIMARY`.
pub const BUTTON_PRIMARY: i32 = 1;
/// `BUTTON_SECONDARY`.
pub const BUTTON_SECONDARY: i32 = 2;
/// `BUTTON_TERTIARY`.
pub const BUTTON_TERTIARY: i32 = 4;
/// `BUTTON_STYLUS_PRIMARY`.
pub const BUTTON_STYLUS_PRIMARY: i32 = 0x20;
/// `BUTTON_STYLUS_SECONDARY`.
pub const BUTTON_STYLUS_SECONDARY: i32 = 0x40;

/// `FLAG_CANCELED` (`MotionEvent.getFlags()`).
pub const FLAG_CANCELED: i32 = 0x20;
