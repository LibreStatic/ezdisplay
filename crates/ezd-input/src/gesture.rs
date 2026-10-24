//! S Pen barrel-button gesture state machine and the actions it triggers.

use crate::config::{ButtonAction, KeyCombo};
use crate::engine::InputEngine;
use ezd_proto::{Key, Message, PenSample, PointerButton, Scroll};

/// Which trigger an action belongs to (selects its key combination).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    Click,
    DoubleClick,
    Hold,
}

/// Modifier bits of a [`KeyCombo`] with their HID usages, in press order.
const MODIFIER_KEYS: [(u8, u16); 4] = [
    (KeyCombo::CTRL, Key::LEFT_CTRL),
    (KeyCombo::SHIFT, Key::LEFT_SHIFT),
    (KeyCombo::ALT, Key::LEFT_ALT),
    (KeyCombo::META, Key::LEFT_GUI),
];

/// Axis conversion of a drag action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragKind {
    /// Natural scrolling (content follows the pen).
    Scroll,
    /// Ctrl + wheel, pen up = zoom in.
    PcZoom,
}

/// A resolved gesture whose barrel button is still down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Held {
    /// BARREL is forwarded until the release.
    Raw,
    /// Resolved without an ongoing effect (one-shot action, disabled trigger,
    /// eraser hold, second press of a double click).
    Done,
    /// Scrolling or zooming the host by moving the pen in hover.
    HoverDrag {
        kind: DragKind,
        anchor: (f32, f32),
        rem: (f32, f32),
    },
    /// Panning the locally zoomed view.
    Pan,
    /// The hold key combination is pressed (`PenState::keys_held`); the pen
    /// reports contact and motion as usual.
    Keys,
    /// Barrel held and the tip touched: contact is reported as hover.
    ContactDrag {
        kind: DragKind,
        anchor: (f32, f32),
        rem: (f32, f32),
        touching: bool,
        touch: (f32, f32),
        dragging: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) enum Gesture {
    /// Barrel up, nothing pending.
    #[default]
    Idle,
    /// First press in hover, withheld until it resolves.
    Pending {
        press: (f32, f32),
        press_t: u64,
        /// The hold time has elapsed (and `hold` did not fire).
        past_t: bool,
    },
    /// Click released, waiting for a possible second press.
    Waiting {
        deadline: u64,
        /// Pen position (view px) at the release; the click happens there.
        pos: (f32, f32),
    },
    /// Resolved, barrel still down.
    Held(Held),
}

/// `(dx, dy, modifiers)` of a scroll message.
type Units = (i32, i32, u8);

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Accumulates a pen delta into integer wheel units, keeping the remainder.
/// Returns `(dx, dy, modifiers)` when a nonzero integer delta is available.
fn drag_units(kind: DragKind, notch_px: f32, d: (f32, f32), rem: &mut (f32, f32)) -> Option<Units> {
    let k = 120.0 / notch_px;
    let mods = match kind {
        DragKind::Scroll => {
            rem.0 += -d.0 * k;
            rem.1 += d.1 * k;
            0
        }
        DragKind::PcZoom => {
            rem.1 += -d.1 * k;
            Scroll::CTRL
        }
    };
    let (ix, iy) = (rem.0.trunc(), rem.1.trunc());
    rem.0 -= ix;
    rem.1 -= iy;
    if ix == 0.0 && iy == 0.0 {
        None
    } else {
        Some((ix as i32, iy as i32, mods))
    }
}

impl InputEngine {
    pub(crate) fn gesture_deadline(&self) -> Option<u64> {
        match self.pen.gesture {
            Gesture::Pending {
                press_t,
                past_t: false,
                ..
            } => Some(press_t.saturating_add(self.p.double_us)),
            Gesture::Waiting { deadline, .. } => Some(deadline),
            _ => None,
        }
    }

    fn cur_pos(&self) -> (f32, f32) {
        (self.pen.cur.x, self.pen.cur.y)
    }

    fn set_held(&mut self, h: Held) {
        self.pen.gesture = Gesture::Held(h);
    }

    // ---- actions ---------------------------------------------------------

    /// Raw action while the barrel is still down: send one BARREL sample now,
    /// keep BARREL on all samples until the release.
    fn raw_held(&mut self, t: u64, out: &mut Vec<Message>) {
        self.pen.barrel_fwd = true;
        let f = self.base_flags() | PenSample::BARREL;
        self.emit_flush(t, f, out);
        self.set_held(Held::Raw);
    }

    /// Raw click resolved after the release: a BARREL sample, then one without.
    fn raw_late(&mut self, t: u64, out: &mut Vec<Message>) {
        let f = self.base_flags();
        self.emit_flush(t, f | PenSample::BARREL, out);
        self.emit_flush(t, f, out);
    }

