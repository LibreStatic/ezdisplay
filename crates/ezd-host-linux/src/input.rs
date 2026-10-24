//! Pure encoders from protocol input messages to evdev event frames (no device needed).
//!
//! A frame is the list of events between two `SYN_REPORT`s; the uinput writer appends the sync.
//! The encoders keep the state that evdev needs (active slots, tool in proximity, buttons down).

use crate::kscreen::KOutput;
use evdev::{AbsoluteAxisCode as Abs, EventType, KeyCode as Key, RelativeAxisCode as Rel};
use ezd_proto::{PenSample, PointerButton, Scroll, TouchContact, TouchFrame};

/// One evdev event: `(type, code, value)`.
pub type Ev = (u16, u16, i32);
/// Events between two `SYN_REPORT`s.
pub type Frame = Vec<Ev>;

fn abs(code: Abs, v: i32) -> Ev {
    (EventType::ABSOLUTE.0, code.0, v)
}
fn key(code: Key, v: i32) -> Ev {
    (EventType::KEY.0, code.0, v)
}
fn rel(code: Rel, v: i32) -> Ev {
    (EventType::RELATIVE.0, code.0, v)
}
fn key_code(code: u16, v: i32) -> Ev {
    (EventType::KEY.0, code, v)
}

/// `ABS_MT_TOOL_TYPE` values.
const MT_TOOL_FINGER: i32 = 0;
const MT_TOOL_PALM: i32 = 2;

/// Physical size used when the client does not report a usable display size.
const FALLBACK_MM: (f64, f64) = (268.5, 167.6);

/// Physical size of the client display in millimetres.
pub fn physical_mm(width_px: u32, height_px: u32, dpi: u32) -> (f64, f64) {
    if width_px == 0 || height_px == 0 || dpi == 0 {
        return FALLBACK_MM;
    }
    let mm = |px: u32| f64::from(px) / f64::from(dpi) * 25.4;
    (mm(width_px), mm(height_px))
}

/// Axis resolution (units per millimetre) for the 0..=65535 range over `mm`.
pub fn resolution(mm: f64) -> i32 {
    (65535.0 / mm.max(1.0)).round() as i32
}

// ---------------------------------------------------------------- touch

/// Maximum number of simultaneous contacts (`ABS_MT_SLOT` range 0..=9).
pub const MAX_SLOTS: usize = 10;

/// Touchscreen encoder (multi-touch protocol B).
#[derive(Debug, Default)]
pub struct TouchEncoder {
    slots: [Option<(u16, u16)>; MAX_SLOTS],
    next_tracking_id: i32,
    touching: bool,
    single: Option<(u16, u16)>,
}

impl TouchEncoder {
    /// Creates an encoder with no active contacts.
    pub fn new() -> Self {
        Self::default()
    }

    fn new_tracking_id(&mut self) -> i32 {
        let id = self.next_tracking_id;
        self.next_tracking_id = (id + 1) & 0xffff;
        id
    }

    /// Encodes one touch frame (one SYN frame, or nothing if nothing changed).
    pub fn touch(&mut self, f: &TouchFrame) -> Vec<Frame> {
        let mut ev: Frame = Vec::new();
        for c in &f.contacts {
            let slot = usize::from(c.id);
            if slot >= MAX_SLOTS {
                continue;
            }
            let pos = (c.x, c.y);
            match c.phase {
                TouchContact::UP => {
                    if self.slots[slot].take().is_some() {
                        ev.push(abs(Abs::ABS_MT_SLOT, slot as i32));
                        ev.push(abs(Abs::ABS_MT_TRACKING_ID, -1));
                    }
                }
                TouchContact::CANCEL => {
                    if self.slots[slot].take().is_some() {
                        ev.push(abs(Abs::ABS_MT_SLOT, slot as i32));
                        ev.push(abs(Abs::ABS_MT_TOOL_TYPE, MT_TOOL_PALM));
                        ev.push(abs(Abs::ABS_MT_TRACKING_ID, -1));
                    }
                }
                // DOWN, MOVE and unknown phases: a contact that is active moves, a new one
                // goes down, whatever the phase says.
                _ => match self.slots[slot] {
                    Some(old) => {
                        if old != pos {
                            ev.push(abs(Abs::ABS_MT_SLOT, slot as i32));
                            ev.push(abs(Abs::ABS_MT_POSITION_X, i32::from(pos.0)));
                            ev.push(abs(Abs::ABS_MT_POSITION_Y, i32::from(pos.1)));
                            self.slots[slot] = Some(pos);
                        }
                    }
                    None => {
                        let id = self.new_tracking_id();
                        ev.push(abs(Abs::ABS_MT_SLOT, slot as i32));
                        ev.push(abs(Abs::ABS_MT_TRACKING_ID, id));
                        ev.push(abs(Abs::ABS_MT_POSITION_X, i32::from(pos.0)));
                        ev.push(abs(Abs::ABS_MT_POSITION_Y, i32::from(pos.1)));
                        ev.push(abs(Abs::ABS_MT_TOOL_TYPE, MT_TOOL_FINGER));
                        self.slots[slot] = Some(pos);
                    }
                },
            }
        }
        self.finish_frame(ev)
    }

