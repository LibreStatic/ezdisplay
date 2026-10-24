//! Touch handling: contact id allocation and palm rejection of fingers.

use crate::android as a;
use crate::engine::InputEngine;
use crate::motion::Motion;
use ezd_proto::{Message, TouchContact, TouchFrame};

/// Maximum simultaneous contacts (ids 0..=9).
const MAX_CONTACTS: usize = 10;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Finger {
    aid: i32,
    cid: u8,
    x: f32,
    y: f32,
}

/// Fingers being forwarded and fingers being ignored until they lift.
#[derive(Debug, Default)]
pub(crate) struct TouchState {
    active: Vec<Finger>,
    ignored: Vec<i32>,
}

fn is_touch_tool(tool: i32) -> bool {
    matches!(
        tool,
        a::TOOL_TYPE_FINGER | a::TOOL_TYPE_UNKNOWN | a::TOOL_TYPE_PALM
    )
}

impl InputEngine {
    /// True while fingers must be ignored because of the pen.
    fn palm_blocked(&self, now: u64) -> bool {
        self.p.palm
            && (self.pen.in_range
                || self
                    .pen
                    .left_at
                    .is_some_and(|l| now.saturating_sub(l) < self.p.grace_us))
    }

    fn push_frame(&self, t_us: u64, mut contacts: Vec<TouchContact>, out: &mut Vec<Message>) {
        if contacts.is_empty() {
            return;
        }
        contacts.sort_by_key(|c| c.id);
        out.push(Message::Touch(TouchFrame { t_us, contacts }));
    }

    /// Processes the fingers of one step of a motion.
    pub(crate) fn touch_step(&mut self, m: &Motion, idx: usize, out: &mut Vec<Message>) {
        let step = &m.steps[idx];
        let is_last = idx + 1 == m.steps.len();
        let is_down = matches!(m.action, a::ACTION_DOWN | a::ACTION_POINTER_DOWN);
        let is_up = matches!(m.action, a::ACTION_UP | a::ACTION_POINTER_UP);
        let action_id = if is_last && (is_down || is_up) {
            step.pointers.get(m.action_index).map(|p| p.id)
        } else {
            None
        };
        // Motions without any finger (e.g. stylus only) do not produce frames.
        if !step.pointers.iter().any(|p| is_touch_tool(p.tool_type)) {
            return;
        }
        let cancel_all = is_last && m.action == a::ACTION_CANCEL;
        let blocked = self.palm_blocked(step.t_us);
        let mp = self.mapper();
        let contact = |cid: u8, x: f32, y: f32, phase: u8| {
            let (x, y) = mp.map(x, y);
            TouchContact {
                id: cid,
                x,
                y,
                phase,
            }
        };
        let ts = &mut self.touch;
        let mut contacts: Vec<TouchContact> = Vec::new();
        let mut finished: Vec<i32> = Vec::new();
        let mut seen: Vec<i32> = Vec::new();

        for p in step.pointers.iter().filter(|p| is_touch_tool(p.tool_type)) {
            seen.push(p.id);
            let is_action = action_id == Some(p.id);
            if p.tool_type == a::TOOL_TYPE_PALM {
                if let Some(pos) = ts.active.iter().position(|f| f.aid == p.id) {
                    let f = ts.active.remove(pos);
                    contacts.push(contact(f.cid, p.x, p.y, TouchContact::CANCEL));
                }
                if !ts.ignored.contains(&p.id) {
                    ts.ignored.push(p.id);
                }
                continue;
            }
            if is_action && is_down {
                ts.ignored.retain(|i| *i != p.id);
                if let Some(f) = ts.active.iter_mut().find(|f| f.aid == p.id) {
                    f.x = p.x;
                    f.y = p.y;
                    contacts.push(contact(f.cid, p.x, p.y, TouchContact::MOVE));
                    continue;
                }
                let free = (0..MAX_CONTACTS as u8).find(|c| ts.active.iter().all(|f| f.cid != *c));
                match free {
                    Some(cid) if !blocked => {
                        ts.active.push(Finger {
                            aid: p.id,
                            cid,
                            x: p.x,
                            y: p.y,
                        });
                        contacts.push(contact(cid, p.x, p.y, TouchContact::DOWN));
                    }
                    _ => ts.ignored.push(p.id),
                }
                continue;
            }
            if ts.ignored.contains(&p.id) {
                if is_action && is_up {
                    ts.ignored.retain(|i| *i != p.id);
                }
                continue;
            }
            if let Some(f) = ts.active.iter_mut().find(|f| f.aid == p.id) {
                f.x = p.x;
                f.y = p.y;
                let phase = if is_action && is_up {
                    finished.push(p.id);
                    if m.flags & a::FLAG_CANCELED != 0 {
                        TouchContact::CANCEL
                    } else {
                        TouchContact::UP
                    }
                } else {
                    TouchContact::MOVE
                };
                contacts.push(contact(f.cid, p.x, p.y, phase));
            }
        }
        // Active fingers missing from this step keep their last position.
        for f in ts.active.iter().filter(|f| !seen.contains(&f.aid)) {
            contacts.push(contact(f.cid, f.x, f.y, TouchContact::MOVE));
        }
        if cancel_all {
            for c in &mut contacts {
                c.phase = TouchContact::CANCEL;
            }
            ts.active.clear();
            ts.ignored.clear();
        } else {
            ts.active.retain(|f| !finished.contains(&f.aid));
        }
        self.push_frame(step.t_us, contacts, out);
    }

    /// Pen entered range: cancel every active contact in one frame and ignore
    /// them until they lift.
    pub(crate) fn touch_cancel_all(&mut self, t: u64, out: &mut Vec<Message>) {
        let mp = self.mapper();
        let ts = &mut self.touch;
        let contacts: Vec<TouchContact> = ts
            .active
            .iter()
            .map(|f| {
                let (x, y) = mp.map(f.x, f.y);
                TouchContact {
                    id: f.cid,
                    x,
                    y,
                    phase: TouchContact::CANCEL,
                }
            })
            .collect();
        let aids: Vec<i32> = ts.active.drain(..).map(|f| f.aid).collect();
        ts.ignored.extend(aids);
        self.push_frame(t, contacts, out);
    }

    /// Cancels every contact and forgets all touch state.
    pub(crate) fn touch_release_all(&mut self, t: u64, out: &mut Vec<Message>) {
        self.touch_cancel_all(t, out);
        self.touch.ignored.clear();
    }
}
