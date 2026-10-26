//! Decision logic of the Windows input injection, independent of Win32 so it is unit-tested on
//! every OS: coordinate mapping, the pen and touch state machines (which frames to inject, with
//! which `POINTER_FLAGS`), the actions (`SendInput` entries) and the mouse fallback for the pen.
//!
//! `win::input` converts these descriptions into Win32 structs and injects them.

use ezd_proto::{Key, PenSample, PointerButton, Scroll, TouchContact, TouchFrame};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
}

const NORM_MAX: i64 = 65535;

fn div_round(num: i64, den: i64) -> i64 {
    // den > 0 and num >= 0 for every caller.
    (num + den / 2) / den
}

/// Maps a normalized point (0..=65535 over the stream) to a desktop pixel of `rect`; the last
/// pixel is `x + w - 1`.
pub fn to_screen(rect: Rect, nx: u16, ny: u16) -> (i32, i32) {
    let map = |n: u16, origin: i32, extent: i32| {
        let span = i64::from(extent.max(1) - 1);
        origin + div_round(i64::from(n) * span, NORM_MAX) as i32
    };
    (map(nx, rect.x, rect.w), map(ny, rect.y, rect.h))
}

/// Maps a normalized point to the `ptPixelLocation` of an injected synthetic pointer.
/// `InjectSyntheticPointerInput` measures it from the virtual screen's top-left corner, not from
/// the primary monitor's origin, so with a monitor left of or above the primary every pen and
/// touch contact would land off by that monitor's size.
pub fn to_pointer(rect: Rect, virt: Rect, nx: u16, ny: u16) -> (i32, i32) {
    let (x, y) = to_screen(rect, nx, ny);
    (x - virt.x, y - virt.y)
}

/// Maps a desktop pixel to the 0..=65535 range `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`
/// expects over the virtual screen `virt`.
pub fn to_virtual_desk(px: i32, py: i32, virt: Rect) -> (i32, i32) {
    let map = |p: i32, origin: i32, extent: i32| {
        let span = i64::from(extent.max(1) - 1);
        if span == 0 {
            return 0;
        }
        let rel = (i64::from(p) - i64::from(origin)).clamp(0, span);
        div_round(rel * NORM_MAX, span) as i32
    };
    (map(px, virt.x, virt.w), map(py, virt.y, virt.h))
}

/// `POINTER_FLAGS` values (`winuser.h`).
pub mod pf {
    /// `POINTER_FLAG_INRANGE`.
    pub const INRANGE: u32 = 0x0002;
    /// `POINTER_FLAG_INCONTACT`.
    pub const INCONTACT: u32 = 0x0004;
    /// `POINTER_FLAG_CANCELED`.
    pub const CANCELED: u32 = 0x8000;
    /// `POINTER_FLAG_DOWN`.
    pub const DOWN: u32 = 0x0001_0000;
    /// `POINTER_FLAG_UPDATE`.
    pub const UPDATE: u32 = 0x0002_0000;
    /// `POINTER_FLAG_UP`.
    pub const UP: u32 = 0x0004_0000;
}

/// `PEN_FLAGS` values.
pub mod penf {
    /// `PEN_FLAG_BARREL`.
    pub const BARREL: u32 = 0x1;
    /// `PEN_FLAG_INVERTED`.
    pub const INVERTED: u32 = 0x2;
    /// `PEN_FLAG_ERASER`.
    pub const ERASER: u32 = 0x4;
}

/// `PEN_MASK` / `TOUCH_MASK` values.
pub mod mask {
    /// `PEN_MASK_PRESSURE`.
    pub const PEN_PRESSURE: u32 = 0x1;
    /// `PEN_MASK_TILT_X`.
    pub const PEN_TILT_X: u32 = 0x4;
    /// `PEN_MASK_TILT_Y`.
    pub const PEN_TILT_Y: u32 = 0x8;
    /// `TOUCH_MASK_CONTACTAREA`.
    pub const TOUCH_CONTACTAREA: u32 = 0x1;
    /// `TOUCH_MASK_ORIENTATION`.
    pub const TOUCH_ORIENTATION: u32 = 0x2;
    /// `TOUCH_MASK_PRESSURE`.
    pub const TOUCH_PRESSURE: u32 = 0x4;
}

/// Maximum pen pressure of `POINTER_PEN_INFO`.
pub const PEN_PRESSURE_MAX: u32 = 1024;
/// Half size of the contact area of a touch, in pixels.
pub const TOUCH_HALF_AREA: i32 = 2;
/// Orientation (degrees) and pressure reported for touches.
pub const TOUCH_ORIENTATION: u32 = 90;
/// Pressure reported for touches (`TOUCH_MASK_PRESSURE`, 0..=1024).
pub const TOUCH_PRESSURE: u32 = 512;
/// Stationary touches are re-injected as UPDATE after this long without any touch injection.
/// To be confirmed on a real machine; set to `Duration::MAX` to disable.
pub const TOUCH_REFRESH_AFTER: Duration = Duration::from_millis(100);
/// Contacts of the synthetic touch device.
pub const MAX_TOUCH: usize = 10;

/// Protocol pressure (0..=65535) to `POINTER_PEN_INFO` pressure (0..=1024).
pub fn scale_pressure(p: u16) -> u32 {
    div_round(i64::from(p) * i64::from(PEN_PRESSURE_MAX), NORM_MAX) as u32
}

/// Protocol tilt (centidegrees) to degrees, clamped to -90..=90.
pub fn tilt_degrees(t: i16) -> i32 {
    let t = i32::from(t);
    // Round half away from zero.
    let d = if t >= 0 {
        (t + 50) / 100
    } else {
        (t - 50) / 100
    };
    d.clamp(-90, 90)
}

/// One pen injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PenFrame {
    /// `POINTER_INFO.pointerFlags` (see [`pf`]).
    pub flags: u32,
    /// `POINTER_PEN_INFO.penFlags` (see [`penf`]).
    pub pen_flags: u32,
    /// 0..=1024.
    pub pressure: u32,
    /// Degrees, -90..=90.
    pub tilt_x: i32,
    /// Degrees, -90..=90.
    pub tilt_y: i32,
    /// Normalized X.
    pub x: u16,
    /// Normalized Y.
    pub y: u16,
}