    /// Appends BTN_TOUCH and the single-touch emulation to `ev`.
    fn finish_frame(&mut self, mut ev: Frame) -> Vec<Frame> {
        let any = self.slots.iter().any(Option::is_some);
        if any != self.touching {
            self.touching = any;
            ev.push(key(Key::BTN_TOUCH, i32::from(any)));
        }
        let lowest = self.slots.iter().flatten().next().copied();
        if lowest != self.single {
            if let Some((x, y)) = lowest {
                ev.push(abs(Abs::ABS_X, i32::from(x)));
                ev.push(abs(Abs::ABS_Y, i32::from(y)));
            }
            self.single = lowest;
        }
        if ev.is_empty() { vec![] } else { vec![ev] }
    }

    /// Lifts every active contact.
    pub fn release_all(&mut self) -> Vec<Frame> {
        let mut ev: Frame = Vec::new();
        for (i, s) in self.slots.iter_mut().enumerate() {
            if s.take().is_some() {
                ev.push(abs(Abs::ABS_MT_SLOT, i as i32));
                ev.push(abs(Abs::ABS_MT_TRACKING_ID, -1));
            }
        }
        self.finish_frame(ev)
    }
}

// ---------------------------------------------------------------- pen

/// Pressure range of the pen device (`ABS_PRESSURE` 0..=PRESSURE_MAX).
pub const PRESSURE_MAX: i32 = 4095;

/// Which evdev button the barrel button emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenButton {
    /// `BTN_STYLUS` (middle click in Qt / Xwayland).
    Stylus,
    /// `BTN_STYLUS2` (right click in Qt / Xwayland). The default.
    Stylus2,
}

impl PenButton {
    /// Parses `EZD_PEN_BUTTON` (`stylus` or `stylus2`; anything else = default).
    pub fn from_env_value(v: Option<&str>) -> PenButton {
        match v.map(str::trim) {
            Some(s) if s.eq_ignore_ascii_case("stylus") => PenButton::Stylus,
            _ => PenButton::Stylus2,
        }
    }

    fn code(self) -> u16 {
        match self {
            PenButton::Stylus => Key::BTN_STYLUS.0,
            PenButton::Stylus2 => Key::BTN_STYLUS2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Pen,
    Eraser,
}

impl Tool {
    fn code(self) -> Key {
        match self {
            Tool::Pen => Key::BTN_TOOL_PEN,
            Tool::Eraser => Key::BTN_TOOL_RUBBER,
        }
    }
}

/// Graphics tablet encoder.
#[derive(Debug)]
pub struct PenEncoder {
    barrel: PenButton,
    tool: Option<Tool>,
    contact: bool,
    barrel_down: bool,
    pos: (u16, u16),
    tilt: (i32, i32),
    pressure: i32,
}

/// Rescales contact pressure so the tip is down for any contact: 6..=100 % of the range.
pub fn contact_pressure(p: u16) -> i32 {
    ((0.06 + 0.94 * f64::from(p) / 65535.0) * f64::from(PRESSURE_MAX)).round() as i32
}

impl PenEncoder {
    /// Creates an encoder with the pen out of proximity.
    pub fn new(barrel: PenButton) -> Self {
        PenEncoder {
            barrel,
            tool: None,
            contact: false,
            barrel_down: false,
            pos: (0, 0),
            tilt: (0, 0),
            pressure: 0,
        }
    }

    fn leave(&mut self, out: &mut Vec<Frame>) {
        let Some(tool) = self.tool.take() else {
            return;
        };
        if self.contact || self.barrel_down {
            let mut f = Frame::new();
            if self.contact {
                f.push(key(Key::BTN_TOUCH, 0));
                f.push(abs(Abs::ABS_PRESSURE, 0));
            }
            if self.barrel_down {
                f.push(key_code(self.barrel.code(), 0));
            }
            out.push(f);
        }
        out.push(vec![key(tool.code(), 0)]);
        self.contact = false;
        self.barrel_down = false;
        self.pressure = 0;
    }

    /// Encodes one pen sample.
    pub fn pen(&mut self, s: &PenSample) -> Vec<Frame> {
        let mut out = Vec::new();
        if s.flags & PenSample::IN_RANGE == 0 {
            self.leave(&mut out);
            return out;
        }
        let want = if s.flags & PenSample::ERASER != 0 {
            Tool::Eraser
        } else {
            Tool::Pen
        };
        if self.tool.is_some() && self.tool != Some(want) {
            self.leave(&mut out);
        }
        let tilt = (
            i32::from(s.tilt_x).clamp(-9000, 9000),
            i32::from(s.tilt_y).clamp(-9000, 9000),
        );
        if self.tool.is_none() {
            self.tool = Some(want);
            self.pos = (s.x, s.y);
            self.tilt = tilt;
            self.pressure = 0;
            out.push(vec![
                key(want.code(), 1),
                abs(Abs::ABS_X, i32::from(s.x)),
                abs(Abs::ABS_Y, i32::from(s.y)),
                abs(Abs::ABS_TILT_X, tilt.0),
                abs(Abs::ABS_TILT_Y, tilt.1),
                abs(Abs::ABS_PRESSURE, 0),
                abs(Abs::ABS_DISTANCE, 1),
            ]);
        }
        let mut f = Frame::new();
        if (s.x, s.y) != self.pos {
            self.pos = (s.x, s.y);
            f.push(abs(Abs::ABS_X, i32::from(s.x)));
            f.push(abs(Abs::ABS_Y, i32::from(s.y)));
        }
        if tilt != self.tilt {
            self.tilt = tilt;
            f.push(abs(Abs::ABS_TILT_X, tilt.0));
            f.push(abs(Abs::ABS_TILT_Y, tilt.1));
        }
        let contact = s.flags & PenSample::CONTACT != 0;
        let pressure = if contact {
            contact_pressure(s.pressure)
        } else {
            0
        };
        if pressure != self.pressure {
            self.pressure = pressure;
            f.push(abs(Abs::ABS_PRESSURE, pressure));
        }
        if contact != self.contact {
            f.push(abs(Abs::ABS_DISTANCE, i32::from(!contact)));
        }
        let barrel = s.flags & PenSample::BARREL != 0;
        if barrel != self.barrel_down {
            self.barrel_down = barrel;
            f.push(key_code(self.barrel.code(), i32::from(barrel)));
        }
        if contact != self.contact {
            self.contact = contact;
            f.push(key(Key::BTN_TOUCH, i32::from(contact)));
        }
        if !f.is_empty() {
            out.push(f);
        }
        out
    }

    /// Takes the pen out of proximity.
    pub fn release_all(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        self.leave(&mut out);
        out
    }
}

// ---------------------------------------------------------------- pointer

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Maps stream-normalized coordinates (0..=65535 over the virtual output) to the pointer
/// device's range, which KWin scales over the whole workspace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapper {
    out: Option<Rect>,
    ws: Rect,
    /// The workspace's bounding box does not start at (0, 0): the pointer lands off by that
    /// offset because KWin does not add it.
    pub origin_offset: bool,
}

impl Mapper {
    /// Passes coordinates through (the output geometry is unknown).
    pub fn identity() -> Mapper {
        Mapper {
            out: None,
            ws: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            origin_offset: false,
        }
    }