    fn click_button(&mut self, button: u8, t: u64, out: &mut Vec<Message>) {
        let (x, y) = self.map(self.pen.cur.x, self.pen.cur.y);
        for pressed in [true, false] {
            out.push(Message::PointerButton(PointerButton {
                t_us: t,
                x,
                y,
                button,
                pressed,
            }));
        }
    }

    fn toggle_zoom(&mut self) {
        if self.view.is_zoomed() {
            self.view = crate::view::ViewTransform::IDENTITY;
        } else {
            let (px, py) = self.cur_pos();
            let w = self.p.zoom;
            self.view = crate::view::ViewTransform {
                scale: w,
                tx: px * (1.0 - w),
                ty: py * (1.0 - w),
            }
            .clamped(self.view_w, self.view_h);
        }
    }

    /// Emits the key presses of `c`: modifiers first, then the key.
    fn press_combo(c: KeyCombo, t: u64, out: &mut Vec<Message>) {
        let keys = MODIFIER_KEYS
            .iter()
            .filter(|(bit, _)| c.mods & bit != 0)
            .map(|(_, usage)| *usage)
            .chain((c.key != 0).then_some(c.key));
        for usage in keys {
            out.push(Message::Key(Key {
                t_us: t,
                usage,
                pressed: true,
            }));
        }
    }

    /// Emits the key releases of `c`, the reverse of [`Self::press_combo`].
    fn release_combo(c: KeyCombo, t: u64, out: &mut Vec<Message>) {
        let start = out.len();
        Self::press_combo(c, t, out);
        out[start..].reverse();
        for m in &mut out[start..] {
            if let Message::Key(k) = m {
                k.pressed = false;
            }
        }
    }

    /// Presses the hold combination and keeps it pressed until the barrel is
    /// released (modifier mode).
    fn start_keys_hold(&mut self, t: u64, out: &mut Vec<Message>) {
        let c = self.p.keys.hold;
        Self::press_combo(c, t, out);
        self.pen.keys_held = Some(c);
        self.set_held(Held::Keys);
    }

    /// Releases the held combination, if any (never twice).
    pub(crate) fn release_keys(&mut self, t: u64, out: &mut Vec<Message>) {
        if let Some(c) = self.pen.keys_held.take() {
            Self::release_combo(c, t, out);
        }
    }

    /// One-shot actions at the current pen position.
    fn instant(&mut self, a: ButtonAction, trigger: Trigger, t: u64, out: &mut Vec<Message>) {
        match a {
            ButtonAction::Keys => {
                let k = &self.p.keys;
                let c = match trigger {
                    Trigger::Click => k.click,
                    Trigger::DoubleClick => k.double_click,
                    Trigger::Hold => k.hold,
                };
                Self::press_combo(c, t, out);
                Self::release_combo(c, t, out);
            }
            ButtonAction::RightClick => self.click_button(PointerButton::RIGHT, t, out),
            ButtonAction::MiddleClick => self.click_button(PointerButton::MIDDLE, t, out),
            ButtonAction::LocalZoom => self.toggle_zoom(),
            _ => {}
        }
    }

    /// Eraser hold: Out, then proximity with the eraser flag.
    fn eraser_start(&mut self, t: u64, out: &mut Vec<Message>) {
        self.emit_flush(t, 0, out);
        self.pen.eraser_hold = true;
        let f = self.base_flags();
        self.emit_flush(t, f, out);
    }

    /// Back to the normal tool: Out, then plain proximity.
    fn eraser_end(&mut self, t: u64, out: &mut Vec<Message>) {
        self.pen.eraser_hold = false;
        self.pen.eraser_switch_pending = false;
        self.emit_flush(t, 0, out);
        let f = self.base_flags();
        self.emit_flush(t, f, out);
    }

    /// Resolves a click (barrel already released).
    fn resolve_click(&mut self, t: u64, out: &mut Vec<Message>) {
        match self.p.map.click {
            ButtonAction::Raw => self.raw_late(t, out),
            a => self.instant(a, Trigger::Click, t, out),
        }
    }

    /// Resolves a click that happened at `pos` (the pen may have moved since).
    fn resolve_click_at(&mut self, pos: (f32, f32), t: u64, out: &mut Vec<Message>) {
        let saved = (self.pen.cur.x, self.pen.cur.y);
        (self.pen.cur.x, self.pen.cur.y) = pos;
        self.resolve_click(t, out);
        (self.pen.cur.x, self.pen.cur.y) = saved;
    }

