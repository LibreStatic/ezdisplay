//! Pen handling: proximity, contact, sample emission and the delayed Out.

use crate::android as a;
use crate::config::KeyCombo;
use crate::engine::InputEngine;
use crate::gesture::Gesture;
use crate::motion::{Motion, MotionStep, PointerSample};
use crate::tilt::tilt_xy;
use ezd_proto::{Message, PenSample};

/// Last known pen position and attributes, in view pixels.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PenPoint {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: i16,
    pub tilt_y: i16,
}

#[derive(Debug, Default)]
pub(crate) struct PenState {
    /// Pen in proximity (until the Out sample has been emitted).
    pub in_range: bool,
    /// When the Out sample was emitted, for the palm grace period.
    pub left_at: Option<u64>,
    /// Armed hover-exit timer.
    pub exit_at: Option<u64>,
    /// Android pointer id of the tracked stylus.
    pub aid: Option<i32>,
    /// Tip is down.
    pub contact: bool,
    /// Tip is down but reported as hover (gesture in progress).
    pub suppress: bool,
    /// The tool type is the eraser end.
    pub tool_eraser: bool,
    /// The Eraser hold action is active.
    pub eraser_hold: bool,
    /// Eraser release happened in contact: switch back after the lift.
    pub eraser_switch_pending: bool,
    /// Previous raw barrel state, for edge detection.
    pub barrel_prev: bool,
    /// BARREL is forwarded on samples.
    pub barrel_fwd: bool,
    pub has_pos: bool,
    pub cur: PenPoint,
    /// Position before the current sample.
    pub prev: (f32, f32),
    /// Last sample emitted by a gesture action in this step, to avoid sending
    /// the same state twice.
    pub last_flush: Option<PenSample>,
    pub gesture: Gesture,
    /// Key combination currently pressed on the host. Only the release paths
    /// clear it (`drop_gesture` cannot emit, so it leaves it alone).
    pub keys_held: Option<KeyCombo>,
}

const BARREL_MASK: i32 =
    a::BUTTON_STYLUS_PRIMARY | a::BUTTON_STYLUS_SECONDARY | a::BUTTON_SECONDARY;

fn is_stylus_tool(tool: i32) -> bool {
    matches!(tool, a::TOOL_TYPE_STYLUS | a::TOOL_TYPE_ERASER)
}