    /// True if the output geometry was found (not the identity mapping).
    pub fn is_known(&self) -> bool {
        self.out.is_some()
    }

    fn logical(o: &KOutput) -> Rect {
        let scale = if o.scale > 0.0 { o.scale } else { 1.0 };
        Rect {
            x: f64::from(o.pos.0),
            y: f64::from(o.pos.1),
            w: f64::from(o.size.0) / scale,
            h: f64::from(o.size.1) / scale,
        }
    }

    /// Builds the mapping for the output called `name` from `kscreen-doctor` data.
    pub fn from_outputs(outputs: &[KOutput], name: &str) -> Mapper {
        let Some(target) = outputs.iter().find(|o| o.name == name) else {
            return Mapper::identity();
        };
        let out = Self::logical(target);
        if out.w <= 0.0 || out.h <= 0.0 {
            return Mapper::identity();
        }
        let (mut x0, mut y0, mut x1, mut y1) = (out.x, out.y, out.x + out.w, out.y + out.h);
        for o in outputs.iter().filter(|o| o.enabled && o.name != name) {
            let r = Self::logical(o);
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.x + r.w);
            y1 = y1.max(r.y + r.h);
        }
        Mapper {
            out: Some(out),
            ws: Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            },
            origin_offset: x0 != 0.0 || y0 != 0.0,
        }
    }

    /// Maps a stream-normalized point to the pointer device's 0..=65535 range.
    pub fn map(&self, x: u16, y: u16) -> (u16, u16) {
        let Some(o) = self.out else {
            return (x, y);
        };
        let px = o.x + f64::from(x) / 65535.0 * o.w;
        let py = o.y + f64::from(y) / 65535.0 * o.h;
        let conv = |p: f64, origin: f64, size: f64| -> u16 {
            ((p - origin) / size * 65535.0).round().clamp(0.0, 65535.0) as u16
        };
        (
            conv(px, self.ws.x, self.ws.w),
            conv(py, self.ws.y, self.ws.h),
        )
    }
}

/// Absolute pointer encoder (buttons, wheel, modifiers).
#[derive(Debug, Default)]
pub struct PointerEncoder {
    buttons: [bool; 3],
    mods: [bool; 2],
    wheel_acc: (i32, i32),
}

const BUTTONS: [Key; 3] = [Key::BTN_LEFT, Key::BTN_RIGHT, Key::BTN_MIDDLE];

impl PointerEncoder {
    /// Creates an encoder with nothing pressed.
    pub fn new() -> Self {
        Self::default()
    }

    fn position(p: (u16, u16)) -> Frame {
        vec![
            abs(Abs::ABS_X, i32::from(p.0)),
            abs(Abs::ABS_Y, i32::from(p.1)),
        ]
    }