    /// Resolves the hold trigger (barrel down).
    fn resolve_hold(&mut self, t: u64, out: &mut Vec<Message>) {
        match self.p.map.hold {
            ButtonAction::Raw => self.raw_held(t, out),
            ButtonAction::Eraser => {
                self.eraser_start(t, out);
                self.set_held(Held::Done);
            }
            ButtonAction::Keys => self.start_keys_hold(t, out),
            a => {
                self.instant(a, Trigger::Hold, t, out);
                self.set_held(Held::Done);
            }
        }
    }

    // ---- barrel edges ----------------------------------------------------

    /// Barrel button edge (`down` = pressed).
    pub(crate) fn on_barrel(&mut self, down: bool, t: u64, out: &mut Vec<Message>) {
        let g = self.pen.gesture;
        if down {
            match g {
                Gesture::Idle if self.pen.contact => {
                    // Pressed with the tip already down: forward raw.
                    self.pen.barrel_fwd = true;
                    self.set_held(Held::Raw);
                }
                Gesture::Idle => {
                    self.pen.gesture = Gesture::Pending {
                        press: self.cur_pos(),
                        press_t: t,
                        past_t: false,
                    };
                    if self.pending_all_raw(false) {
                        self.raw_held(t, out);
                    }
                }
                Gesture::Waiting { .. } => match self.p.map.double_click {
                    ButtonAction::Raw => self.raw_held(t, out),
                    a => {
                        self.instant(a, Trigger::DoubleClick, t, out);
                        self.set_held(Held::Done);
                    }
                },
                _ => {}
            }
        } else {
            // The held keys are released whatever the gesture state is.
            self.release_keys(t, out);
            self.pen.gesture = Gesture::Idle;
            match g {
                Gesture::Pending { .. } => {
                    let click = self.p.map.click;
                    if click == ButtonAction::None {
                        return;
                    }
                    let dc = self.p.map.double_click;
                    let same = dc == click
                        && (click != ButtonAction::Keys
                            || self.p.keys.click == self.p.keys.double_click);
                    if dc == ButtonAction::None || same {
                        self.resolve_click(t, out);
                    } else {
                        self.pen.gesture = Gesture::Waiting {
                            deadline: t.saturating_add(self.p.double_us),
                            pos: self.cur_pos(),
                        };
                    }
                }
                Gesture::Held(h) => {
                    self.pen.barrel_fwd = false;
                    if h == Held::Done && self.pen.eraser_hold {
                        if self.pen.contact {
                            self.pen.eraser_switch_pending = true;
                        } else {
                            self.eraser_end(t, out);
                        }
                    }
                }
                Gesture::Idle | Gesture::Waiting { .. } => {
                    self.pen.gesture = g;
                }
            }
        }
    }

    /// True if every trigger still possible for the pending press maps to Raw
    /// (and the local-zoom pan is not possible), so it can be forwarded now.
    fn pending_all_raw(&self, past_t: bool) -> bool {
        if self.view.is_zoomed() {
            return false;
        }
        let m = &self.p.map;
        let set: &[ButtonAction] = if past_t {
            &[m.click, m.hold_drag_contact, m.hold_drag_hover]
        } else {
            &[
                m.click,
                m.double_click,
                m.hold,
                m.hold_drag_contact,
                m.hold_drag_hover,
            ]
        };
        let mut it = set.iter().filter(|a| **a != ButtonAction::None).peekable();
        it.peek().is_some() && it.all(|a| *a == ButtonAction::Raw)
    }

    // ---- timers ----------------------------------------------------------

    /// A gesture timer fired at its deadline `d`.
    pub(crate) fn fire_gesture(&mut self, d: u64, out: &mut Vec<Message>) {
        match self.pen.gesture {
            Gesture::Pending { press, press_t, .. } => {
                self.pen.gesture = Gesture::Pending {
                    press,
                    press_t,
                    past_t: true,
                };
                if self.p.map.hold != ButtonAction::None {
                    self.resolve_hold(d, out);
                } else if self.pending_all_raw(true) {
                    self.raw_held(d, out);
                }
            }
            Gesture::Waiting { pos, .. } => {
                self.pen.gesture = Gesture::Idle;
                self.resolve_click_at(pos, d, out);
            }
            _ => {}
        }
    }

    /// The pen is about to go out: a waiting click resolves first.
    pub(crate) fn resolve_gesture_on_out(&mut self, t: u64, out: &mut Vec<Message>) {
        if let Gesture::Waiting { pos, .. } = self.pen.gesture {
            self.pen.gesture = Gesture::Idle;
            self.resolve_click_at(pos, t, out);
        }
    }

    // ---- tip contact -----------------------------------------------------