/// Turns pen samples into the frames to inject, honouring the rules of
/// `InjectSyntheticPointerInput`: DOWN and UP are never combined with UPDATE, a proximity always
/// starts with a hover frame, and leaving proximity is an UPDATE without INRANGE.
#[derive(Debug, Default)]
pub struct PenTracker {
    in_range: bool,
    contact: bool,
    eraser: bool,
    last: (u16, u16),
    last_tilt: (i32, i32),
}

fn pen_flags(eraser: bool, barrel: bool, contact: bool) -> u32 {
    let mut f = 0;
    if barrel {
        f |= penf::BARREL;
    }
    if eraser {
        f |= penf::INVERTED;
        if contact {
            f |= penf::ERASER;
        }
    }
    f
}

impl PenTracker {
    /// Whether the pen is currently in range.
    pub fn in_range(&self) -> bool {
        self.in_range
    }

    fn frame(&self, flags: u32, barrel: bool, pressure: u32, pos: (u16, u16)) -> PenFrame {
        PenFrame {
            flags,
            pen_flags: pen_flags(self.eraser, barrel, flags & pf::INCONTACT != 0),
            pressure,
            tilt_x: self.last_tilt.0,
            tilt_y: self.last_tilt.1,
            x: pos.0,
            y: pos.1,
        }
    }

    /// Frames to inject for `s`, in order (possibly none).
    pub fn sample(&mut self, s: &PenSample) -> Vec<PenFrame> {
        let in_range = s.flags & (PenSample::IN_RANGE | PenSample::CONTACT) != 0;
        let contact = s.flags & PenSample::CONTACT != 0;
        let eraser = s.flags & PenSample::ERASER != 0;
        let barrel = s.flags & PenSample::BARREL != 0;
        let pos = (s.x, s.y);
        let mut out = Vec::new();
        if !in_range {
            // The leaving sample carries no valid tilt; the last one is reused.
            return self.leave(pos);
        }
        if self.in_range && eraser != self.eraser {
            // Pen <-> eraser: leave range, then enter again with the new flags.
            out.extend(self.leave(pos));
        }
        self.last_tilt = (tilt_degrees(s.tilt_x), tilt_degrees(s.tilt_y));
        let pressure = if contact {
            scale_pressure(s.pressure)
        } else {
            0
        };
        let entered = !self.in_range;
        if entered {
            self.in_range = true;
            self.eraser = eraser;
            out.push(self.frame(pf::INRANGE | pf::UPDATE, barrel, 0, pos));
        }
        match (self.contact, contact) {
            (false, true) => out.push(self.frame(
                pf::INRANGE | pf::INCONTACT | pf::DOWN,
                barrel,
                pressure,
                pos,
            )),
            (true, true) => out.push(self.frame(
                pf::INRANGE | pf::INCONTACT | pf::UPDATE,
                barrel,
                pressure,
                pos,
            )),
            (true, false) => out.push(self.frame(pf::INRANGE | pf::UP, barrel, 0, pos)),
            (false, false) if !entered => {
                out.push(self.frame(pf::INRANGE | pf::UPDATE, barrel, 0, pos))
            }
            (false, false) => {}
        }
        self.contact = contact;
        self.last = pos;
        out
    }

    /// Lifts the pen and leaves range (end of session). Empty if the pen is not in range.
    pub fn release(&mut self) -> Vec<PenFrame> {
        self.leave(self.last)
    }

    /// UP (when in contact) and leave-range frames at `pos`; empty when not in range.
    fn leave(&mut self, pos: (u16, u16)) -> Vec<PenFrame> {
        let mut out = Vec::new();
        if !self.in_range {
            return out;
        }
        if self.contact {
            out.push(self.frame(pf::INRANGE | pf::UP, false, 0, pos));
        }
        out.push(self.frame(pf::UPDATE, false, 0, pos));
        self.in_range = false;
        self.contact = false;
        self.eraser = false;
        out
    }
}

/// One touch contact of an injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchContactOut {
    /// Contact id (becomes the `pointerId`).
    pub id: u8,
    /// `POINTER_INFO.pointerFlags`.
    pub flags: u32,
    /// Normalized X.
    pub x: u16,
    /// Normalized Y.
    pub y: u16,
}

impl TouchContactOut {
    /// DOWN or UP (as opposed to a plain update).
    pub fn is_transition(&self) -> bool {
        self.flags & (pf::DOWN | pf::UP) != 0
    }
}

/// Splits an injection that mixes DOWN/UP and UPDATE contacts into the UPDATE-only part and the
/// DOWN/UP part (the fallback when Windows rejects the mixed array with `E_INVALIDARG`).
/// `None` when the injection is not mixed.
pub fn split_mixed(
    contacts: &[TouchContactOut],
) -> Option<(Vec<TouchContactOut>, Vec<TouchContactOut>)> {
    let (trans, upd): (Vec<_>, Vec<_>) = contacts.iter().copied().partition(|c| c.is_transition());
    (!trans.is_empty() && !upd.is_empty()).then_some((upd, trans))
}

/// Turns touch frames into the contact lists to inject.
#[derive(Debug, Default)]
pub struct TouchTracker {
    active: BTreeMap<u8, (u16, u16)>,
    last_inject: Option<Instant>,
}

impl TouchTracker {
    /// The contacts to inject for `f`: those of the frame in its order, then an UP for every
    /// contact that was active but is no longer listed. Empty = nothing to inject.
    pub fn frame(&mut self, f: &TouchFrame, now: Instant) -> Vec<TouchContactOut> {
        let mut out: Vec<TouchContactOut> = Vec::new();
        let mut listed: Vec<u8> = Vec::new();
        for c in &f.contacts {
            if listed.contains(&c.id) {
                continue;
            }
            listed.push(c.id);
            let was_active = self.active.contains_key(&c.id);
            let flags = match c.phase {
                TouchContact::UP | TouchContact::CANCEL => {
                    if !was_active {
                        continue;
                    }
                    self.active.remove(&c.id);
                    if c.phase == TouchContact::UP {
                        pf::UP
                    } else {
                        pf::UP | pf::CANCELED
                    }
                }
                // DOWN, MOVE and unknown phases: a contact we do not know yet starts with DOWN.
                _ => {
                    if !was_active && self.active.len() >= MAX_TOUCH {
                        continue;
                    }
                    self.active.insert(c.id, (c.x, c.y));
                    if was_active {
                        pf::INRANGE | pf::INCONTACT | pf::UPDATE
                    } else {
                        pf::INRANGE | pf::INCONTACT | pf::DOWN
                    }
                }
            };
            out.push(TouchContactOut {
                id: c.id,
                flags,
                x: c.x,
                y: c.y,
            });
        }
        let stale: Vec<u8> = self
            .active
            .keys()
            .copied()
            .filter(|id| !listed.contains(id))
            .collect();
        for id in stale {
            let (x, y) = self.active.remove(&id).unwrap_or_default();
            out.push(TouchContactOut {
                id,
                flags: pf::UP,
                x,
                y,
            });
        }
        if !out.is_empty() {
            self.last_inject = Some(now);
        }
        out
    }