    /// A button press or release at `(x, y)`: a move frame, then the button frame.
    pub fn button(&mut self, b: &PointerButton, m: &Mapper) -> Vec<Frame> {
        let mut out = vec![Self::position(m.map(b.x, b.y))];
        let i = usize::from(b.button);
        if i < BUTTONS.len() {
            self.buttons[i] = b.pressed;
            out.push(vec![key(BUTTONS[i], i32::from(b.pressed))]);
        }
        out
    }

    /// A wheel event at `(x, y)`, with Ctrl / Shift held around it.
    pub fn scroll(&mut self, s: &Scroll, m: &Mapper) -> Vec<Frame> {
        let mut out = vec![Self::position(m.map(s.x, s.y))];
        if s.dx == 0 && s.dy == 0 {
            return out;
        }
        let mods: Vec<(usize, Key)> = [
            (Scroll::CTRL, Key::KEY_LEFTCTRL),
            (Scroll::SHIFT, Key::KEY_LEFTSHIFT),
        ]
        .into_iter()
        .enumerate()
        .filter(|(_, (bit, _))| s.modifiers & bit != 0)
        .map(|(i, (_, k))| (i, k))
        .collect();
        if !mods.is_empty() {
            out.push(mods.iter().map(|&(_, k)| key(k, 1)).collect());
            for &(i, _) in &mods {
                self.mods[i] = true;
            }
        }
        let mut w = Frame::new();
        if s.dy != 0 {
            w.push(rel(Rel::REL_WHEEL_HI_RES, s.dy));
        }
        if s.dx != 0 {
            w.push(rel(Rel::REL_HWHEEL_HI_RES, s.dx));
        }
        let ny = Self::notches(&mut self.wheel_acc.1, s.dy);
        let nx = Self::notches(&mut self.wheel_acc.0, s.dx);
        if ny != 0 {
            w.push(rel(Rel::REL_WHEEL, ny));
        }
        if nx != 0 {
            w.push(rel(Rel::REL_HWHEEL, nx));
        }
        out.push(w);
        if !mods.is_empty() {
            out.push(mods.iter().map(|&(_, k)| key(k, 0)).collect());
            for &(i, _) in &mods {
                self.mods[i] = false;
            }
        }
        out
    }

    /// Whole notches in `acc + delta` (1 notch = 120); the remainder stays in `acc`, which is
    /// reset when the direction flips.
    fn notches(acc: &mut i32, delta: i32) -> i32 {
        if delta == 0 {
            return 0;
        }
        if (*acc > 0) != (delta > 0) && *acc != 0 {
            *acc = 0;
        }
        *acc += delta;
        let n = *acc / 120;
        *acc -= n * 120;
        n
    }

    /// Pen samples in `--pen-mode mouse`: move, left button = contact, right = barrel.
    pub fn pen_as_mouse(&mut self, s: &PenSample, m: &Mapper) -> Vec<Frame> {
        let mut out = Vec::new();
        let in_range = s.flags & PenSample::IN_RANGE != 0;
        let want = if in_range {
            out.push(Self::position(m.map(s.x, s.y)));
            [
                s.flags & PenSample::CONTACT != 0,
                s.flags & PenSample::BARREL != 0,
            ]
        } else {
            [false, false]
        };
        let mut f = Frame::new();
        for (i, &w) in want.iter().enumerate() {
            if self.buttons[i] != w {
                self.buttons[i] = w;
                f.push(key(BUTTONS[i], i32::from(w)));
            }
        }
        if !f.is_empty() {
            out.push(f);
        }
        out
    }