impl InputEngine {
    /// The tracked stylus pointer of a step (the first one seen).
    pub(crate) fn stylus_pointer<'a>(&mut self, step: &'a MotionStep) -> Option<&'a PointerSample> {
        let mut first = None;
        for p in step.pointers.iter().filter(|p| is_stylus_tool(p.tool_type)) {
            if Some(p.id) == self.pen.aid {
                return Some(p);
            }
            first.get_or_insert(p);
        }
        if let Some(p) = first {
            self.pen.aid = Some(p.id);
        }
        first
    }

    pub(crate) fn make_sample(&self, t: u64, flags: u8) -> PenSample {
        let (x, y) = self.map(self.pen.cur.x, self.pen.cur.y);
        let pressure = if flags & PenSample::CONTACT != 0 {
            (self.pen.cur.pressure.clamp(0.0, 1.0) * 65535.0).round() as u16
        } else {
            0
        };
        let (tilt_x, tilt_y) = if flags == 0 {
            (0, 0)
        } else {
            (self.pen.cur.tilt_x, self.pen.cur.tilt_y)
        };
        PenSample {
            t_us: t,
            x,
            y,
            pressure,
            tilt_x,
            tilt_y,
            flags,
        }
    }

    /// Flags of a regular sample, without BARREL.
    pub(crate) fn base_flags(&self) -> u8 {
        let p = &self.pen;
        let mut f = PenSample::IN_RANGE;
        if p.contact && !p.suppress {
            f |= PenSample::CONTACT;
        }
        if p.tool_eraser || p.eraser_hold {
            f |= PenSample::ERASER;
        }
        f
    }

    /// Emits a sample on behalf of a gesture action.
    pub(crate) fn emit_flush(&mut self, t: u64, flags: u8, out: &mut Vec<Message>) {
        let s = self.make_sample(t, flags);
        self.pen.last_flush = Some(s);
        out.push(Message::Pen(s));
    }

    /// Proximity started: palm rejection cancels active fingers.
    fn pen_enter(&mut self, t: u64, out: &mut Vec<Message>) {
        self.pen.in_range = true;
        self.pen.left_at = None;
        if self.p.palm {
            self.touch_cancel_all(t, out);
        }
    }

    /// The pen left range: resolve what is pending and emit the Out sample.
    pub(crate) fn pen_out(&mut self, t: u64, out: &mut Vec<Message>) {
        self.release_keys(t, out);
        self.resolve_gesture_on_out(t, out);
        self.drop_gesture();
        self.pen.eraser_hold = false;
        self.pen.eraser_switch_pending = false;
        self.pen.contact = false;
        self.pen.suppress = false;
        self.pen.barrel_prev = false;
        self.pen.exit_at = None;
        self.pen.aid = None;
        if self.pen.in_range {
            let s = self.make_sample(t, 0);
            out.push(Message::Pen(s));
        }
        self.pen.in_range = false;
        self.pen.left_at = Some(t);
    }

    /// Processes the stylus pointer of one step of a motion.
    pub(crate) fn pen_step(
        &mut self,
        m: &Motion,
        step: &MotionStep,
        ptr: &PointerSample,
        is_last: bool,
        first: bool,
        out: &mut Vec<Message>,
    ) {
        let t = step.t_us;
        self.pen.last_flush = None;
        let action = if is_last { m.action } else { a::ACTION_MOVE };
        if action == a::ACTION_HOVER_EXIT {
            if self.pen.in_range && self.pen.exit_at.is_none() {
                self.pen.exit_at = Some(t.saturating_add(self.p.exit_us));
                self.fire_due(t, out);
            }
            return;
        }
        let is_action_ptr = is_last
            && step
                .pointers
                .get(m.action_index)
                .is_some_and(|q| q.id == ptr.id);

        if !self.pen.in_range {
            self.pen_enter(t, out);
        }
        self.pen.exit_at = None;

        let (tilt_x, tilt_y) = tilt_xy(ptr.tilt, ptr.orientation);
        self.pen.prev = if self.pen.has_pos {
            (self.pen.cur.x, self.pen.cur.y)
        } else {
            (ptr.x, ptr.y)
        };
        self.pen.has_pos = true;
        self.pen.cur = PenPoint {
            x: ptr.x,
            y: ptr.y,
            pressure: ptr.pressure,
            tilt_x,
            tilt_y,
        };
        self.pen.tool_eraser = ptr.tool_type == a::TOOL_TYPE_ERASER;

        if first {
            let barrel = m.button_state & BARREL_MASK != 0;
            if barrel != self.pen.barrel_prev {
                self.pen.barrel_prev = barrel;
                self.on_barrel(barrel, t, out);
            }
        }

        let down = is_action_ptr && matches!(action, a::ACTION_DOWN | a::ACTION_POINTER_DOWN);
        if down && !self.pen.contact {
            self.on_tip_down(t, out);
            self.pen.contact = true;
        }
        self.gesture_sample(t, out);

        let up = (is_action_ptr && matches!(action, a::ACTION_UP | a::ACTION_POINTER_UP))
            || action == a::ACTION_CANCEL;
        let lifted = up && self.pen.contact;
        if lifted {
            self.pen.contact = false;
        }
        let flags = self.base_flags()
            | if self.pen.barrel_fwd {
                PenSample::BARREL
            } else {
                0
            };
        let s = self.make_sample(t, flags);
        let dup = self
            .pen
            .last_flush
            .is_some_and(|f| PenSample { t_us: t, ..f } == s);
        if !dup {
            out.push(Message::Pen(s));
        }
        if lifted {
            self.on_tip_up(t, out);
        }
        if action == a::ACTION_CANCEL {
            self.pen.exit_at = Some(t.saturating_add(self.p.exit_us));
            self.fire_due(t, out);
        }
    }

    /// Cancels the gesture state without emitting anything.
    pub(crate) fn drop_gesture(&mut self) {
        self.pen.gesture = Gesture::Idle;
        self.pen.barrel_fwd = false;
        self.pen.eraser_hold = false;
        self.pen.eraser_switch_pending = false;
    }
}
