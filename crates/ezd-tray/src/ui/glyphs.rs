//! Codepoints of the bundled icon font (`assets/fonts/fluent-icons-subset.ttf`).
//!
//! Put a constant in any text: [`super::fonts::install_fonts`] adds the font as a fallback of both
//! families. The font holds exactly these glyphs; `assets/gen-icon-font.sh` regenerates it.

#![cfg_attr(not(test), allow(dead_code))]

/// Status page and its nav row.
pub const PULSE: &str = "\u{E9DC}";
/// Display page and its nav row.
pub const DESKTOP: &str = "\u{F359}";
/// Input page and its nav row.
pub const PEN: &str = "\u{E8D7}";
/// Connection page and its nav row.
pub const PLUG_CONNECTED: &str = "\u{E999}";
/// General page and its nav row.
pub const SETTINGS: &str = "\u{F6A9}";
/// Display driver page and its nav row.
pub const DEVELOPER_BOARD: &str = "\u{E4AC}";
/// Start button.
pub const PLAY: &str = "\u{F605}";
/// Stop button.
pub const STOP: &str = "\u{F72A}";
/// Confirmation next to a finished action.
pub const CHECKMARK: &str = "\u{F294}";
/// Collapsed or dropdown indicator.
pub const CHEVRON_DOWN: &str = "\u{F2A3}";
/// Row that opens something.
pub const CHEVRON_RIGHT: &str = "\u{F2B0}";
/// Expanded indicator.
pub const CHEVRON_UP: &str = "\u{F2B6}";
/// Warning banners.
pub const WARNING: &str = "\u{F869}";
/// Informational banners.
pub const INFO: &str = "\u{F4A3}";
/// Error banners.
pub const ERROR_CIRCLE: &str = "\u{F3F1}";
/// Success banners and healthy states.
pub const CHECKMARK_CIRCLE: &str = "\u{F298}";
/// Time and latency.
pub const CLOCK: &str = "\u{F2DD}";
/// Opening a folder, such as the logs.
pub const FOLDER_OPEN: &str = "\u{F42E}";
/// Opening an external page or file.
pub const OPEN: &str = "\u{F582}";
/// Repair and troubleshooting actions.
pub const WRENCH: &str = "\u{EE86}";
/// Restart and refresh actions.
pub const ARROW_SYNC: &str = "\u{F190}";
/// The connected tablet.
pub const TABLET: &str = "\u{F779}";
/// No tablet connected.
pub const PLUG_DISCONNECTED: &str = "\u{F609}";
/// Statistics.
pub const DATA_BAR_VERTICAL: &str = "\u{F33F}";
/// Frame rate.
pub const GAUGE: &str = "\u{F4C7}";
/// Latency measurements.
pub const TIMER: &str = "\u{ED88}";
/// Bitrate and data transfer.
pub const ARROW_SWAP: &str = "\u{F18D}";
/// Throughput and speed.
pub const TOP_SPEED: &str = "\u{ED9B}";
/// Video, codec and encoder.
pub const VIDEO: &str = "\u{F84C}";
/// Pointer and touch input.
pub const CURSOR: &str = "\u{E443}";
/// Licenses and documents.
pub const DOCUMENT: &str = "\u{F378}";

/// Every glyph of the font.
#[cfg(test)]
pub const ALL: &[&str] = &[
    PULSE,
    DESKTOP,
    PEN,
    PLUG_CONNECTED,
    SETTINGS,
    DEVELOPER_BOARD,
    PLAY,
    STOP,
    CHECKMARK,
    CHEVRON_DOWN,
    CHEVRON_RIGHT,
    CHEVRON_UP,
    WARNING,
    INFO,
    ERROR_CIRCLE,
    CHECKMARK_CIRCLE,
    CLOCK,
    FOLDER_OPEN,
    OPEN,
    WRENCH,
    ARROW_SYNC,
    TABLET,
    PLUG_DISCONNECTED,
    DATA_BAR_VERTICAL,
    GAUGE,
    TIMER,
    ARROW_SWAP,
    TOP_SPEED,
    VIDEO,
    CURSOR,
    DOCUMENT,
];