    /// Releases the buttons and modifiers that are down.
    pub fn release_all(&mut self) -> Vec<Frame> {
        let mut f = Frame::new();
        for (i, b) in self.buttons.iter_mut().enumerate() {
            if std::mem::take(b) {
                f.push(key(BUTTONS[i], 0));
            }
        }
        for (i, b) in self.mods.iter_mut().enumerate() {
            if std::mem::take(b) {
                f.push(key([Key::KEY_LEFTCTRL, Key::KEY_LEFTSHIFT][i], 0));
            }
        }
        self.wheel_acc = (0, 0);
        if f.is_empty() { vec![] } else { vec![f] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u16 = EventType::ABSOLUTE.0;
    const K: u16 = EventType::KEY.0;
    const R: u16 = EventType::RELATIVE.0;

    fn c(id: u8, x: u16, y: u16, phase: u8) -> TouchContact {
        TouchContact { id, x, y, phase }
    }
    fn tf(contacts: Vec<TouchContact>) -> TouchFrame {
        TouchFrame { t_us: 0, contacts }
    }
    fn ps(x: u16, y: u16, pressure: u16, flags: u8) -> PenSample {
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
    const HOVER: u8 = PenSample::IN_RANGE;
    const TOUCH: u8 = PenSample::IN_RANGE | PenSample::CONTACT;

    fn has(f: &Frame, ev: Ev) -> bool {
        f.contains(&ev)
    }
    fn code(c: Abs) -> u16 {
        c.0
    }

    #[test]
    fn touch_down_move_up() {
        let mut e = TouchEncoder::new();
        let d = e.touch(&tf(vec![c(0, 10, 20, TouchContact::DOWN)]));
        assert_eq!(
            d,
            vec![vec![
                (A, code(Abs::ABS_MT_SLOT), 0),
                (A, code(Abs::ABS_MT_TRACKING_ID), 0),
                (A, code(Abs::ABS_MT_POSITION_X), 10),
                (A, code(Abs::ABS_MT_POSITION_Y), 20),
                (A, code(Abs::ABS_MT_TOOL_TYPE), 0),
                (K, Key::BTN_TOUCH.0, 1),
                (A, code(Abs::ABS_X), 10),
                (A, code(Abs::ABS_Y), 20),
            ]]
        );
        let m = e.touch(&tf(vec![c(0, 11, 20, TouchContact::MOVE)]));
        assert_eq!(
            m,
            vec![vec![
                (A, code(Abs::ABS_MT_SLOT), 0),
                (A, code(Abs::ABS_MT_POSITION_X), 11),
                (A, code(Abs::ABS_MT_POSITION_Y), 20),
                (A, code(Abs::ABS_X), 11),
                (A, code(Abs::ABS_Y), 20),
            ]]
        );
        // Unchanged contact: nothing to send.
        assert!(
            e.touch(&tf(vec![c(0, 11, 20, TouchContact::MOVE)]))
                .is_empty()
        );
        let u = e.touch(&tf(vec![c(0, 11, 20, TouchContact::UP)]));
        assert_eq!(
            u,
            vec![vec![
                (A, code(Abs::ABS_MT_SLOT), 0),
                (A, code(Abs::ABS_MT_TRACKING_ID), -1),
                (K, Key::BTN_TOUCH.0, 0),
            ]]
        );
        // The next contact gets a new tracking id.
        let d2 = e.touch(&tf(vec![c(0, 1, 1, TouchContact::DOWN)]));
        assert!(has(&d2[0], (A, code(Abs::ABS_MT_TRACKING_ID), 1)));
    }

    #[test]
    fn touch_cancel_marks_palm_first() {
        let mut e = TouchEncoder::new();
        e.touch(&tf(vec![c(3, 5, 5, TouchContact::DOWN)]));
        let x = e.touch(&tf(vec![c(3, 5, 5, TouchContact::CANCEL)]));
        assert_eq!(
            x,
            vec![vec![
                (A, code(Abs::ABS_MT_SLOT), 3),
                (A, code(Abs::ABS_MT_TOOL_TYPE), 2),
                (A, code(Abs::ABS_MT_TRACKING_ID), -1),
                (K, Key::BTN_TOUCH.0, 0),
            ]]
        );
    }

    #[test]
    fn two_contacts_single_touch_follows_lowest_slot() {
        let mut e = TouchEncoder::new();
        e.touch(&tf(vec![c(1, 100, 100, TouchContact::DOWN)]));
        let f = e.touch(&tf(vec![
            c(0, 7, 8, TouchContact::DOWN),
            c(1, 100, 100, TouchContact::MOVE),
        ]));
        let f = &f[0];
        assert!(has(f, (A, code(Abs::ABS_MT_SLOT), 0)));
        assert!(has(f, (A, code(Abs::ABS_X), 7)));
        // BTN_TOUCH was already down.
        assert!(!f.iter().any(|e| e.0 == K));
        // Slot 0 goes up: the single-touch position falls back to slot 1.
        let f = e.touch(&tf(vec![
            c(0, 7, 8, TouchContact::UP),
            c(1, 100, 100, TouchContact::MOVE),
        ]));
        assert!(has(&f[0], (A, code(Abs::ABS_X), 100)));
        assert!(!f[0].iter().any(|e| e.0 == K));
    }

    #[test]
    fn touch_unknown_phase_and_missing_slot() {
        let mut e = TouchEncoder::new();
        // MOVE for a slot that is not active = DOWN.
        let f = e.touch(&tf(vec![c(2, 9, 9, TouchContact::MOVE)]));
        assert!(has(&f[0], (A, code(Abs::ABS_MT_TRACKING_ID), 0)));
        // DOWN for an active slot = MOVE (no new tracking id).
        let f = e.touch(&tf(vec![c(2, 10, 9, TouchContact::DOWN)]));
        assert!(!f[0].iter().any(|e| e.1 == code(Abs::ABS_MT_TRACKING_ID)));
        assert!(has(&f[0], (A, code(Abs::ABS_MT_POSITION_X), 10)));
        // Unknown phase = MOVE.
        let f = e.touch(&tf(vec![c(2, 11, 9, 77)]));
        assert!(has(&f[0], (A, code(Abs::ABS_MT_POSITION_X), 11)));
        // UP of an unknown slot and ids >= 10 are ignored.
        assert!(
            e.touch(&tf(vec![
                c(5, 1, 1, TouchContact::UP),
                c(10, 1, 1, TouchContact::DOWN)
            ]))
            .is_empty()
        );
    }

    #[test]
    fn touch_release_all() {
        let mut e = TouchEncoder::new();
        e.touch(&tf(vec![
            c(0, 1, 1, TouchContact::DOWN),
            c(4, 2, 2, TouchContact::DOWN),
        ]));
        let f = e.release_all();
        assert_eq!(
            f,
            vec![vec![
                (A, code(Abs::ABS_MT_SLOT), 0),
                (A, code(Abs::ABS_MT_TRACKING_ID), -1),
                (A, code(Abs::ABS_MT_SLOT), 4),
                (A, code(Abs::ABS_MT_TRACKING_ID), -1),
                (K, Key::BTN_TOUCH.0, 0),
            ]]
        );
        assert!(e.release_all().is_empty());
    }

    #[test]
    fn pen_proximity_then_contact() {
        let mut e = PenEncoder::new(PenButton::Stylus2);
        let f = e.pen(&ps(100, 200, 0, HOVER));
        assert_eq!(
            f,
            vec![vec![
                (K, Key::BTN_TOOL_PEN.0, 1),
                (A, code(Abs::ABS_X), 100),
                (A, code(Abs::ABS_Y), 200),
                (A, code(Abs::ABS_TILT_X), 0),
                (A, code(Abs::ABS_TILT_Y), 0),
                (A, code(Abs::ABS_PRESSURE), 0),
                (A, code(Abs::ABS_DISTANCE), 1),
            ]]
        );
        // First sample is already a contact: prox-in first, contact in its own frame.
        let mut e = PenEncoder::new(PenButton::Stylus2);
        let f = e.pen(&ps(100, 200, 65535, TOUCH));
        assert_eq!(f.len(), 2);
        assert!(has(&f[0], (A, code(Abs::ABS_PRESSURE), 0)));
        assert!(has(&f[1], (A, code(Abs::ABS_PRESSURE), 4095)));
        assert!(has(&f[1], (A, code(Abs::ABS_DISTANCE), 0)));
        assert!(has(&f[1], (K, Key::BTN_TOUCH.0, 1)));
        // Lift: pressure 0, distance 1.
        let f = e.pen(&ps(100, 200, 0, HOVER));
        assert_eq!(f.len(), 1);
        assert!(has(&f[0], (A, code(Abs::ABS_PRESSURE), 0)));
        assert!(has(&f[0], (A, code(Abs::ABS_DISTANCE), 1)));
        assert!(has(&f[0], (K, Key::BTN_TOUCH.0, 0)));
        // Hover moves report no pressure.
        let f = e.pen(&ps(101, 200, 500, HOVER));
        assert_eq!(
            f,
            vec![vec![(A, code(Abs::ABS_X), 101), (A, code(Abs::ABS_Y), 200)]]
        );
    }

    #[test]
    fn pen_pressure_rescale() {
        let min_down = (0.06 * f64::from(PRESSURE_MAX)).floor() as i32;
        assert!(contact_pressure(1) > min_down);
        assert_eq!(contact_pressure(65535), 4095);
        assert!(contact_pressure(0) > min_down);
        assert!(contact_pressure(30000) < contact_pressure(40000));
    }

    #[test]
    fn pen_tilt_clamped_and_changes_only() {
        let mut e = PenEncoder::new(PenButton::Stylus2);
        let mut s = ps(1, 1, 0, HOVER);
        s.tilt_x = 12000;
        s.tilt_y = -100;
        let f = e.pen(&s);
        assert!(has(&f[0], (A, code(Abs::ABS_TILT_X), 9000)));
        assert!(has(&f[0], (A, code(Abs::ABS_TILT_Y), -100)));
        s.tilt_y = 300;
        let f = e.pen(&s);
        assert_eq!(
            f,
            vec![vec![
                (A, code(Abs::ABS_TILT_X), 9000),
                (A, code(Abs::ABS_TILT_Y), 300),
            ]]
        );
    }

    #[test]
    fn pen_eraser_switch_is_prox_out_then_in() {
        let mut e = PenEncoder::new(PenButton::Stylus2);
        e.pen(&ps(5, 5, 0, HOVER));
        let f = e.pen(&ps(5, 5, 0, HOVER | PenSample::ERASER));
        assert_eq!(f.len(), 2);
        assert_eq!(f[0], vec![(K, Key::BTN_TOOL_PEN.0, 0)]);
        assert!(has(&f[1], (K, Key::BTN_TOOL_RUBBER.0, 1)));
        // Back to the pen while touching: contact released before leaving.
        let f = e.pen(&ps(5, 5, 20000, TOUCH | PenSample::ERASER));
        assert_eq!(f.len(), 1);
        let f = e.pen(&ps(5, 5, 20000, TOUCH));
        assert_eq!(
            f[0],
            vec![(K, Key::BTN_TOUCH.0, 0), (A, code(Abs::ABS_PRESSURE), 0)]
        );
        assert_eq!(f[1], vec![(K, Key::BTN_TOOL_RUBBER.0, 0)]);
        assert!(has(&f[2], (K, Key::BTN_TOOL_PEN.0, 1)));
        assert!(has(&f[3], (K, Key::BTN_TOUCH.0, 1)));
    }

    #[test]
    fn pen_barrel_code_and_out_releases_everything() {
        for (btn, want) in [
            (PenButton::Stylus2, Key::BTN_STYLUS2.0),
            (PenButton::Stylus, Key::BTN_STYLUS.0),
        ] {
            let mut e = PenEncoder::new(btn);
            e.pen(&ps(5, 5, 0, HOVER));
            let f = e.pen(&ps(5, 5, 0, HOVER | PenSample::BARREL));
            assert_eq!(f, vec![vec![(K, want, 1)]]);
            e.pen(&ps(5, 5, 100, TOUCH | PenSample::BARREL));
            let f = e.pen(&ps(5, 5, 0, 0));
            assert_eq!(f.len(), 2);
            assert_eq!(
                f[0],
                vec![
                    (K, Key::BTN_TOUCH.0, 0),
                    (A, code(Abs::ABS_PRESSURE), 0),
                    (K, want, 0)
                ]
            );
            assert_eq!(f[1], vec![(K, Key::BTN_TOOL_PEN.0, 0)]);
            // Already out: nothing.
            assert!(e.pen(&ps(5, 5, 0, 0)).is_empty());
            assert!(e.release_all().is_empty());
        }
        assert_eq!(PenButton::from_env_value(None), PenButton::Stylus2);
        assert_eq!(PenButton::from_env_value(Some("stylus")), PenButton::Stylus);
        assert_eq!(PenButton::from_env_value(Some("bogus")), PenButton::Stylus2);
    }

    #[test]
    fn pen_hover_out_is_one_frame() {
        let mut e = PenEncoder::new(PenButton::Stylus2);
        e.pen(&ps(5, 5, 0, HOVER));
        assert_eq!(
            e.pen(&ps(5, 5, 0, 0)),
            vec![vec![(K, Key::BTN_TOOL_PEN.0, 0)]]
        );
    }

    fn kout(name: &str, pos: (i32, i32), size: (u32, u32), scale: f64) -> KOutput {
        KOutput {
            name: name.into(),
            id: 1,
            enabled: true,
            connected: true,
            size,
            pos,
            scale,
            current_mode_id: String::new(),
            modes: vec![],
        }
    }

    #[test]
    fn mapper_scaled_internal_plus_virtual_to_the_right() {
        // eDP-1 1920x1080 at 1.1 -> 1745.45 x 981.8 logical; the virtual output starts at 1746.
        let outs = [
            kout("eDP-1", (0, 0), (1920, 1080), 1.1),
            kout("Virtual-x", (1746, 0), (2560, 1600), 1.0),
        ];
        let m = Mapper::from_outputs(&outs, "Virtual-x");
        assert!(m.is_known() && !m.origin_offset);
        let ws_w: f64 = 1746.0 + 2560.0;
        assert_eq!(m.map(0, 0), ((1746.0 / ws_w * 65535.0).round() as u16, 0));
        assert_eq!(m.map(65535, 65535), (65535, 65535));
        let (x, y) = m.map(32768, 32768);
        let ex = ((1746.0 + 32768.0 / 65535.0 * 2560.0) / ws_w * 65535.0).round() as u16;
        assert_eq!((x, y), (ex, 32768));
        // The internal screen's scale matters for another layout: virtual output on the left.
        let outs = [
            kout("Virtual-x", (0, 0), (1000, 500), 1.0),
            kout("eDP-1", (1000, 0), (2200, 1100), 1.1),
        ];
        let m = Mapper::from_outputs(&outs, "Virtual-x");
        assert_eq!(
            m.map(65535, 0).0,
            (1000.0f64 / 3000.0 * 65535.0).round() as u16
        );
        assert_eq!(m.map(0, 65535).1, 32768); // 500 of 1000 logical rows
    }

    #[test]
    fn mapper_negative_origin_warns_and_subtracts() {
        let outs = [
            kout("Virtual-x", (-1000, 0), (1000, 1000), 1.0),
            kout("eDP-1", (0, 0), (1000, 1000), 1.0),
        ];
        let m = Mapper::from_outputs(&outs, "Virtual-x");
        assert!(m.origin_offset);
        assert_eq!(m.map(0, 0), (0, 0));
        assert_eq!(m.map(65535, 0).0, 32768);
    }

    #[test]
    fn mapper_unknown_output_is_identity() {
        let m = Mapper::from_outputs(&[kout("eDP-1", (0, 0), (10, 10), 1.0)], "nope");
        assert!(!m.is_known());
        assert_eq!(m.map(123, 456), (123, 456));
    }

    fn btn(x: u16, y: u16, button: u8, pressed: bool) -> PointerButton {
        PointerButton {
            t_us: 0,
            x,
            y,
            button,
            pressed,
        }
    }
    fn sc(dx: i32, dy: i32, modifiers: u8) -> Scroll {
        Scroll {
            t_us: 0,
            x: 1,
            y: 2,
            dx,
            dy,
            modifiers,
        }
    }

    #[test]
    fn pointer_button_frames() {
        let mut e = PointerEncoder::new();
        let m = Mapper::identity();
        let f = e.button(&btn(10, 20, PointerButton::RIGHT, true), &m);
        assert_eq!(
            f,
            vec![
                vec![(A, code(Abs::ABS_X), 10), (A, code(Abs::ABS_Y), 20)],
                vec![(K, Key::BTN_RIGHT.0, 1)],
            ]
        );
        // Unknown buttons only move.
        assert_eq!(e.button(&btn(10, 20, 9, true), &m).len(), 1);
        assert_eq!(e.release_all(), vec![vec![(K, Key::BTN_RIGHT.0, 0)]]);
        assert!(e.release_all().is_empty());
    }

    #[test]
    fn scroll_hires_notches_and_flip() {
        let mut e = PointerEncoder::new();
        let m = Mapper::identity();
        let wheel = |f: &Vec<Frame>| f.last().unwrap().clone();
        // 3 x 40 = one notch on the third event; hi-res always passes through.
        let f = e.scroll(&sc(0, 40, 0), &m);
        assert_eq!(wheel(&f), vec![(R, Rel::REL_WHEEL_HI_RES.0, 40)]);
        let f = e.scroll(&sc(0, 40, 0), &m);
        assert_eq!(wheel(&f), vec![(R, Rel::REL_WHEEL_HI_RES.0, 40)]);
        let f = e.scroll(&sc(0, 40, 0), &m);
        assert_eq!(
            wheel(&f),
            vec![(R, Rel::REL_WHEEL_HI_RES.0, 40), (R, Rel::REL_WHEEL.0, 1)]
        );
        // Remainder, then a direction flip resets it.
        e.scroll(&sc(0, 100, 0), &m);
        let f = e.scroll(&sc(0, -100, 0), &m);
        assert_eq!(wheel(&f), vec![(R, Rel::REL_WHEEL_HI_RES.0, -100)]);
        let f = e.scroll(&sc(0, -10, 0), &m);
        assert_eq!(wheel(&f), vec![(R, Rel::REL_WHEEL_HI_RES.0, -10)]);
        // Horizontal: dx > 0 = right, both axes in one frame; 240 = two notches.
        let f = e.scroll(&sc(240, 120, 0), &m);
        assert_eq!(
            wheel(&f),
            vec![
                (R, Rel::REL_WHEEL_HI_RES.0, 120),
                (R, Rel::REL_HWHEEL_HI_RES.0, 240),
                (R, Rel::REL_WHEEL.0, 1),
                (R, Rel::REL_HWHEEL.0, 2),
            ]
        );
    }

    #[test]
    fn scroll_modifiers_wrap_the_wheel() {
        let mut e = PointerEncoder::new();
        let f = e.scroll(
            &sc(0, 120, Scroll::CTRL | Scroll::SHIFT),
            &Mapper::identity(),
        );
        assert_eq!(
            f,
            vec![
                vec![(A, code(Abs::ABS_X), 1), (A, code(Abs::ABS_Y), 2)],
                vec![(K, Key::KEY_LEFTCTRL.0, 1), (K, Key::KEY_LEFTSHIFT.0, 1)],
                vec![(R, Rel::REL_WHEEL_HI_RES.0, 120), (R, Rel::REL_WHEEL.0, 1)],
                vec![(K, Key::KEY_LEFTCTRL.0, 0), (K, Key::KEY_LEFTSHIFT.0, 0)],
            ]
        );
        assert!(e.release_all().is_empty());
    }

    #[test]
    fn pen_as_mouse() {
        let mut e = PointerEncoder::new();
        let m = Mapper::identity();
        let f = e.pen_as_mouse(&ps(7, 8, 0, HOVER), &m);
        assert_eq!(
            f,
            vec![vec![(A, code(Abs::ABS_X), 7), (A, code(Abs::ABS_Y), 8)]]
        );
        let f = e.pen_as_mouse(&ps(9, 8, 500, TOUCH | PenSample::BARREL), &m);
        assert_eq!(f.len(), 2);
        assert_eq!(
            f[1],
            vec![(K, Key::BTN_LEFT.0, 1), (K, Key::BTN_RIGHT.0, 1)]
        );
        // Moving while pressed changes nothing but the position.
        let f = e.pen_as_mouse(&ps(10, 8, 500, TOUCH | PenSample::BARREL), &m);
        assert_eq!(f.len(), 1);
        // Out of range: release both, no move.
        let f = e.pen_as_mouse(&ps(10, 8, 0, 0), &m);
        assert_eq!(
            f,
            vec![vec![(K, Key::BTN_LEFT.0, 0), (K, Key::BTN_RIGHT.0, 0)]]
        );
    }

    #[test]
    fn physical_size() {
        let (w, h) = physical_mm(2560, 1600, 320);
        assert!((w - 203.2).abs() < 0.01 && (h - 127.0).abs() < 0.01);
        assert_eq!(physical_mm(0, 1600, 320), FALLBACK_MM);
        assert_eq!(physical_mm(2560, 1600, 0), FALLBACK_MM);
        assert_eq!(resolution(203.2), 323);
    }
}
