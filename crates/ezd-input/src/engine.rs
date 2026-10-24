//! The engine: public API, event routing and timers.

use crate::android as a;
use crate::config::{InputConfig, Params};
use crate::motion::{Motion, MotionStep};
use crate::pen::PenState;
use crate::touch::TouchState;
use crate::view::{Mapper, ViewTransform};
use ezd_proto::{Message, Scroll};

/// Client input engine. See the [crate docs](crate).
pub struct InputEngine {
    pub(crate) p: Params,
    pub(crate) view_w: f32,
    pub(crate) view_h: f32,
    pub(crate) view: ViewTransform,
    pub(crate) touch: TouchState,
    pub(crate) pen: PenState,
}

impl InputEngine {
    /// Creates an engine for a view of `view_w` x `view_h` pixels.
    pub fn new(cfg: InputConfig, view_w: f32, view_h: f32) -> Self {
        InputEngine {
            p: Params::from_config(&cfg),
            view_w,
            view_h,
            view: ViewTransform::IDENTITY,
            touch: TouchState::default(),
            pen: PenState::default(),
        }
    }

    /// Applies a new config. Keeps state but cancels a pending gesture.
    pub fn set_config(&mut self, cfg: InputConfig) {
        self.p = Params::from_config(&cfg);
        self.drop_gesture();
    }

    /// Sets the view size; resets the zoom to 1x if the size changed.
    pub fn set_view_size(&mut self, w: f32, h: f32) {
        if w != self.view_w || h != self.view_h {
            self.view = ViewTransform::IDENTITY;
        }
        self.view_w = w;
        self.view_h = h;
    }

    /// Current local zoom transform.
    pub fn view(&self) -> ViewTransform {
        self.view
    }

    /// Pen position in normalized stream coordinates while the pen is in range
    /// (for the local cursor).
    pub fn pen_position(&self) -> Option<(u16, u16)> {
        if self.pen.in_range {
            Some(self.mapper().map(self.pen.cur.x, self.pen.cur.y))
        } else {
            None
        }
    }

    /// Earliest pending timer, in client microseconds.
    pub fn next_deadline(&self) -> Option<u64> {
        [self.pen.exit_at, self.gesture_deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Fires every timer due at `now_us`.
    pub fn tick(&mut self, now_us: u64, out: &mut Vec<Message>) {
        self.fire_due(now_us, out);
    }

    /// Releases everything (activity paused / disconnect): pen out, all touches
    /// CANCEL, gesture dropped.
    pub fn release_all(&mut self, now_us: u64, out: &mut Vec<Message>) {
        self.drop_gesture();
        self.pen.exit_at = None;
        if self.pen.in_range {
            self.pen_out(now_us, out);
        }
        self.touch_release_all(now_us, out);
    }

    /// Processes one `MotionEvent`.
    pub fn motion(&mut self, m: &Motion, out: &mut Vec<Message>) {
        let Some(last) = m.steps.last() else {
            return;
        };
        if m.action == a::ACTION_OUTSIDE {
            return;
        }
        if m.action == a::ACTION_SCROLL {
            self.scroll_event(m, last, out);
            return;
        }
        let n = m.steps.len();
        let mut first_pen_step = true;
        for (i, step) in m.steps.iter().enumerate() {
            self.fire_due(step.t_us, out);
            if let Some(ptr) = self.stylus_pointer(step) {
                self.pen_step(m, step, ptr, i + 1 == n, first_pen_step, out);
                first_pen_step = false;
            }
            self.touch_step(m, i, out);
        }
    }

    pub(crate) fn mapper(&self) -> Mapper {
        Mapper {
            view: self.view,
            w: self.view_w,
            h: self.view_h,
        }
    }

    pub(crate) fn map(&self, x: f32, y: f32) -> (u16, u16) {
        self.mapper().map(x, y)
    }

    fn scroll_event(&mut self, m: &Motion, last: &MotionStep, out: &mut Vec<Message>) {
        let Some(ptr) = last.pointers.get(m.action_index).or(last.pointers.first()) else {
            return;
        };
        let dx = (m.hscroll * 120.0).round() as i32;
        let dy = (m.vscroll * 120.0).round() as i32;
        if dx == 0 && dy == 0 {
            return;
        }
        let (x, y) = self.map(ptr.x, ptr.y);
        out.push(Message::Scroll(Scroll {
            t_us: last.t_us,
            x,
            y,
            dx,
            dy,
            modifiers: 0,
        }));
    }

    /// Fires pen and gesture timers due at `now`, earliest first (the gesture
    /// first on a tie, so a pending click resolves before the pen goes out).
    pub(crate) fn fire_due(&mut self, now: u64, out: &mut Vec<Message>) {
        loop {
            let g = self.gesture_deadline().filter(|d| *d <= now);
            let e = self.pen.exit_at.filter(|d| *d <= now);
            match (g, e) {
                (Some(gd), Some(ed)) if gd <= ed => self.fire_gesture(gd, out),
                (Some(gd), None) => self.fire_gesture(gd, out),
                (_, Some(ed)) => {
                    self.pen.exit_at = None;
                    self.pen_out(ed, out);
                }
                (None, None) => break,
            }
        }
    }
}