    /// The tip is about to touch (called before `contact` is set).
    pub(crate) fn on_tip_down(&mut self, t: u64, out: &mut Vec<Message>) {
        match self.pen.gesture {
            Gesture::Pending { .. } if self.p.map.hold == ButtonAction::Keys => {
                self.start_keys_hold(t, out);
            }
            Gesture::Pending { press, .. } => match self.p.map.hold_drag_contact {
                ButtonAction::Raw => self.raw_held(t, out),
                a @ (ButtonAction::Scroll | ButtonAction::PcZoom) => {
                    self.pen.suppress = true;
                    let kind = if a == ButtonAction::Scroll {
                        DragKind::Scroll
                    } else {
                        DragKind::PcZoom
                    };
                    self.set_held(Held::ContactDrag {
                        kind,
                        anchor: press,
                        rem: (0.0, 0.0),
                        touching: true,
                        touch: self.cur_pos(),
                        dragging: false,
                    });
                }
                _ => self.set_held(Held::Done),
            },
            Gesture::Waiting { pos, .. } => {
                self.pen.gesture = Gesture::Idle;
                self.resolve_click_at(pos, t, out);
            }
            Gesture::Held(Held::ContactDrag {
                kind, anchor, rem, ..
            }) => {
                self.pen.suppress = true;
                self.set_held(Held::ContactDrag {
                    kind,
                    anchor,
                    rem,
                    touching: true,
                    touch: self.cur_pos(),
                    dragging: false,
                });
            }
            _ => {}
        }
    }

    /// The tip lifted (called after the lift sample was emitted).
    pub(crate) fn on_tip_up(&mut self, t: u64, out: &mut Vec<Message>) {
        self.pen.suppress = false;
        if let Gesture::Held(Held::ContactDrag {
            kind,
            anchor,
            rem,
            touching,
            dragging,
            ..
        }) = self.pen.gesture
        {
            self.set_held(Held::ContactDrag {
                kind,
                anchor,
                rem,
                touching: false,
                touch: (0.0, 0.0),
                dragging: false,
            });
            if touching && !dragging {
                let click = self.p.map.click;
                self.instant(click, Trigger::Click, t, out);
            }
        }
        if self.pen.eraser_switch_pending {
            self.eraser_end(t, out);
        }
    }

    // ---- motion ----------------------------------------------------------

    /// Movement-driven transitions and drag actions, once per pen sample.
    pub(crate) fn gesture_sample(&mut self, t: u64, out: &mut Vec<Message>) {
        let cur = self.cur_pos();
        if let Gesture::Pending { press, .. } = self.pen.gesture
            && !self.pen.contact
            && dist(cur, press) > self.p.slop
        {
            if self.view.is_zoomed() {
                self.set_held(Held::Pan);
            } else if self.p.map.hold == ButtonAction::Keys {
                self.start_keys_hold(t, out);
            } else {
                match self.p.map.hold_drag_hover {
                    ButtonAction::Raw => self.raw_held(t, out),
                    a @ (ButtonAction::Scroll | ButtonAction::PcZoom) => {
                        let kind = if a == ButtonAction::Scroll {
                            DragKind::Scroll
                        } else {
                            DragKind::PcZoom
                        };
                        self.set_held(Held::HoverDrag {
                            kind,
                            anchor: press,
                            rem: (0.0, 0.0),
                        });
                    }
                    _ => self.set_held(Held::Done),
                }
            }
        }
        let prev = self.pen.prev;
        let d = (cur.0 - prev.0, cur.1 - prev.1);
        let n = self.p.scroll_n;
        let slop = self.p.slop;
        let mut emit: Option<((f32, f32), Units)> = None;
        match &mut self.pen.gesture {
            Gesture::Held(Held::HoverDrag { kind, anchor, rem }) => {
                emit = drag_units(*kind, n, d, rem).map(|u| (*anchor, u));
            }
            Gesture::Held(Held::ContactDrag {
                kind,
                anchor,
                rem,
                touching: true,
                touch,
                dragging,
            }) => {
                if !*dragging && dist(cur, *touch) > slop {
                    *dragging = true;
                }
                if *dragging {
                    emit = drag_units(*kind, n, d, rem).map(|u| (*anchor, u));
                }
            }
            Gesture::Held(Held::Pan) => {
                self.view = crate::view::ViewTransform {
                    tx: self.view.tx + d.0,
                    ty: self.view.ty + d.1,
                    ..self.view
                }
                .clamped(self.view_w, self.view_h);
            }
            _ => {}
        }
        if let Some((anchor, (dx, dy, modifiers))) = emit {
            let (x, y) = self.map(anchor.0, anchor.1);
            out.push(Message::Scroll(Scroll {
                t_us: t,
                x,
                y,
                dx,
                dy,
                modifiers,
            }));
        }
    }
}
