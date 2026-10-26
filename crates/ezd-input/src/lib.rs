//! ezdisplay client input engine (pure Rust, no IO, no threads, no clock).
//!
//! Turns Android `MotionEvent`s ([`Motion`]) into protocol 0.4 input messages
//! ([`ezd_proto::Message`]: `Pen`, `Touch`, `PointerButton`, `Scroll`, `Key`), with:
//!
//! * palm rejection (fingers are dropped while the pen is near),
//! * S Pen barrel-button gestures (click, double click, hold, drag) mapped to
//!   right click, scrolling, PC zoom, eraser, local zoom, a keyboard shortcut
//!   (tapped, or held while the barrel is held, also after a double click), zoom
//!   steps (a shortcut plus a click per distance dragged) or
//!   raw forwarding,
//! * a local zoom of the view ([`ViewTransform`]) that is undone for the
//!   coordinates sent to the host.
//!
//! Every input carries a timestamp and timers are driven by
//! [`InputEngine::tick`]; [`InputEngine::next_deadline`] says when to call it.
//!
//! * [`android`]: `MotionEvent` constants.
//! * [`InputConfig`]: JSON-configurable behaviour.
//! * [`InputEngine`]: the state machine.

#![warn(missing_docs)]

pub mod android;
mod config;
mod engine;
mod gesture;
mod motion;
mod pen;
mod tilt;
mod touch;
mod view;

pub use config::{ButtonAction, ButtonMap, InputConfig, KeyBindings, KeyCombo, Profile};
pub use engine::InputEngine;
pub use motion::{Motion, MotionStep, PointerSample};
pub use tilt::tilt_xy;
pub use view::ViewTransform;

#[cfg(test)]
mod tests;