    /// Stationary contacts as UPDATE if nothing was injected for [`TOUCH_REFRESH_AFTER`].
    pub fn refresh(&mut self, now: Instant) -> Option<Vec<TouchContactOut>> {
        if self.active.is_empty() {
            return None;
        }
        let due = self
            .last_inject
            .is_none_or(|t| now.saturating_duration_since(t) >= TOUCH_REFRESH_AFTER);
        if !due {
            return None;
        }
        self.last_inject = Some(now);
        Some(
            self.active
                .iter()
                .map(|(&id, &(x, y))| TouchContactOut {
                    id,
                    flags: pf::INRANGE | pf::INCONTACT | pf::UPDATE,
                    x,
                    y,
                })
                .collect(),
        )
    }

    /// UP for every active contact (end of session).
    pub fn release_all(&mut self) -> Vec<TouchContactOut> {
        let out = self
            .active
            .iter()
            .map(|(&id, &(x, y))| TouchContactOut {
                id,
                flags: pf::UP,
                x,
                y,
            })
            .collect();
        self.active.clear();
        out
    }
}

/// A keyboard modifier pressed around a wheel event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    /// `VK_CONTROL`.
    Ctrl,
    /// `VK_SHIFT`.
    Shift,
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary.
    Left,
    /// Secondary.
    Right,
    /// Middle.
    Middle,
}

/// One `INPUT` entry of a `SendInput` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseOrKey {
    /// Absolute mouse move to a desktop pixel.
    Move {
        /// Desktop X in pixels.
        x: i32,
        /// Desktop Y in pixels.
        y: i32,
    },
    /// Button press or release.
    Button {
        /// Which button.
        button: MouseButton,
        /// Press (true) or release (false).
        down: bool,
    },
    /// `MOUSEEVENTF_WHEEL`, > 0 = forward (up).
    Wheel(i32),
    /// `MOUSEEVENTF_HWHEEL`, > 0 = right.
    HWheel(i32),
    /// Modifier key press or release.
    Key {
        /// Which modifier.
        key: Modifier,
        /// Press (true) or release (false).
        down: bool,
    },
    /// A keyboard key by PC/AT scancode (`KEYEVENTF_SCANCODE`, no virtual key): the layout is the
    /// user's, like a real keyboard.
    Scan {
        /// Scancode (set 1, without the `E0` prefix).
        scan: u16,
        /// `E0`-prefixed key (`KEYEVENTF_EXTENDEDKEY`).
        extended: bool,
        /// Press (true) or release (false).
        down: bool,
    },
}

