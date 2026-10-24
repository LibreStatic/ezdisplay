//! Decision logic of the Windows input injection, independent of Win32 so it is unit-tested on
//! every OS: coordinate mapping, the pen and touch state machines (which frames to inject, with
//! which `POINTER_FLAGS`), the actions (`SendInput` entries) and the mouse fallback for the pen.
//!
//! `win::input` converts these descriptions into Win32 structs and injects them.

use ezd_proto::{PenSample, PointerButton, Scroll, TouchContact, TouchFrame};
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
}