/// PC/AT scancode set 1 for a HID keyboard-page usage: `(scancode, extended)` where `extended`
/// means the `E0` prefix. Positional (US layout), same usage set as the Linux host. `None` for
/// unmapped usages: Pause (multi-byte sequence) and Power (not injected on purpose).
pub fn hid_to_scancode(usage: u16) -> Option<(u16, bool)> {
    const LETTERS: [u16; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    let plain = |s: u16| Some((s, false));
    let ext = |s: u16| Some((s, true));
    match usage {
        0x04..=0x1D => plain(LETTERS[usize::from(usage - 0x04)]),
        0x1E..=0x27 => plain(usage - 0x1E + 0x02), // 1..9, 0
        0x28 => plain(0x1C),
        0x29 => plain(0x01),
        0x2A => plain(0x0E),
        0x2B => plain(0x0F),
        0x2C => plain(0x39),
        0x2D => plain(0x0C),
        0x2E => plain(0x0D),
        0x2F => plain(0x1A),
        0x30 => plain(0x1B),
        0x31 | 0x32 => plain(0x2B),
        0x33 => plain(0x27),
        0x34 => plain(0x28),
        0x35 => plain(0x29),
        0x36 => plain(0x33),
        0x37 => plain(0x34),
        0x38 => plain(0x35),
        0x39 => plain(0x3A),
        0x3A..=0x43 => plain(usage - 0x3A + 0x3B), // F1..F10
        0x44 => plain(0x57),
        0x45 => plain(0x58),
        0x46 => ext(0x37), // PrintScreen
        0x47 => plain(0x46),
        0x49 => ext(0x52),   // Insert
        0x4A => ext(0x47),   // Home
        0x4B => ext(0x49),   // PageUp
        0x4C => ext(0x53),   // Delete
        0x4D => ext(0x4F),   // End
        0x4E => ext(0x51),   // PageDown
        0x4F => ext(0x4D),   // Right
        0x50 => ext(0x4B),   // Left
        0x51 => ext(0x50),   // Down
        0x52 => ext(0x48),   // Up
        0x53 => plain(0x45), // NumLock
        0x54 => ext(0x35),   // keypad /
        0x55 => plain(0x37),
        0x56 => plain(0x4A),
        0x57 => plain(0x4E),
        0x58 => ext(0x1C), // keypad Enter
        0x59 => plain(0x4F),
        0x5A => plain(0x50),
        0x5B => plain(0x51),
        0x5C => plain(0x4B),
        0x5D => plain(0x4C),
        0x5E => plain(0x4D),
        0x5F => plain(0x47),
        0x60 => plain(0x48),
        0x61 => plain(0x49),
        0x62 => plain(0x52),
        0x63 => plain(0x53),
        0x64 => plain(0x56),                       // Non-US \
        0x65 => ext(0x5D),                         // Application
        0x67 => plain(0x59),                       // keypad =
        0x68..=0x72 => plain(usage - 0x68 + 0x64), // F13..F23
        0x73 => plain(0x76),                       // F24
        0xE0 => plain(0x1D),
        0xE1 => plain(0x2A),
        0xE2 => plain(0x38),
        0xE3 => ext(0x5B),
        0xE4 => ext(0x1D),
        0xE5 => plain(0x36),
        0xE6 => ext(0x38),
        0xE7 => ext(0x5C),
        _ => None,
    }
}

/// Tracks the keys that are down. A press of a key already down is an autorepeat and gives
/// another key-down; releases of keys that are not down and unmapped usages produce nothing.
#[derive(Debug, Default)]
pub struct KeyTracker {
    /// `(usage, scancode, extended)` in press order.
    down: Vec<(u16, u16, bool)>,
}

impl KeyTracker {
    /// A tracker with nothing pressed.
    pub fn new() -> Self {
        Self::default()
    }

    /// The entries for a key message.
    pub fn key(&mut self, k: &Key) -> Vec<MouseOrKey> {
        let Some((scan, extended)) = hid_to_scancode(k.usage) else {
            return vec![];
        };
        let pos = self.down.iter().position(|d| d.0 == k.usage);
        match (k.pressed, pos) {
            (true, found) => {
                if found.is_none() {
                    self.down.push((k.usage, scan, extended));
                }
                vec![MouseOrKey::Scan {
                    scan,
                    extended,
                    down: true,
                }]
            }
            (false, Some(i)) => {
                self.down.remove(i);
                vec![MouseOrKey::Scan {
                    scan,
                    extended,
                    down: false,
                }]
            }
            _ => vec![],
        }
    }

    /// Releases every key that is down, last pressed first.
    pub fn release_all(&mut self) -> Vec<MouseOrKey> {
        std::mem::take(&mut self.down)
            .into_iter()
            .rev()
            .map(|(_, scan, extended)| MouseOrKey::Scan {
                scan,
                extended,
                down: false,
            })
            .collect()
    }
}

/// Entries for a protocol button event: move to the point, then press or release. Unknown
/// buttons give nothing.
pub fn button_inputs(screen: Rect, b: &PointerButton) -> Vec<MouseOrKey> {
    let button = match b.button {
        PointerButton::LEFT => MouseButton::Left,
        PointerButton::RIGHT => MouseButton::Right,
        PointerButton::MIDDLE => MouseButton::Middle,
        _ => return Vec::new(),
    };
    let (x, y) = to_screen(screen, b.x, b.y);
    vec![
        MouseOrKey::Move { x, y },
        MouseOrKey::Button {
            button,
            down: b.pressed,
        },
    ]
}

/// Entries for a scroll event: move to the point, modifiers down, wheel(s), modifiers up.
/// A scroll without any delta gives nothing.
pub fn scroll_inputs(screen: Rect, s: &Scroll) -> Vec<MouseOrKey> {
    if s.dx == 0 && s.dy == 0 {
        return Vec::new();
    }
    let (x, y) = to_screen(screen, s.x, s.y);
    let mut mods = Vec::new();
    if s.modifiers & Scroll::CTRL != 0 {
        mods.push(Modifier::Ctrl);
    }
    if s.modifiers & Scroll::SHIFT != 0 {
        mods.push(Modifier::Shift);
    }
    let mut v = vec![MouseOrKey::Move { x, y }];
    v.extend(mods.iter().map(|&key| MouseOrKey::Key { key, down: true }));
    if s.dy != 0 {
        v.push(MouseOrKey::Wheel(s.dy));
    }
    if s.dx != 0 {
        v.push(MouseOrKey::HWheel(s.dx));
    }
    v.extend(
        mods.iter()
            .rev()
            .map(|&key| MouseOrKey::Key { key, down: false }),
    );
    v
}

/// The pen as an absolute mouse (`PenMode::Mouse`): CONTACT = left button, BARREL = right button,
/// leaving range releases both.
#[derive(Debug, Default)]
pub struct MousePenTracker {
    in_range: bool,
    left: bool,
    right: bool,
}

impl MousePenTracker {
    /// Whether the pen is currently in range.
    pub fn in_range(&self) -> bool {
        self.in_range
    }

    /// Entries for `s` (possibly none).
    pub fn sample(&mut self, screen: Rect, s: &PenSample) -> Vec<MouseOrKey> {
        let in_range = s.flags & (PenSample::IN_RANGE | PenSample::CONTACT) != 0;
        if !in_range {
            return self.release();
        }
        self.in_range = true;
        let (x, y) = to_screen(screen, s.x, s.y);
        let mut v = vec![MouseOrKey::Move { x, y }];
        let left = s.flags & PenSample::CONTACT != 0;
        let right = s.flags & PenSample::BARREL != 0;
        for (now, was, button) in [
            (left, self.left, MouseButton::Left),
            (right, self.right, MouseButton::Right),
        ] {
            if now != was {
                v.push(MouseOrKey::Button { button, down: now });
            }
        }
        self.left = left;
        self.right = right;
        v
    }

    /// Releases the buttons that are down (no move).
    pub fn release(&mut self) -> Vec<MouseOrKey> {
        let mut v = Vec::new();
        if self.left {
            v.push(MouseOrKey::Button {
                button: MouseButton::Left,
                down: false,
            });
        }
        if self.right {
            v.push(MouseOrKey::Button {
                button: MouseButton::Right,
                down: false,
            });
        }
        *self = MousePenTracker::default();
        v
    }
}

/// Lets a log message through at most once per interval, counting the suppressed ones.
#[derive(Debug)]
pub struct RateLimit {
    every: Duration,
    last: Option<Instant>,
    suppressed: u32,
}

impl RateLimit {
    /// A limiter with the given interval.
    pub fn new(every: Duration) -> Self {
        RateLimit {
            every,
            last: None,
            suppressed: 0,
        }
    }

    /// `Some(n)` = log now, `n` messages were suppressed since the previous one.
    pub fn allow(&mut self, now: Instant) -> Option<u32> {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < self.every)
        {
            self.suppressed += 1;
            return None;
        }
        self.last = Some(now);
        Some(std::mem::take(&mut self.suppressed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOVER: u8 = PenSample::IN_RANGE;
    const TOUCHING: u8 = PenSample::IN_RANGE | PenSample::CONTACT;

    fn pen(x: u16, y: u16, pressure: u16, flags: u8) -> PenSample {
        PenSample {
            t_us: 0,
            x,
            y,
            pressure,
            tilt_x: 0,
            tilt_y: 0,
            flags,
        }
    }

    fn flags(v: &[PenFrame]) -> Vec<u32> {
        v.iter().map(|f| f.flags).collect()
    }

    fn assert_legal(v: &[PenFrame]) {
        for f in v {
            assert!(
                !(f.flags & pf::DOWN != 0 && f.flags & pf::UPDATE != 0),
                "{f:?}"
            );
            assert!(
                !(f.flags & pf::UP != 0 && f.flags & pf::UPDATE != 0),
                "{f:?}"
            );
        }
    }

    const HOVER_F: u32 = pf::INRANGE | pf::UPDATE;
    const DOWN_F: u32 = pf::INRANGE | pf::INCONTACT | pf::DOWN;
    const DRAG_F: u32 = pf::INRANGE | pf::INCONTACT | pf::UPDATE;
    const UP_F: u32 = pf::INRANGE | pf::UP;

    #[test]
    fn pointer_is_relative_to_the_virtual_screen() {
        let tablet = Rect {
            x: -170,
            y: 1080,
            w: 2960,
            h: 1848,
        };
        let virt = Rect {
            x: -1770,
            y: 0,
            w: 4560,
            h: 2928,
        };
        assert_eq!(to_pointer(tablet, virt, 0, 0), (1600, 1080));
        assert_eq!(to_pointer(tablet, virt, 65535, 65535), (4559, 2927));
        let primary_only = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        assert_eq!(to_pointer(primary_only, primary_only, 65535, 0), (1919, 0));
    }

    #[test]
    fn pen_full_stroke() {
        let mut t = PenTracker::default();
        let mut all = Vec::new();
        let mut step = |s: PenSample| {
            let v = t.sample(&s);
            all.extend(v.clone());
            v
        };
        assert_eq!(flags(&step(pen(1, 1, 0, HOVER))), vec![HOVER_F]);
        assert_eq!(flags(&step(pen(2, 2, 0, HOVER))), vec![HOVER_F]);
        assert_eq!(flags(&step(pen(3, 3, 30000, TOUCHING))), vec![DOWN_F]);
        assert_eq!(flags(&step(pen(4, 4, 40000, TOUCHING))), vec![DRAG_F]);
        assert_eq!(flags(&step(pen(5, 5, 0, HOVER))), vec![UP_F]);
        assert_eq!(flags(&step(pen(6, 6, 0, HOVER))), vec![HOVER_F]);
        assert_eq!(flags(&step(pen(7, 7, 0, 0))), vec![pf::UPDATE]);
        assert!(step(pen(8, 8, 0, 0)).is_empty());
        assert_legal(&all);
    }

    #[test]
    fn pen_contact_as_first_sample_hovers_first() {
        let mut t = PenTracker::default();
        let v = t.sample(&pen(10, 20, 65535, TOUCHING));
        assert_eq!(flags(&v), vec![HOVER_F, DOWN_F]);
        assert_eq!(v[0].pressure, 0);
        assert_eq!(v[1].pressure, 1024);
        assert!(v.iter().all(|f| (f.x, f.y) == (10, 20)));
    }

    #[test]
    fn pen_pressure_and_tilt() {
        assert_eq!(scale_pressure(0), 0);
        assert_eq!(scale_pressure(65535), 1024);
        assert_eq!(scale_pressure(32768), 512);
        assert_eq!(tilt_degrees(0), 0);
        assert_eq!(tilt_degrees(4500), 45);
        assert_eq!(tilt_degrees(-4500), -45);
        assert_eq!(tilt_degrees(149), 1);
        assert_eq!(tilt_degrees(-149), -1);
        assert_eq!(tilt_degrees(150), 2);
        assert_eq!(tilt_degrees(9000), 90);
        assert_eq!(tilt_degrees(i16::MAX), 90);
        assert_eq!(tilt_degrees(i16::MIN), -90);
        let mut t = PenTracker::default();
        let mut s = pen(0, 0, 0, HOVER);
        s.tilt_x = -3000;
        s.tilt_y = 12000;
        let v = t.sample(&s);
        assert_eq!((v[0].tilt_x, v[0].tilt_y), (-30, 90));
        assert_eq!(v[0].pressure, 0);
    }

    #[test]
    fn pen_hover_has_zero_pressure_even_if_reported() {
        let mut t = PenTracker::default();
        let v = t.sample(&pen(0, 0, 50000, HOVER));
        assert_eq!(v[0].pressure, 0);
    }

    #[test]
    fn pen_barrel_flag() {
        let mut t = PenTracker::default();
        let v = t.sample(&pen(0, 0, 0, HOVER | PenSample::BARREL));
        assert_eq!(v[0].pen_flags, penf::BARREL);
        let v = t.sample(&pen(0, 0, 100, TOUCHING | PenSample::BARREL));
        assert_eq!(v[0].pen_flags, penf::BARREL);
        let v = t.sample(&pen(0, 0, 100, TOUCHING));
        assert_eq!(v[0].pen_flags, 0);
        assert_eq!(v[0].flags, DRAG_F);
    }

    #[test]
    fn pen_eraser_flags() {
        let mut t = PenTracker::default();
        let v = t.sample(&pen(0, 0, 0, HOVER | PenSample::ERASER));
        assert_eq!(flags(&v), vec![HOVER_F]);
        assert_eq!(v[0].pen_flags, penf::INVERTED);
        let v = t.sample(&pen(0, 0, 100, TOUCHING | PenSample::ERASER));
        assert_eq!(flags(&v), vec![DOWN_F]);
        assert_eq!(v[0].pen_flags, penf::INVERTED | penf::ERASER);
        let v = t.sample(&pen(0, 0, 0, HOVER | PenSample::ERASER));
        assert_eq!(flags(&v), vec![UP_F]);
        assert_eq!(v[0].pen_flags, penf::INVERTED);
    }

    #[test]
    fn pen_eraser_switch_leaves_range_first() {
        let mut t = PenTracker::default();
        t.sample(&pen(0, 0, 0, HOVER));
        let v = t.sample(&pen(1, 1, 0, HOVER | PenSample::ERASER));
        assert_eq!(flags(&v), vec![pf::UPDATE, HOVER_F]);
        assert_eq!(v[0].pen_flags, 0);
        assert_eq!(v[1].pen_flags, penf::INVERTED);
        assert_legal(&v);
        // Back to the tip, this time while touching: lift, leave, enter, down.
        let mut t = PenTracker::default();
        t.sample(&pen(0, 0, 0, TOUCHING | PenSample::ERASER));
        let v = t.sample(&pen(1, 1, 500, TOUCHING));
        assert_eq!(flags(&v), vec![UP_F, pf::UPDATE, HOVER_F, DOWN_F]);
        assert_eq!(v[3].pen_flags, 0);
        assert_legal(&v);
    }

    #[test]
    fn pen_release_from_contact_and_hover() {
        let mut t = PenTracker::default();
        assert!(t.release().is_empty());
        t.sample(&pen(9, 8, 100, TOUCHING));
        let v = t.release();
        assert_eq!(flags(&v), vec![UP_F, pf::UPDATE]);
        assert!(v.iter().all(|f| (f.x, f.y) == (9, 8)));
        assert!(!t.in_range());
        assert!(t.release().is_empty());
        t.sample(&pen(1, 1, 0, HOVER));
        assert_eq!(flags(&t.release()), vec![pf::UPDATE]);
    }

    #[test]
    fn pen_leaving_from_contact_lifts_first() {
        let mut t = PenTracker::default();
        t.sample(&pen(1, 1, 100, TOUCHING));
        let v = t.sample(&pen(2, 2, 0, 0));
        assert_eq!(flags(&v), vec![UP_F, pf::UPDATE]);
        assert_legal(&v);
    }

    fn contact(id: u8, phase: u8) -> TouchContact {
        TouchContact {
            id,
            x: u16::from(id) * 100,
            y: 5,
            phase,
        }
    }

    fn tf(contacts: Vec<TouchContact>) -> TouchFrame {
        TouchFrame { t_us: 0, contacts }
    }

    fn ids_flags(v: &[TouchContactOut]) -> Vec<(u8, u32)> {
        v.iter().map(|c| (c.id, c.flags)).collect()
    }

    const T_DOWN: u32 = pf::INRANGE | pf::INCONTACT | pf::DOWN;
    const T_MOVE: u32 = pf::INRANGE | pf::INCONTACT | pf::UPDATE;

    #[test]
    fn touch_two_contacts_out_of_order() {
        let now = Instant::now();
        let mut t = TouchTracker::default();
        let v = t.frame(&tf(vec![contact(3, TouchContact::DOWN)]), now);
        assert_eq!(ids_flags(&v), vec![(3, T_DOWN)]);
        let v = t.frame(
            &tf(vec![
                contact(3, TouchContact::MOVE),
                contact(1, TouchContact::DOWN),
            ]),
            now,
        );
        assert_eq!(ids_flags(&v), vec![(3, T_MOVE), (1, T_DOWN)]);
        // The first contact lifts while the other moves.
        let v = t.frame(
            &tf(vec![
                contact(3, TouchContact::UP),
                contact(1, TouchContact::MOVE),
            ]),
            now,
        );
        assert_eq!(ids_flags(&v), vec![(3, pf::UP), (1, T_MOVE)]);
        // The lifted contact is gone afterwards.
        let v = t.frame(&tf(vec![contact(1, TouchContact::MOVE)]), now);
        assert_eq!(ids_flags(&v), vec![(1, T_MOVE)]);
        let v = t.frame(&tf(vec![contact(1, TouchContact::UP)]), now);
        assert_eq!(ids_flags(&v), vec![(1, pf::UP)]);
        assert!(t.release_all().is_empty());
    }

    #[test]
    fn touch_cancel_and_unknown_up() {
        let now = Instant::now();
        let mut t = TouchTracker::default();
        t.frame(&tf(vec![contact(0, TouchContact::DOWN)]), now);
        let v = t.frame(&tf(vec![contact(0, TouchContact::CANCEL)]), now);
        assert_eq!(ids_flags(&v), vec![(0, pf::UP | pf::CANCELED)]);
        // UP / CANCEL of a contact we never saw produce nothing.
        assert!(
            t.frame(&tf(vec![contact(7, TouchContact::UP)]), now)
                .is_empty()
        );
        assert!(
            t.frame(&tf(vec![contact(7, TouchContact::CANCEL)]), now)
                .is_empty()
        );
        assert!(t.frame(&tf(vec![]), now).is_empty());
    }

    #[test]
    fn touch_move_of_unknown_contact_starts_with_down() {
        let mut t = TouchTracker::default();
        let v = t.frame(&tf(vec![contact(2, TouchContact::MOVE)]), Instant::now());
        assert_eq!(ids_flags(&v), vec![(2, T_DOWN)]);
        let v = t.frame(&tf(vec![contact(2, TouchContact::DOWN)]), Instant::now());
        assert_eq!(ids_flags(&v), vec![(2, T_MOVE)]);
    }

    #[test]
    fn touch_stale_contacts_are_lifted() {
        let now = Instant::now();
        let mut t = TouchTracker::default();
        t.frame(
            &tf(vec![
                contact(0, TouchContact::DOWN),
                contact(1, TouchContact::DOWN),
            ]),
            now,
        );
        let v = t.frame(&tf(vec![contact(1, TouchContact::MOVE)]), now);
        assert_eq!(ids_flags(&v), vec![(1, T_MOVE), (0, pf::UP)]);
        assert_eq!((v[1].x, v[1].y), (0, 5));
        let v = t.frame(&tf(vec![]), now);
        assert_eq!(ids_flags(&v), vec![(1, pf::UP)]);
        assert!(t.refresh(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn touch_limits_active_contacts() {
        let mut t = TouchTracker::default();
        let many: Vec<_> = (0..12).map(|i| contact(i, TouchContact::DOWN)).collect();
        let v = t.frame(&tf(many), Instant::now());
        assert_eq!(v.len(), MAX_TOUCH);
    }

    #[test]
    fn touch_refresh_only_after_inactivity() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut t = TouchTracker::default();
        assert!(t.refresh(t0).is_none());
        t.frame(&tf(vec![contact(4, TouchContact::DOWN)]), t0);
        assert!(t.refresh(t0 + ms(50)).is_none());
        assert!(t.refresh(t0 + ms(99)).is_none());
        let v = t.refresh(t0 + ms(100)).unwrap();
        assert_eq!(ids_flags(&v), vec![(4, T_MOVE)]);
        assert_eq!((v[0].x, v[0].y), (400, 5));
        // The refresh restarts the timer.
        assert!(t.refresh(t0 + ms(150)).is_none());
        assert!(t.refresh(t0 + ms(200)).is_some());
        // A real frame does too.
        t.frame(&tf(vec![contact(4, TouchContact::MOVE)]), t0 + ms(250));
        assert!(t.refresh(t0 + ms(300)).is_none());
        assert!(t.refresh(t0 + ms(350)).is_some());
    }

    #[test]
    fn touch_release_all() {
        let now = Instant::now();
        let mut t = TouchTracker::default();
        t.frame(
            &tf(vec![
                contact(5, TouchContact::DOWN),
                contact(2, TouchContact::DOWN),
            ]),
            now,
        );
        let v = t.release_all();
        assert_eq!(ids_flags(&v), vec![(2, pf::UP), (5, pf::UP)]);
        assert!(t.refresh(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn mixed_injections_are_split() {
        let c = |id, flags| TouchContactOut {
            id,
            flags,
            x: 0,
            y: 0,
        };
        assert!(split_mixed(&[c(0, T_MOVE), c(1, T_MOVE)]).is_none());
        assert!(split_mixed(&[c(0, T_DOWN), c(1, pf::UP)]).is_none());
        let (upd, trans) = split_mixed(&[c(0, T_MOVE), c(1, T_DOWN), c(2, pf::UP)]).unwrap();
        assert_eq!(ids_flags(&upd), vec![(0, T_MOVE)]);
        assert_eq!(ids_flags(&trans), vec![(1, T_DOWN), (2, pf::UP)]);
    }

    const MON: Rect = Rect {
        x: 1920,
        y: 0,
        w: 2960,
        h: 1848,
    };

    #[test]
    fn coordinates_hit_the_monitor_edges() {
        assert_eq!(to_screen(MON, 0, 0), (1920, 0));
        assert_eq!(to_screen(MON, 65535, 65535), (1920 + 2959, 1847));
        let (cx, cy) = to_screen(MON, 32768, 32768);
        assert!((1920 + 1479..=1920 + 1480).contains(&cx), "{cx}");
        assert!((923..=924).contains(&cy), "{cy}");
        let one = Rect {
            x: 5,
            y: 6,
            w: 1,
            h: 1,
        };
        assert_eq!(to_screen(one, 65535, 65535), (5, 6));
    }

    #[test]
    fn virtual_desk_with_negative_origin() {
        // Monitor to the left of the primary: the virtual desktop starts at -1920.
        let virt = Rect {
            x: -1920,
            y: 0,
            w: 1920 + 2960,
            h: 1848,
        };
        assert_eq!(to_virtual_desk(-1920, 0, virt), (0, 0));
        assert_eq!(to_virtual_desk(2959, 1847, virt), (65535, 65535));
        let (x, _) = to_virtual_desk(0, 0, virt);
        assert_eq!(x, (1920i64 * 65535 + 2439) as i32 / 4879);
        // Out-of-desktop points are clamped.
        assert_eq!(to_virtual_desk(-5000, -5, virt), (0, 0));
        assert_eq!(to_virtual_desk(99999, 99999, virt), (65535, 65535));
        // Round trip of the secondary monitor's edges.
        let m = Rect {
            x: 1920,
            y: 0,
            w: 2960,
            h: 1848,
        };
        let virt = Rect {
            x: 0,
            y: 0,
            w: 1920 + 2960,
            h: 1848,
        };
        let (px, py) = to_screen(m, 65535, 65535);
        assert_eq!(to_virtual_desk(px, py, virt), (65535, 65535));
        let (px, _) = to_screen(m, 0, 0);
        let (vx, _) = to_virtual_desk(px, 0, virt);
        assert_eq!(vx, (1920i64 * 65535 / 4879) as i32 + 1);
        assert_eq!(
            to_virtual_desk(
                3,
                3,
                Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1
                }
            ),
            (0, 0)
        );
    }

    fn click(button: u8, pressed: bool) -> PointerButton {
        PointerButton {
            t_us: 0,
            x: 65535,
            y: 0,
            button,
            pressed,
        }
    }

    #[test]
    fn right_click_inputs() {
        let want_pt = MouseOrKey::Move { x: 4879, y: 0 };
        assert_eq!(
            button_inputs(MON, &click(PointerButton::RIGHT, true)),
            vec![
                want_pt,
                MouseOrKey::Button {
                    button: MouseButton::Right,
                    down: true
                }
            ]
        );
        assert_eq!(
            button_inputs(MON, &click(PointerButton::MIDDLE, false))[1],
            MouseOrKey::Button {
                button: MouseButton::Middle,
                down: false
            }
        );
        assert_eq!(
            button_inputs(MON, &click(PointerButton::LEFT, true))[1],
            MouseOrKey::Button {
                button: MouseButton::Left,
                down: true
            }
        );
        assert!(button_inputs(MON, &click(9, true)).is_empty());
    }

    fn scroll(dx: i32, dy: i32, modifiers: u8) -> Scroll {
        Scroll {
            t_us: 0,
            x: 0,
            y: 65535,
            dx,
            dy,
            modifiers,
        }
    }

    #[test]
    fn ctrl_wheel_inputs() {
        let mv = MouseOrKey::Move { x: 1920, y: 1847 };
        assert_eq!(
            scroll_inputs(MON, &scroll(0, 120, Scroll::CTRL)),
            vec![
                mv,
                MouseOrKey::Key {
                    key: Modifier::Ctrl,
                    down: true
                },
                MouseOrKey::Wheel(120),
                MouseOrKey::Key {
                    key: Modifier::Ctrl,
                    down: false
                },
            ]
        );
        assert_eq!(
            scroll_inputs(MON, &scroll(-60, -30, 0)),
            vec![mv, MouseOrKey::Wheel(-30), MouseOrKey::HWheel(-60)]
        );
        let v = scroll_inputs(MON, &scroll(0, 120, Scroll::CTRL | Scroll::SHIFT));
        assert_eq!(v.len(), 6);
        // Modifiers are released in reverse order.
        assert_eq!(
            v[4],
            MouseOrKey::Key {
                key: Modifier::Shift,
                down: false
            }
        );
        assert!(scroll_inputs(MON, &scroll(0, 0, Scroll::CTRL)).is_empty());
    }

    #[test]
    fn mouse_pen_mode() {
        let b = |button, down| MouseOrKey::Button { button, down };
        let mut t = MousePenTracker::default();
        let v = t.sample(MON, &pen(0, 0, 0, HOVER));
        assert_eq!(v, vec![MouseOrKey::Move { x: 1920, y: 0 }]);
        let v = t.sample(MON, &pen(0, 0, 900, TOUCHING));
        assert_eq!(v[1..], [b(MouseButton::Left, true)]);
        let v = t.sample(MON, &pen(0, 0, 900, TOUCHING | PenSample::BARREL));
        assert_eq!(v[1..], [b(MouseButton::Right, true)]);
        // Dragging: just moves.
        let v = t.sample(MON, &pen(1, 1, 900, TOUCHING | PenSample::BARREL));
        assert_eq!(v.len(), 1);
        // Leaving range releases both, without a move.
        let v = t.sample(MON, &pen(1, 1, 0, 0));
        assert_eq!(
            v,
            vec![b(MouseButton::Left, false), b(MouseButton::Right, false)]
        );
        assert!(!t.in_range());
        assert!(t.sample(MON, &pen(1, 1, 0, 0)).is_empty());
        t.sample(MON, &pen(0, 0, 900, TOUCHING));
        assert_eq!(t.release(), vec![b(MouseButton::Left, false)]);
    }

    #[test]
    fn rate_limit() {
        let t0 = Instant::now();
        let mut r = RateLimit::new(Duration::from_secs(5));
        assert_eq!(r.allow(t0), Some(0));
        assert_eq!(r.allow(t0 + Duration::from_secs(1)), None);
        assert_eq!(r.allow(t0 + Duration::from_secs(2)), None);
        assert_eq!(r.allow(t0 + Duration::from_secs(5)), Some(2));
    }

    #[test]
    fn scancodes() {
        assert_eq!(hid_to_scancode(0x04), Some((0x1E, false))); // A
        assert_eq!(hid_to_scancode(0x1D), Some((0x2C, false))); // Z
        assert_eq!(hid_to_scancode(0x1E), Some((0x02, false))); // 1
        assert_eq!(hid_to_scancode(0x27), Some((0x0B, false))); // 0
        assert_eq!(hid_to_scancode(Key::SPACE), Some((0x39, false)));
        assert_eq!(hid_to_scancode(0x28), Some((0x1C, false))); // Enter
        assert_eq!(hid_to_scancode(0x29), Some((0x01, false))); // Esc
        assert_eq!(hid_to_scancode(0x3A), Some((0x3B, false))); // F1
        assert_eq!(hid_to_scancode(0x43), Some((0x44, false))); // F10
        assert_eq!(hid_to_scancode(0x44), Some((0x57, false))); // F11
        assert_eq!(hid_to_scancode(0x45), Some((0x58, false))); // F12
        assert_eq!(hid_to_scancode(0x68), Some((0x64, false))); // F13
        assert_eq!(hid_to_scancode(0x73), Some((0x76, false))); // F24
        assert_eq!(hid_to_scancode(Key::LEFT_CTRL), Some((0x1D, false)));
        assert_eq!(hid_to_scancode(Key::LEFT_SHIFT), Some((0x2A, false)));
        assert_eq!(hid_to_scancode(Key::LEFT_ALT), Some((0x38, false)));
        assert_eq!(hid_to_scancode(Key::LEFT_GUI), Some((0x5B, true)));
        assert_eq!(hid_to_scancode(0xE4), Some((0x1D, true))); // Right Ctrl
        assert_eq!(hid_to_scancode(0xE5), Some((0x36, false))); // Right Shift
        assert_eq!(hid_to_scancode(0xE6), Some((0x38, true))); // Right Alt
        assert_eq!(hid_to_scancode(0xE7), Some((0x5C, true)));
        assert_eq!(hid_to_scancode(0x52), Some((0x48, true))); // Up
        assert_eq!(hid_to_scancode(0x4C), Some((0x53, true))); // Delete
        assert_eq!(hid_to_scancode(0x58), Some((0x1C, true))); // keypad Enter
        assert_eq!(hid_to_scancode(0x62), Some((0x52, false))); // keypad 0 (not Insert)
        assert_eq!(hid_to_scancode(0x65), Some((0x5D, true))); // Application
        for u in [0x00, 0x03, 0x48, 0x66, 0x74, 0xDF, 0xE8, 0xFFFF] {
            assert_eq!(hid_to_scancode(u), None, "{u:#x}");
        }
    }

    #[test]
    fn key_tracker_rules() {
        let k = |usage, pressed| Key {
            t_us: 0,
            usage,
            pressed,
        };
        let scan = |scan, extended, down| MouseOrKey::Scan {
            scan,
            extended,
            down,
        };
        let mut t = KeyTracker::new();
        assert!(t.key(&k(Key::SPACE, false)).is_empty());
        assert_eq!(
            t.key(&k(Key::LEFT_CTRL, true)),
            vec![scan(0x1D, false, true)]
        );
        assert_eq!(t.key(&k(Key::LEFT_GUI, true)), vec![scan(0x5B, true, true)]);
        assert_eq!(
            t.key(&k(Key::LEFT_GUI, true)),
            vec![scan(0x5B, true, true)],
            "press of a key already down is an autorepeat"
        );
        assert!(t.key(&k(0x03, true)).is_empty(), "unmapped");
        assert_eq!(t.key(&k(Key::SPACE, true)), vec![scan(0x39, false, true)]);
        assert_eq!(t.key(&k(Key::SPACE, false)), vec![scan(0x39, false, false)]);
        assert_eq!(
            t.release_all(),
            vec![scan(0x5B, true, false), scan(0x1D, false, false)]
        );
        assert!(t.release_all().is_empty());
    }
}
