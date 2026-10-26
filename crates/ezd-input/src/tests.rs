//! Table-style tests. The test view is 65535 x 65535 px so that view pixels
//! equal normalized coordinates.

use crate::android::*;
use crate::config::{ButtonAction, ButtonMap, InputConfig, KeyBindings, KeyCombo, Profile};
use crate::engine::InputEngine;
use crate::motion::{Motion, MotionStep, PointerSample};
use crate::tilt::tilt_xy;
use crate::view::{Mapper, ViewTransform};
use ezd_proto::{Message, PenSample, TouchContact, normalize};

const SIDE: f32 = 65535.0;
const PRIMARY: i32 = BUTTON_STYLUS_PRIMARY;

fn ms(v: u64) -> u64 {
    v * 1000
}

fn ptr(id: i32, tool: i32, x: f32, y: f32, pressure: f32) -> PointerSample {
    PointerSample {
        id,
        tool_type: tool,
        x,
        y,
        pressure,
        tilt: 0.0,
        orientation: 0.0,
    }
}

fn motion(
    action: i32,
    index: usize,
    buttons: i32,
    steps: Vec<(u64, Vec<PointerSample>)>,
) -> Motion {
    Motion {
        action,
        action_index: index,
        button_state: buttons,
        flags: 0,
        vscroll: 0.0,
        hscroll: 0.0,
        steps: steps
            .into_iter()
            .map(|(t, pointers)| MotionStep {
                t_us: ms(t),
                pointers,
            })
            .collect(),
    }
}

/// One stylus event (id 7). Pressure 0.5 for DOWN/MOVE.
fn pen_at(action: i32, t: u64, x: f32, y: f32, buttons: i32) -> Motion {
    let p = if matches!(action, ACTION_DOWN | ACTION_MOVE) {
        0.5
    } else {
        0.0
    };
    motion(
        action,
        0,
        buttons,
        vec![(t, vec![ptr(7, TOOL_TYPE_STYLUS, x, y, p)])],
    )
}

fn hover(t: u64, x: f32, y: f32, b: i32) -> Motion {
    pen_at(ACTION_HOVER_MOVE, t, x, y, b)
}
fn pen_down(t: u64, x: f32, y: f32, b: i32) -> Motion {
    pen_at(ACTION_DOWN, t, x, y, b)
}
fn pen_move(t: u64, x: f32, y: f32, b: i32) -> Motion {
    pen_at(ACTION_MOVE, t, x, y, b)
}
fn pen_up(t: u64, x: f32, y: f32, b: i32) -> Motion {
    pen_at(ACTION_UP, t, x, y, b)
}
fn exit(t: u64, x: f32, y: f32) -> Motion {
    pen_at(ACTION_HOVER_EXIT, t, x, y, 0)
}
fn press(t: u64, x: f32, y: f32) -> Motion {
    pen_at(ACTION_BUTTON_PRESS, t, x, y, PRIMARY)
}
fn release(t: u64, x: f32, y: f32) -> Motion {
    pen_at(ACTION_BUTTON_RELEASE, t, x, y, 0)
}

/// One finger event with the given pointers `(id, x, y)`.
fn finger(action: i32, index: usize, t: u64, ps: &[(i32, f32, f32)]) -> Motion {
    let pointers = ps
        .iter()
        .map(|&(id, x, y)| ptr(id, TOOL_TYPE_FINGER, x, y, 1.0))
        .collect();
    motion(action, index, 0, vec![(t, pointers)])
}

fn show(m: &Message) -> String {
    match m {
        Message::Pen(p) => {
            let mut f = String::new();
            for (bit, c) in [
                (PenSample::IN_RANGE, 'I'),
                (PenSample::CONTACT, 'C'),
                (PenSample::BARREL, 'B'),
                (PenSample::ERASER, 'E'),
            ] {
                if p.flags & bit != 0 {
                    f.push(c);
                }
            }
            if f.is_empty() {
                f.push('-');
            }
            if p.pressure != 0 {
                format!("pen {f} {},{} p={}", p.x, p.y, p.pressure)
            } else {
                format!("pen {f} {},{}", p.x, p.y)
            }
        }
        Message::Touch(f) => {
            let cs: Vec<String> = f
                .contacts
                .iter()
                .map(|c| {
                    let ph = match c.phase {
                        TouchContact::DOWN => 'D',
                        TouchContact::MOVE => 'M',
                        TouchContact::UP => 'U',
                        _ => 'C',
                    };
                    format!("{}:{ph}({},{})", c.id, c.x, c.y)
                })
                .collect();
            format!("touch {}", cs.join(" "))
        }
        Message::PointerButton(b) => {
            let n = ["L", "R", "M"][usize::from(b.button)];
            format!(
                "btn {n}{} {},{}",
                if b.pressed { '+' } else { '-' },
                b.x,
                b.y
            )
        }
        Message::Scroll(s) => format!("scroll {},{} m{} @{},{}", s.dx, s.dy, s.modifiers, s.x, s.y),
        Message::Key(k) => format!(
            "key {:#04x} {}",
            k.usage,
            if k.pressed { "down" } else { "up" }
        ),
        other => panic!("unexpected {other:?}"),
    }
}

struct Rig {
    e: InputEngine,
    out: Vec<Message>,
}

impl Rig {
    fn new() -> Rig {
        Rig::with(InputConfig::default())
    }
    fn with(cfg: InputConfig) -> Rig {
        Rig {
            e: InputEngine::new(cfg, SIDE, SIDE),
            out: Vec::new(),
        }
    }
    fn profile(p: Profile) -> Rig {
        Rig::with(InputConfig {
            profile: p,
            ..InputConfig::default()
        })
    }
    fn raw(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.out)
    }
    fn feed(&mut self, m: Motion) -> Vec<String> {
        self.e.motion(&m, &mut self.out);
        self.raw().iter().map(show).collect()
    }
    fn tick(&mut self, t: u64) -> Vec<String> {
        self.e.tick(ms(t), &mut self.out);
        self.raw().iter().map(show).collect()
    }
}

fn pen_t(m: &Message) -> u64 {
    match m {
        Message::Pen(p) => p.t_us,
        _ => panic!("not a pen sample"),
    }
}

// ---- 1. touch -------------------------------------------------------------

#[test]
fn finger_tap() {
    let mut r = Rig::new();
    assert_eq!(
        r.feed(finger(ACTION_DOWN, 0, 0, &[(3, 100.0, 200.0)])),
        ["touch 0:D(100,200)"]
    );
    assert_eq!(
        r.feed(finger(ACTION_UP, 0, 50, &[(3, 100.0, 200.0)])),
        ["touch 0:U(100,200)"]
    );
    assert_eq!(
        r.feed(finger(ACTION_DOWN, 0, 80, &[(3, 5.0, 6.0)])),
        ["touch 0:D(5,6)"]
    );
}

#[test]
fn finger_drag_with_history_gives_one_frame_per_step() {
    let mut r = Rig::new();
    r.feed(finger(ACTION_DOWN, 0, 0, &[(0, 10.0, 10.0)]));
    let mk = |x| vec![ptr(0, TOOL_TYPE_FINGER, x, 10.0, 1.0)];
    let m = motion(
        ACTION_MOVE,
        0,
        0,
        vec![(10, mk(20.0)), (20, mk(30.0)), (30, mk(40.0))],
    );
    r.e.motion(&m, &mut r.out);
    let ts: Vec<u64> = r
        .out
        .iter()
        .map(|m| match m {
            Message::Touch(f) => f.t_us,
            _ => panic!(),
        })
        .collect();
    assert_eq!(ts, [ms(10), ms(20), ms(30)]);
    let shown: Vec<String> = r.raw().iter().map(show).collect();
    assert_eq!(
        shown,
        ["touch 0:M(20,10)", "touch 0:M(30,10)", "touch 0:M(40,10)"]
    );
}

#[test]
fn two_fingers_out_of_order_and_id_reuse() {
    let mut r = Rig::new();
    // Android pointer ids 5 and 9.
    assert_eq!(
        r.feed(finger(ACTION_DOWN, 0, 0, &[(5, 1.0, 1.0)])),
        ["touch 0:D(1,1)"]
    );
    assert_eq!(
        r.feed(finger(
            ACTION_POINTER_DOWN,
            1,
            10,
            &[(5, 1.0, 1.0), (9, 2.0, 2.0)]
        )),
        ["touch 0:M(1,1) 1:D(2,2)"]
    );
    // The first finger lifts first.
    assert_eq!(
        r.feed(finger(
            ACTION_POINTER_UP,
            0,
            20,
            &[(5, 1.0, 1.0), (9, 2.0, 2.0)]
        )),
        ["touch 0:U(1,1) 1:M(2,2)"]
    );
    // A new finger takes the lowest free id, 0.
    assert_eq!(
        r.feed(finger(
            ACTION_POINTER_DOWN,
            1,
            30,
            &[(9, 2.0, 2.0), (4, 3.0, 3.0)]
        )),
        ["touch 0:D(3,3) 1:M(2,2)"]
    );
    assert_eq!(
        r.feed(finger(
            ACTION_POINTER_UP,
            1,
            40,
            &[(9, 2.0, 2.0), (4, 3.0, 3.0)]
        )),
        ["touch 0:U(3,3) 1:M(2,2)"]
    );
    assert_eq!(
        r.feed(finger(ACTION_UP, 0, 50, &[(9, 2.0, 2.0)])),
        ["touch 1:U(2,2)"]
    );
    assert!(
        r.feed(finger(ACTION_MOVE, 0, 60, &[(9, 3.0, 3.0)]))
            .is_empty()
    );
}

#[test]
fn touch_cancel_variants() {
    let mut r = Rig::new();
    r.feed(finger(ACTION_DOWN, 0, 0, &[(0, 1.0, 1.0)]));
    r.feed(finger(
        ACTION_POINTER_DOWN,
        1,
        1,
        &[(0, 1.0, 1.0), (1, 2.0, 2.0)],
    ));
    assert_eq!(
        r.feed(finger(ACTION_CANCEL, 0, 2, &[(0, 1.0, 1.0), (1, 2.0, 2.0)])),
        ["touch 0:C(1,1) 1:C(2,2)"]
    );
    assert!(
        r.feed(finger(ACTION_MOVE, 0, 3, &[(0, 1.0, 1.0)]))
            .is_empty()
    );

    // FLAG_CANCELED on an UP.
    let mut r = Rig::new();
    r.feed(finger(ACTION_DOWN, 0, 0, &[(0, 1.0, 1.0)]));
    let mut up = finger(ACTION_UP, 0, 1, &[(0, 1.0, 1.0)]);
    up.flags = FLAG_CANCELED;
    assert_eq!(r.feed(up), ["touch 0:C(1,1)"]);
}

#[test]
fn palm_tool_mid_stroke() {
    let mut r = Rig::new();
    r.feed(finger(ACTION_DOWN, 0, 0, &[(0, 1.0, 1.0)]));
    r.feed(finger(
        ACTION_POINTER_DOWN,
        1,
        1,
        &[(0, 1.0, 1.0), (1, 2.0, 2.0)],
    ));
    let m = motion(
        ACTION_MOVE,
        0,
        0,
        vec![(
            2,
            vec![
                ptr(0, TOOL_TYPE_FINGER, 1.0, 1.0, 1.0),
                ptr(1, TOOL_TYPE_PALM, 2.0, 2.0, 1.0),
            ],
        )],
    );
    assert_eq!(r.feed(m), ["touch 0:M(1,1) 1:C(2,2)"]);
    // The palm is forgotten for the rest of its life.
    let m = motion(
        ACTION_MOVE,
        0,
        0,
        vec![(
            3,
            vec![
                ptr(0, TOOL_TYPE_FINGER, 1.0, 1.0, 1.0),
                ptr(1, TOOL_TYPE_FINGER, 9.0, 9.0, 1.0),
            ],
        )],
    );
    assert_eq!(r.feed(m), ["touch 0:M(1,1)"]);
    assert_eq!(
        r.feed(finger(
            ACTION_POINTER_UP,
            1,
            4,
            &[(0, 1.0, 1.0), (1, 9.0, 9.0)]
        )),
        ["touch 0:M(1,1)"]
    );
}

#[test]
fn eleventh_finger_is_ignored() {
    let mut r = Rig::new();
    let mut ps: Vec<(i32, f32, f32)> = Vec::new();
    for i in 0..10 {
        ps.push((i, i as f32, 0.0));
        let a = if i == 0 {
            ACTION_DOWN
        } else {
            ACTION_POINTER_DOWN
        };
        let out = r.feed(finger(a, ps.len() - 1, i as u64, &ps));
        assert_eq!(out.len(), 1);
    }
    ps.push((10, 10.0, 0.0));
    assert!(
        r.feed(finger(ACTION_POINTER_DOWN, 10, 20, &ps))
            .iter()
            .all(|s| !s.contains("10,0"))
    );
    // Its moves and up stay silent for it: only the ten contacts are listed.
    let out = r.feed(finger(ACTION_MOVE, 0, 21, &ps));
    assert_eq!(out[0].matches(':').count(), 10);
    ps.remove(0);
    let mut all = ps.clone();
    all.insert(0, (0, 0.0, 0.0));
    // Finger 0 lifts, the ignored finger 10 stays ignored even though id 0 is free.
    r.feed(finger(ACTION_POINTER_UP, 0, 22, &all));
    let out = r.feed(finger(ACTION_MOVE, 0, 23, &ps));
    assert_eq!(out[0].matches(':').count(), 9);
}

// ---- 2. pen basics --------------------------------------------------------

#[test]
fn pen_hover_down_move_up_hover_exit() {
    let mut r = Rig::new();
    assert_eq!(r.feed(hover(0, 100.0, 100.0, 0)), ["pen I 100,100"]);
    assert_eq!(
        r.feed(pen_down(10, 100.0, 100.0, 0)),
        ["pen IC 100,100 p=32768"]
    );
    assert_eq!(
        r.feed(pen_move(20, 120.0, 100.0, 0)),
        ["pen IC 120,100 p=32768"]
    );
    assert_eq!(r.feed(pen_up(30, 120.0, 100.0, 0)), ["pen I 120,100"]);
    assert_eq!(r.feed(hover(40, 130.0, 100.0, 0)), ["pen I 130,100"]);
    assert!(r.feed(exit(50, 130.0, 100.0)).is_empty());
    assert_eq!(r.e.next_deadline(), Some(ms(90)));
    assert_eq!(r.e.pen_position(), Some((130, 100)));
    assert_eq!(r.tick(89), Vec::<String>::new());
    assert_eq!(r.tick(90), ["pen - 130,100"]);
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.e.pen_position(), None);
}

#[test]
fn hover_exit_followed_by_down_emits_no_out() {
    let mut r = Rig::new();
    r.feed(hover(0, 100.0, 100.0, 0));
    r.feed(exit(10, 100.0, 100.0));
    assert_eq!(r.e.next_deadline(), Some(ms(50)));
    assert_eq!(
        r.feed(pen_down(12, 100.0, 100.0, 0)),
        ["pen IC 100,100 p=32768"]
    );
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.tick(1000), Vec::<String>::new());
}

#[test]
fn hover_exit_out_fires_at_next_motion_past_the_deadline() {
    let mut r = Rig::new();
    r.feed(hover(0, 100.0, 100.0, 0));
    r.feed(exit(10, 100.0, 100.0));
    assert_eq!(
        r.feed(hover(100, 200.0, 100.0, 0)),
        ["pen - 100,100", "pen I 200,100"]
    );
}

#[test]
fn stylus_cancel_gives_lift_sample_then_delayed_out() {
    let mut r = Rig::new();
    r.feed(pen_down(0, 10.0, 10.0, 0));
    assert_eq!(
        r.feed(pen_at(ACTION_CANCEL, 5, 10.0, 10.0, 0)),
        ["pen I 10,10"]
    );
    assert_eq!(r.tick(45), ["pen - 10,10"]);
}

#[test]
fn mouse_scroll_event() {
    let mut r = Rig::new();
    let mut m = motion(
        ACTION_SCROLL,
        0,
        0,
        vec![(0, vec![ptr(0, TOOL_TYPE_MOUSE, 7.0, 8.0, 0.0)])],
    );
    m.vscroll = 1.0;
    m.hscroll = -0.5;
    assert_eq!(r.feed(m), ["scroll -60,120 m0 @7,8"]);
    // Other mouse events are ignored.
    let m = motion(
        ACTION_HOVER_MOVE,
        0,
        0,
        vec![(1, vec![ptr(0, TOOL_TYPE_MOUSE, 7.0, 8.0, 0.0)])],
    );
    assert!(r.feed(m).is_empty());
}

// ---- 3. palm rejection ----------------------------------------------------

#[test]
fn pen_enter_cancels_active_fingers_in_one_frame() {
    let mut r = Rig::new();
    r.feed(finger(ACTION_DOWN, 0, 0, &[(0, 1.0, 1.0)]));
    r.feed(finger(
        ACTION_POINTER_DOWN,
        1,
        1,
        &[(0, 1.0, 1.0), (1, 2.0, 2.0)],
    ));
    assert_eq!(
        r.feed(hover(5, 500.0, 500.0, 0)),
        ["touch 0:C(1,1) 1:C(2,2)", "pen I 500,500"]
    );
    // They stay ignored until they lift.
    assert!(
        r.feed(finger(ACTION_MOVE, 0, 6, &[(0, 3.0, 3.0), (1, 4.0, 4.0)]))
            .is_empty()
    );
    assert!(
        r.feed(finger(
            ACTION_POINTER_UP,
            1,
            7,
            &[(0, 3.0, 3.0), (1, 4.0, 4.0)]
        ))
        .is_empty()
    );
    assert!(r.feed(finger(ACTION_UP, 0, 8, &[(0, 3.0, 3.0)])).is_empty());
    // A second hover sample does not cancel again.
    assert_eq!(r.feed(hover(9, 501.0, 500.0, 0)), ["pen I 501,500"]);
}

#[test]
fn finger_down_while_pen_in_range_is_ignored_for_its_life() {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    assert!(
        r.feed(finger(ACTION_DOWN, 0, 1, &[(0, 1.0, 1.0)]))
            .is_empty()
    );
    assert!(
        r.feed(finger(ACTION_MOVE, 0, 2, &[(0, 2.0, 2.0)]))
            .is_empty()
    );
    // The pen leaves; the finger is still ignored.
    r.feed(exit(3, 500.0, 500.0));
    r.tick(100);
    assert!(
        r.feed(finger(ACTION_MOVE, 0, 4000, &[(0, 3.0, 3.0)]))
            .is_empty()
    );
    assert!(
        r.feed(finger(ACTION_UP, 0, 4001, &[(0, 3.0, 3.0)]))
            .is_empty()
    );
    // A new finger much later is accepted.
    assert_eq!(
        r.feed(finger(ACTION_DOWN, 0, 5000, &[(0, 1.0, 1.0)])),
        ["touch 0:D(1,1)"]
    );
}

#[test]
fn palm_grace_period_after_the_pen_left() {
    for (finger_t, accepted) in [(1240, false), (1440, true)] {
        let mut r = Rig::new();
        r.feed(hover(0, 500.0, 500.0, 0));
        r.feed(exit(1000, 500.0, 500.0));
        assert_eq!(r.tick(1040), ["pen - 500,500"]);
        let out = r.feed(finger(ACTION_DOWN, 0, finger_t, &[(0, 1.0, 1.0)]));
        assert_eq!(!out.is_empty(), accepted, "finger at {finger_t}");
    }
}

#[test]
fn palm_rejection_off_passes_fingers() {
    let mut r = Rig::with(InputConfig {
        palm_rejection: false,
        ..InputConfig::default()
    });
    r.feed(finger(ACTION_DOWN, 0, 0, &[(0, 1.0, 1.0)]));
    assert_eq!(r.feed(hover(1, 500.0, 500.0, 0)), ["pen I 500,500"]);
    assert_eq!(
        r.feed(finger(
            ACTION_POINTER_DOWN,
            1,
            2,
            &[(0, 1.0, 1.0), (1, 2.0, 2.0)]
        )),
        ["touch 0:M(1,1) 1:D(2,2)"]
    );
}

// ---- 4. raw profile -------------------------------------------------------

#[test]
fn raw_single_click_resolves_after_the_double_click_time() {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    assert_eq!(r.feed(press(100, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.next_deadline(), Some(ms(400)));
    assert_eq!(r.feed(release(200, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.next_deadline(), Some(ms(500)));
    assert_eq!(r.tick(499), Vec::<String>::new());
    assert_eq!(r.tick(500), ["pen IB 500,500", "pen I 500,500"]);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn raw_double_click_toggles_local_zoom() {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    assert_eq!(r.feed(press(300, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(
        r.e.view(),
        ViewTransform {
            scale: 2.0,
            tx: -500.0,
            ty: -500.0
        }
    );
    assert_eq!(r.feed(release(400, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.next_deadline(), None);
    // Second double click goes back to 1x.
    r.feed(press(1000, 500.0, 500.0));
    r.feed(release(1100, 500.0, 500.0));
    assert_eq!(r.feed(press(1200, 500.0, 500.0)), ["pen I 500,500"]);
    r.feed(release(1300, 500.0, 500.0));
    assert_eq!(r.e.view(), ViewTransform::IDENTITY);
    assert_eq!(r.tick(5000), Vec::<String>::new());
}

#[test]
fn raw_press_then_tip_touch_flushes_barrel_before_contact() {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(
        r.feed(pen_down(150, 500.0, 500.0, PRIMARY)),
        ["pen IB 500,500", "pen ICB 500,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_move(160, 510.0, 500.0, PRIMARY)),
        ["pen ICB 510,500 p=32768"]
    );
    // The release drops BARREL at once.
    assert_eq!(r.feed(release(170, 510.0, 500.0)), ["pen IC 510,500"]);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn raw_hold_emits_barrel_at_exactly_press_plus_t() {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(r.tick(399), Vec::<String>::new());
    r.e.tick(ms(450), &mut r.out);
    assert_eq!(r.out.len(), 1);
    assert_eq!(pen_t(&r.out[0]), ms(400));
    assert_eq!(show(&r.out[0]), "pen IB 500,500");
    r.raw();
    assert_eq!(
        r.feed(hover(500, 510.0, 500.0, PRIMARY)),
        ["pen IB 510,500"]
    );
    assert_eq!(r.feed(release(600, 510.0, 500.0)), ["pen I 510,500"]);
    // Nothing else is pending: no late click.
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn raw_press_in_contact_is_forwarded_at_once() {
    let mut r = Rig::new();
    r.feed(pen_down(0, 500.0, 500.0, 0));
    assert_eq!(
        r.feed(pen_move(10, 501.0, 500.0, PRIMARY)),
        ["pen ICB 501,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_up(20, 501.0, 500.0, PRIMARY)),
        ["pen IB 501,500"]
    );
    assert_eq!(r.feed(release(30, 501.0, 500.0)), ["pen I 501,500"]);
    assert_eq!(r.e.next_deadline(), None);
}

fn zoomed_rig() -> Rig {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(10, 500.0, 500.0));
    r.feed(release(20, 500.0, 500.0));
    r.feed(press(30, 500.0, 500.0));
    r.feed(release(40, 500.0, 500.0));
    assert_eq!(r.e.view().scale, 2.0);
    r
}

#[test]
fn raw_zoomed_hover_drag_pans_with_grab_semantics() {
    let mut r = zoomed_rig();
    let before = r.e.pen_position();
    r.feed(press(1000, 500.0, 500.0));
    // Within the slop nothing happens.
    assert_eq!(
        r.feed(hover(1010, 510.0, 500.0, PRIMARY)),
        ["pen I 505,500"]
    );
    assert_eq!(r.e.view().tx, -500.0);
    // Beyond it the view follows the pen; the pen stays over the same content.
    let out = r.feed(hover(1020, 600.0, 520.0, PRIMARY));
    assert_eq!(out.len(), 1);
    assert!(!out[0].contains('B'));
    let v = r.e.view();
    assert_eq!((v.scale, v.tx, v.ty), (2.0, -500.0 + 90.0, -500.0 + 20.0));
    assert_eq!(r.e.pen_position(), Some(normalize_pair(600.0, 520.0, v)));
    // Keeps panning until the release; BARREL never appears, even after T.
    assert_eq!(r.tick(2000), Vec::<String>::new());
    let out = r.feed(hover(2010, 650.0, 520.0, PRIMARY));
    assert!(!out[0].contains('B'));
    assert_eq!(r.e.view().tx, -500.0 + 140.0);
    r.feed(release(2020, 650.0, 520.0));
    assert_eq!(r.e.next_deadline(), None);
    assert_ne!(before, None);
}

fn normalize_pair(x: f32, y: f32, v: ViewTransform) -> (u16, u16) {
    let m = Mapper {
        view: v,
        w: SIDE,
        h: SIDE,
    };
    m.map(x, y)
}

#[test]
fn raw_zoomed_press_then_tip_touch_flushes_raw() {
    let mut r = zoomed_rig();
    r.feed(press(1000, 500.0, 500.0));
    assert_eq!(
        r.feed(pen_down(1050, 500.0, 500.0, PRIMARY)),
        ["pen IB 500,500", "pen ICB 500,500 p=32768"]
    );
}

#[test]
fn raw_with_everything_raw_resolves_at_the_press() {
    let raw = ButtonMap {
        double_click: ButtonAction::None,
        ..ButtonMap::default()
    };
    let mut r = Rig::with(InputConfig {
        profile: Profile::Custom,
        custom: raw,
        ..InputConfig::default()
    });
    r.feed(hover(0, 500.0, 500.0, 0));
    assert_eq!(r.feed(press(100, 500.0, 500.0)), ["pen IB 500,500"]);
    assert_eq!(r.feed(release(110, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn pen_out_during_gesture() {
    // Held first press: dropped silently.
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(10, 500.0, 500.0));
    r.feed(exit(20, 500.0, 500.0));
    assert_eq!(r.tick(60), ["pen - 500,500"]);
    // Waiting click: resolves first.
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(10, 500.0, 500.0));
    r.feed(release(20, 500.0, 500.0));
    r.feed(exit(30, 500.0, 500.0));
    assert_eq!(
        r.tick(70),
        ["pen IB 500,500", "pen I 500,500", "pen - 500,500"]
    );
}

#[test]
fn tip_touch_while_waiting_resolves_the_click_first() {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(10, 500.0, 500.0));
    r.feed(release(20, 500.0, 500.0));
    assert_eq!(
        r.feed(pen_down(30, 500.0, 500.0, 0)),
        ["pen IB 500,500", "pen I 500,500", "pen IC 500,500 p=32768"]
    );
}

// ---- 5. desktop profile ---------------------------------------------------

#[test]
fn desktop_click_is_a_right_click_after_the_double_click_time() {
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    assert_eq!(r.tick(499), Vec::<String>::new());
    assert_eq!(r.tick(500), ["btn R+ 500,500", "btn R- 500,500"]);
    assert!(r.out.is_empty());
}

#[test]
fn desktop_button_tap_is_a_right_click_at_the_tap_position() {
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    // Contact is suppressed: reported as hover.
    assert_eq!(
        r.feed(pen_down(150, 510.0, 500.0, PRIMARY)),
        ["pen I 510,500"]
    );
    assert_eq!(
        r.feed(pen_move(160, 512.0, 500.0, PRIMARY)),
        ["pen I 512,500"]
    );
    assert_eq!(
        r.feed(pen_up(170, 512.0, 500.0, PRIMARY)),
        ["pen I 512,500", "btn R+ 512,500", "btn R- 512,500"]
    );
    assert_eq!(r.feed(release(200, 512.0, 500.0)), ["pen I 512,500"]);
    assert_eq!(r.e.next_deadline(), None);
    // Contact works again afterwards.
    assert_eq!(
        r.feed(pen_down(300, 512.0, 500.0, 0)),
        ["pen IC 512,500 p=32768"]
    );
}

#[test]
fn desktop_hold_and_contact_drag_scrolls_with_remainders() {
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    // 48 px down = one notch up (content follows the pen), at the press position.
    assert_eq!(
        r.feed(pen_move(120, 500.0, 548.0, PRIMARY)),
        ["scroll 0,120 m0 @500,500", "pen I 500,548"]
    );
    // 2.5 units per px: 2, then 3 (remainder carried).
    assert_eq!(
        r.feed(pen_move(130, 500.0, 549.0, PRIMARY))[0],
        "scroll 0,2 m0 @500,500"
    );
    assert_eq!(
        r.feed(pen_move(140, 500.0, 550.0, PRIMARY))[0],
        "scroll 0,3 m0 @500,500"
    );
    // Moving right scrolls left.
    assert_eq!(
        r.feed(pen_move(150, 548.0, 550.0, PRIMARY))[0],
        "scroll -120,0 m0 @500,500"
    );
    // Lifting after a drag is not a tap.
    assert_eq!(
        r.feed(pen_up(160, 548.0, 550.0, PRIMARY)),
        ["pen I 548,550"]
    );
    assert_eq!(r.feed(release(170, 548.0, 550.0)), ["pen I 548,550"]);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn desktop_drag_ends_with_barrel_release_and_contact_stays_suppressed() {
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    r.feed(pen_move(120, 500.0, 548.0, PRIMARY));
    assert_eq!(r.feed(release(130, 500.0, 548.0)), ["pen I 500,548"]);
    // Still suppressed, and no scrolling any more.
    assert_eq!(r.feed(pen_move(140, 500.0, 600.0, 0)), ["pen I 500,600"]);
    assert_eq!(r.feed(pen_up(150, 500.0, 600.0, 0)), ["pen I 500,600"]);
    assert_eq!(
        r.feed(pen_down(160, 500.0, 600.0, 0)),
        ["pen IC 500,600 p=32768"]
    );
}

#[test]
fn desktop_hover_drag_zooms_the_pc_with_ctrl() {
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    // Pen up the screen = zoom in = positive.
    assert_eq!(
        r.feed(hover(110, 500.0, 452.0, PRIMARY)),
        ["scroll 0,120 m1 @500,500", "pen I 500,452"]
    );
    // Pen down = zoom out; horizontal movement is ignored.
    assert_eq!(
        r.feed(hover(120, 700.0, 476.0, PRIMARY))[0],
        "scroll 0,-60 m1 @500,500"
    );
    r.feed(release(130, 700.0, 476.0));
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn desktop_long_still_hold_then_release_is_a_click() {
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(r.tick(2000), Vec::<String>::new());
    assert_eq!(r.feed(release(2000, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.tick(2300), ["btn R+ 500,500", "btn R- 500,500"]);
}

#[test]
fn late_click_happens_where_it_was_released() {
    // Desktop: right click at the release position even if the pen moved on.
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    assert_eq!(r.feed(hover(300, 700.0, 500.0, 0)), ["pen I 700,500"]);
    assert_eq!(r.tick(500), ["btn R+ 500,500", "btn R- 500,500"]);
    assert_eq!(r.feed(hover(510, 710.0, 500.0, 0)), ["pen I 710,500"]);

    // Raw: both samples at the release position, then regular ones follow.
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    r.feed(hover(300, 700.0, 500.0, 0));
    assert_eq!(r.tick(500), ["pen IB 500,500", "pen I 500,500"]);
    assert_eq!(r.feed(hover(510, 710.0, 500.0, 0)), ["pen I 710,500"]);

    // Tip contact elsewhere: the click resolves first, at the old position.
    let mut r = Rig::profile(Profile::Desktop);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    r.feed(hover(300, 700.0, 500.0, 0));
    assert_eq!(
        r.feed(pen_down(350, 700.0, 500.0, 0)),
        ["btn R+ 500,500", "btn R- 500,500", "pen IC 700,500 p=32768"]
    );
}

// ---- 6. drawing profile ---------------------------------------------------

#[test]
fn drawing_hold_switches_to_the_eraser() {
    let mut r = Rig::profile(Profile::Drawing);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(r.tick(400), ["pen - 500,500", "pen IE 500,500"]);
    assert_eq!(
        r.feed(hover(410, 505.0, 500.0, PRIMARY)),
        ["pen IE 505,500"]
    );
    assert_eq!(
        r.feed(pen_down(420, 505.0, 500.0, PRIMARY)),
        ["pen ICE 505,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_up(430, 505.0, 500.0, PRIMARY)),
        ["pen IE 505,500"]
    );
    // Release in hover: back to the normal tool.
    assert_eq!(
        r.feed(release(440, 505.0, 500.0)),
        ["pen - 505,500", "pen I 505,500"]
    );
    assert_eq!(r.feed(hover(450, 506.0, 500.0, 0)), ["pen I 506,500"]);
}

#[test]
fn drawing_release_in_contact_switches_after_the_lift() {
    let mut r = Rig::profile(Profile::Drawing);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.tick(400);
    r.feed(pen_down(420, 500.0, 500.0, PRIMARY));
    assert_eq!(r.feed(release(430, 500.0, 500.0)), ["pen ICE 500,500"]);
    assert_eq!(
        r.feed(pen_move(440, 510.0, 500.0, 0)),
        ["pen ICE 510,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_up(450, 510.0, 500.0, 0)),
        ["pen IE 510,500", "pen - 510,500", "pen I 510,500"]
    );
}

#[test]
fn drawing_click_is_late_raw() {
    let mut r = Rig::profile(Profile::Drawing);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(150, 500.0, 500.0));
    assert_eq!(r.tick(450), ["pen IB 500,500", "pen I 500,500"]);
}

// ---- 7. config ------------------------------------------------------------

#[test]
fn config_json_defaults_unknown_fields_and_errors() {
    let c = InputConfig::from_json("{}").unwrap();
    assert_eq!(c, InputConfig::default());
    assert_eq!(c.button_map(), ButtonMap::for_profile(Profile::Raw));

    let c = InputConfig::from_json(
        r#"{"profile":"custom","future_field":1,"slop_px":10.0,
            "custom":{"click":"middle_click","hold_drag_hover":"pc_zoom","other":true}}"#,
    )
    .unwrap();
    assert_eq!(c.slop_px, 10.0);
    assert_eq!(c.double_click_ms, 300);
    let m = c.button_map();
    assert_eq!(m.click, ButtonAction::MiddleClick);
    assert_eq!(m.hold_drag_hover, ButtonAction::PcZoom);
    assert_eq!(m.double_click, ButtonAction::LocalZoom);
    assert_eq!(m.hold, ButtonAction::Raw);

    assert!(InputConfig::from_json(r#"{"profile":"nope"}"#).is_err());
    assert!(InputConfig::from_json(r#"{"custom":{"click":"sing"}}"#).is_err());
    assert!(InputConfig::from_json("not json").is_err());
}

#[test]
fn invalid_action_trigger_combinations_become_none() {
    use ButtonAction::*;
    let c = InputConfig {
        profile: Profile::Custom,
        custom: ButtonMap {
            click: Scroll,
            double_click: Eraser,
            hold: PcZoom,
            double_hold: RightClick,
            hold_drag_contact: RightClick,
            hold_drag_hover: LocalZoom,
        },
        ..InputConfig::default()
    };
    let m = c.button_map();
    assert_eq!(
        (
            m.click,
            m.double_click,
            m.hold,
            m.double_hold,
            m.hold_drag_contact,
            m.hold_drag_hover
        ),
        (None, None, None, None, None, None)
    );
    // Valid ones survive.
    let c = InputConfig {
        profile: Profile::Custom,
        custom: ButtonMap {
            click: MiddleClick,
            double_click: MiddleClick,
            hold: Eraser,
            double_hold: Keys,
            hold_drag_contact: PcZoom,
            hold_drag_hover: Scroll,
        },
        ..InputConfig::default()
    };
    assert_eq!(c.button_map(), c.custom);
    // A raw double click is just two raw clicks.
    let c = InputConfig {
        profile: Profile::Custom,
        custom: ButtonMap {
            double_click: Raw,
            ..ButtonMap::default()
        },
        ..InputConfig::default()
    };
    assert_eq!(c.button_map().double_click, None);
}

#[test]
fn profile_maps() {
    use ButtonAction::*;
    let t = |p| {
        let m = ButtonMap::for_profile(p);
        [
            m.click,
            m.double_click,
            m.hold,
            m.double_hold,
            m.hold_drag_contact,
            m.hold_drag_hover,
        ]
    };
    assert_eq!(t(Profile::Raw), [Raw, LocalZoom, Raw, None, Raw, Raw]);
    assert_eq!(
        t(Profile::Desktop),
        [RightClick, LocalZoom, None, None, Scroll, PcZoom]
    );
    assert_eq!(
        t(Profile::Drawing),
        [Raw, LocalZoom, Eraser, None, Raw, Raw]
    );
    assert_eq!(
        t(Profile::GraphicsTablet),
        [RightClick, LocalZoom, Keys, Keys, Raw, Raw]
    );
}

#[test]
fn set_config_cancels_a_pending_gesture() {
    let mut r = Rig::new();
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.e.set_config(InputConfig::default());
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.tick(1000), Vec::<String>::new());
}

// ---- 8. view mapping ------------------------------------------------------

#[test]
fn view_clamping_at_all_edges() {
    let v = |scale, tx, ty| ViewTransform { scale, tx, ty }.clamped(1000.0, 500.0);
    assert_eq!(
        v(2.0, -5000.0, 10.0),
        ViewTransform {
            scale: 2.0,
            tx: -1000.0,
            ty: 0.0
        }
    );
    assert_eq!(
        v(2.0, 100.0, -9999.0),
        ViewTransform {
            scale: 2.0,
            tx: 0.0,
            ty: -500.0
        }
    );
    assert_eq!(
        v(9.0, -1.0, -1.0),
        ViewTransform {
            scale: 4.0,
            tx: -1.0,
            ty: -1.0
        }
    );
    assert_eq!(v(0.2, -50.0, -50.0), ViewTransform::IDENTITY);
}

#[test]
fn inverse_mapping_with_zoom() {
    let m = Mapper {
        view: ViewTransform {
            scale: 2.0,
            tx: -1000.0,
            ty: -500.0,
        },
        w: 1000.0,
        h: 500.0,
    };
    // Bottom-right zoomed: the right/bottom edges of the stream.
    assert_eq!(m.map(1000.0, 500.0), (65535, 65535));
    assert_eq!(m.map(0.0, 0.0), (32768, 32768));
    assert_eq!(m.map(1000.0, 0.0), (65535, 32768));
    assert_eq!(m.map(0.0, 500.0), (32768, 65535));
    let id = Mapper {
        view: ViewTransform::IDENTITY,
        w: 1000.0,
        h: 500.0,
    };
    assert_eq!(id.map(0.0, 0.0), (0, 0));
    assert_eq!(id.map(1000.0, 500.0), (65535, 65535));
    assert_eq!(normalize(1000.0, 1000.0), 65535);
}

#[test]
fn local_zoom_keeps_the_pen_point_and_clamps() {
    let mut e = InputEngine::new(
        InputConfig {
            zoom_factor: 4.0,
            ..InputConfig::default()
        },
        1000.0,
        500.0,
    );
    let mut out = Vec::new();
    let mut ev = |e: &mut InputEngine, m: Motion| e.motion(&m, &mut out);
    ev(&mut e, hover(0, 1000.0, 500.0, 0));
    for t in [10, 30] {
        ev(&mut e, press(t, 1000.0, 500.0));
        ev(&mut e, release(t + 10, 1000.0, 500.0));
    }
    // Pen in the bottom-right corner, zoom 4: pinned to the right/bottom edge.
    assert_eq!(
        e.view(),
        ViewTransform {
            scale: 4.0,
            tx: -3000.0,
            ty: -1500.0
        }
    );
    assert_eq!(e.pen_position(), Some((65535, 65535)));
}

#[test]
fn set_view_size_resets_zoom_only_on_change() {
    let mut r = zoomed_rig();
    r.e.set_view_size(SIDE, SIDE);
    assert_eq!(r.e.view().scale, 2.0);
    r.e.set_view_size(100.0, 100.0);
    assert_eq!(r.e.view(), ViewTransform::IDENTITY);
}

// ---- 9. tilt --------------------------------------------------------------

#[test]
fn tilt_signs() {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
    assert_eq!(tilt_xy(0.0, 1.0), (0, 0));
    // Leaning toward the top of the screen: tilt_y negative.
    assert_eq!(tilt_xy(FRAC_PI_4, 0.0), (0, -4500));
    // Leaning right: tilt_x positive.
    assert_eq!(tilt_xy(FRAC_PI_4, FRAC_PI_2), (4500, 0));
    // Leaning toward the user / left.
    assert_eq!(tilt_xy(FRAC_PI_4, std::f32::consts::PI), (0, 4500));
    assert_eq!(tilt_xy(FRAC_PI_4, -FRAC_PI_2), (-4500, 0));
    // Clamped and NaN-safe.
    let (x, y) = tilt_xy(FRAC_PI_2, FRAC_PI_2);
    assert!((8990..=9000).contains(&x) && y == 0);
    assert_eq!(tilt_xy(f32::NAN, 0.0), (0, 0));
}

#[test]
fn pen_sample_carries_tilt() {
    let mut r = Rig::new();
    let mut p = ptr(7, TOOL_TYPE_STYLUS, 1.0, 2.0, 0.0);
    p.tilt = std::f32::consts::FRAC_PI_4;
    p.orientation = std::f32::consts::FRAC_PI_2;
    r.e.motion(
        &motion(ACTION_HOVER_MOVE, 0, 0, vec![(0, vec![p])]),
        &mut r.out,
    );
    match &r.out[0] {
        Message::Pen(s) => assert_eq!((s.tilt_x, s.tilt_y), (4500, 0)),
        _ => panic!(),
    }
}

#[test]
fn eraser_tool_sets_the_eraser_flag() {
    let mut r = Rig::new();
    let p = ptr(7, TOOL_TYPE_ERASER, 1.0, 2.0, 0.0);
    assert_eq!(
        r.feed(motion(ACTION_HOVER_MOVE, 0, 0, vec![(0, vec![p])])),
        ["pen IE 1,2"]
    );
}

// ---- 10. release_all ------------------------------------------------------

#[test]
fn release_all_releases_everything() {
    // Active contacts are cancelled.
    let mut r = Rig::new();
    r.feed(finger(ACTION_DOWN, 0, 0, &[(0, 1.0, 1.0)]));
    r.feed(finger(
        ACTION_POINTER_DOWN,
        1,
        1,
        &[(0, 1.0, 1.0), (1, 2.0, 2.0)],
    ));
    r.e.release_all(ms(5), &mut r.out);
    let shown: Vec<String> = r.raw().iter().map(show).collect();
    assert_eq!(shown, ["touch 0:C(1,1) 1:C(2,2)"]);

    // The pen goes out and a pending gesture is dropped.
    let mut r = Rig::new();
    r.feed(hover(5, 500.0, 500.0, 0));
    r.feed(press(10, 500.0, 500.0));
    r.e.release_all(ms(20), &mut r.out);
    let shown: Vec<String> = r.raw().iter().map(show).collect();
    assert_eq!(shown, ["pen - 500,500"]);
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.e.pen_position(), None);
    assert_eq!(r.tick(5000), Vec::<String>::new());
    assert_eq!(r.feed(release(30, 500.0, 500.0)), ["pen I 500,500"]);
}

// ---- 11. robustness -------------------------------------------------------

#[test]
fn time_going_backwards_and_equal_timestamps_do_not_panic() {
    for profile in [Profile::Raw, Profile::Desktop, Profile::Drawing] {
        let mut r = Rig::profile(profile);
        r.feed(hover(1000, 500.0, 500.0, 0));
        r.feed(press(500, 500.0, 500.0));
        r.feed(release(500, 500.0, 500.0));
        r.feed(press(10, 500.0, 500.0));
        r.feed(pen_down(10, 500.0, 520.0, PRIMARY));
        r.feed(pen_move(0, 500.0, 900.0, PRIMARY));
        r.feed(pen_up(0, 500.0, 900.0, PRIMARY));
        r.feed(release(0, 500.0, 900.0));
        r.feed(exit(0, 500.0, 900.0));
        r.tick(0);
        r.tick(u64::MAX / 2000);
        r.feed(finger(ACTION_DOWN, 0, 5, &[(0, 1.0, 1.0)]));
        r.feed(finger(ACTION_UP, 0, 1, &[(0, 1.0, 1.0)]));
        r.e.release_all(0, &mut r.out);
        r.e.tick(u64::MAX, &mut r.out);
    }
}

#[test]
fn empty_motion_is_ignored() {
    let mut r = Rig::new();
    assert!(r.feed(motion(ACTION_MOVE, 0, 0, vec![])).is_empty());
    assert!(
        r.feed(motion(ACTION_OUTSIDE, 0, 0, vec![(0, vec![])]))
            .is_empty()
    );
}

// ---- 9. keyboard shortcut (Keys action) ------------------------------------

const CTRL_SPACE_DOWN: [&str; 2] = ["key 0xe0 down", "key 0x2c down"];
const CTRL_SPACE_UP: [&str; 2] = ["key 0x2c up", "key 0xe0 up"];

fn keys_of(out: &[String]) -> Vec<&str> {
    out.iter()
        .map(String::as_str)
        .filter(|s| s.starts_with("key "))
        .collect()
}

fn custom_keys(map: ButtonMap, keys: KeyBindings) -> Rig {
    Rig::with(InputConfig {
        profile: Profile::Custom,
        custom: map,
        keys,
        ..InputConfig::default()
    })
}

/// A graphics-tablet rig with the barrel held past the timer (keys down, output drained).
fn graphics_tablet_held() -> Rig {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(r.tick(400), CTRL_SPACE_DOWN);
    r
}

#[test]
fn graphics_tablet_hold_presses_the_combo_at_press_plus_t_and_releases_with_the_barrel() {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    assert_eq!(r.feed(press(100, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.next_deadline(), Some(ms(400)));
    assert_eq!(r.tick(399), Vec::<String>::new());
    r.e.tick(ms(450), &mut r.out);
    assert_eq!(r.out.len(), 2);
    for m in &r.out {
        assert!(matches!(m, Message::Key(k) if k.t_us == ms(400)));
    }
    assert_eq!(
        r.raw().iter().map(show).collect::<Vec<_>>(),
        CTRL_SPACE_DOWN
    );
    assert_eq!(r.e.next_deadline(), None);
    // Pen tip and hover go through without BARREL and without suppression.
    assert_eq!(r.feed(hover(500, 510.0, 500.0, PRIMARY)), ["pen I 510,500"]);
    assert_eq!(
        r.feed(pen_down(600, 510.0, 500.0, PRIMARY)),
        ["pen IC 510,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_move(700, 600.0, 500.0, PRIMARY)),
        ["pen IC 600,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_up(800, 600.0, 500.0, PRIMARY)),
        ["pen I 600,500"]
    );
    // Barrel up: release in reverse order, then the sample.
    let mut want = CTRL_SPACE_UP.map(String::from).to_vec();
    want.push("pen I 600,500".into());
    assert_eq!(r.feed(release(900, 600.0, 500.0)), want);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn graphics_tablet_tip_down_before_the_timer_presses_the_keys_before_the_contact() {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(
        r.feed(pen_down(150, 500.0, 500.0, PRIMARY)),
        [
            CTRL_SPACE_DOWN[0],
            CTRL_SPACE_DOWN[1],
            "pen IC 500,500 p=32768"
        ]
    );
    // The timer no longer does anything.
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.tick(1000), Vec::<String>::new());
}

#[test]
fn graphics_tablet_hover_move_beyond_slop_presses_the_keys_before_the_move() {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    // Within the slop (24 px): still pending.
    assert_eq!(r.feed(hover(110, 520.0, 500.0, PRIMARY)), ["pen I 520,500"]);
    assert_eq!(r.e.next_deadline(), Some(ms(400)));
    assert_eq!(
        r.feed(hover(120, 530.0, 500.0, PRIMARY)),
        [CTRL_SPACE_DOWN[0], CTRL_SPACE_DOWN[1], "pen I 530,500"]
    );
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.feed(hover(130, 540.0, 500.0, PRIMARY)), ["pen I 540,500"]);
}

#[test]
fn graphics_tablet_zoomed_hover_move_pans_instead_of_pressing_keys() {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    for (t, f) in [(10, true), (20, false), (30, true), (40, false)] {
        r.feed(if f {
            press(t, 500.0, 500.0)
        } else {
            release(t, 500.0, 500.0)
        });
    }
    assert_eq!(r.e.view().scale, 2.0);
    let before = r.e.view();
    r.feed(press(100, 500.0, 500.0));
    let out = r.feed(hover(110, 600.0, 500.0, PRIMARY));
    assert!(keys_of(&out).is_empty(), "{out:?}");
    assert_ne!(r.e.view(), before, "panned");
    assert_eq!(r.e.next_deadline(), None);
    let out = r.feed(release(200, 600.0, 500.0));
    assert!(keys_of(&out).is_empty(), "{out:?}");
}

#[test]
fn pen_out_while_held_releases_the_keys_before_the_out_sample() {
    let mut r = graphics_tablet_held();
    r.feed(exit(500, 500.0, 500.0));
    assert_eq!(
        r.tick(540),
        [CTRL_SPACE_UP[0], CTRL_SPACE_UP[1], "pen - 500,500"]
    );
    // The later barrel-up edge has nothing left to release.
    assert_eq!(r.feed(release(600, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn release_all_releases_held_keys_once() {
    let mut r = graphics_tablet_held();
    r.e.release_all(ms(500), &mut r.out);
    let out: Vec<String> = r.raw().iter().map(show).collect();
    assert_eq!(out, [CTRL_SPACE_UP[0], CTRL_SPACE_UP[1], "pen - 500,500"]);
    r.e.release_all(ms(510), &mut r.out);
    assert!(r.out.is_empty());
    assert_eq!(r.feed(release(600, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn set_config_while_held_still_releases_the_keys_on_barrel_up() {
    let mut r = graphics_tablet_held();
    r.e.set_config(InputConfig {
        profile: Profile::Raw,
        ..InputConfig::default()
    });
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(
        r.feed(release(500, 500.0, 500.0)),
        [CTRL_SPACE_UP[0], CTRL_SPACE_UP[1], "pen I 500,500"]
    );
    assert_eq!(r.feed(release(600, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn graphics_tablet_click_is_a_right_click_and_double_click_toggles_the_zoom() {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 400.0, 300.0));
    r.feed(release(200, 400.0, 300.0));
    assert_eq!(r.tick(499), Vec::<String>::new());
    assert_eq!(r.tick(500), ["btn R+ 400,300", "btn R- 400,300"]);

    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    let mut all = Vec::new();
    all.extend(r.feed(press(100, 500.0, 500.0)));
    all.extend(r.feed(release(200, 500.0, 500.0)));
    all.extend(r.feed(press(300, 500.0, 500.0)));
    all.extend(r.feed(release(400, 500.0, 500.0)));
    assert!(keys_of(&all).is_empty(), "{all:?}");
    assert!(all.iter().all(|s| s.starts_with("pen ")), "{all:?}");
    assert_eq!(r.e.view().scale, 2.0);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn custom_click_and_double_click_tap_their_own_combos() {
    let map = ButtonMap {
        click: ButtonAction::Keys,
        double_click: ButtonAction::Keys,
        hold: ButtonAction::None,
        double_hold: ButtonAction::None,
        hold_drag_contact: ButtonAction::None,
        hold_drag_hover: ButtonAction::None,
    };
    let keys = KeyBindings {
        click: KeyCombo {
            mods: KeyCombo::CTRL,
            key: 0x1D,
        },
        double_click: KeyCombo {
            mods: KeyCombo::CTRL | KeyCombo::SHIFT,
            key: 0x1D,
        },
        ..KeyBindings::default()
    };
    // Single click: after the double-click window.
    let mut r = custom_keys(map, keys);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    assert_eq!(r.tick(499), Vec::<String>::new());
    assert_eq!(
        r.tick(500),
        [
            "key 0xe0 down",
            "key 0x1d down",
            "key 0x1d up",
            "key 0xe0 up"
        ]
    );
    // Double click: the second press taps the other combo at once.
    let mut r = custom_keys(map, keys);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    assert_eq!(
        r.feed(press(300, 500.0, 500.0)),
        [
            "key 0xe0 down",
            "key 0xe1 down",
            "key 0x1d down",
            "key 0x1d up",
            "key 0xe1 up",
            "key 0xe0 up",
            "pen I 500,500"
        ]
    );
    assert_eq!(r.feed(release(400, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.next_deadline(), None);
    // Same action with equal combos resolves the click at the release.
    let same = KeyBindings {
        double_click: keys.click,
        ..keys
    };
    let mut r = custom_keys(map, same);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    let out = r.feed(release(200, 500.0, 500.0));
    assert_eq!(keys_of(&out).len(), 4, "{out:?}");
}

#[test]
fn contact_drag_tap_taps_the_click_combo() {
    let map = ButtonMap {
        click: ButtonAction::Keys,
        double_click: ButtonAction::None,
        hold: ButtonAction::None,
        double_hold: ButtonAction::None,
        hold_drag_contact: ButtonAction::Scroll,
        hold_drag_hover: ButtonAction::None,
    };
    let keys = KeyBindings {
        click: KeyCombo { mods: 0, key: 0x1D },
        ..KeyBindings::default()
    };
    let mut r = custom_keys(map, keys);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    assert_eq!(
        r.feed(pen_up(120, 500.0, 500.0, PRIMARY)),
        ["pen I 500,500", "key 0x1d down", "key 0x1d up"]
    );
}

#[test]
fn keys_hold_wins_over_the_drag_triggers() {
    let map = ButtonMap {
        click: ButtonAction::None,
        double_click: ButtonAction::None,
        hold: ButtonAction::Keys,
        double_hold: ButtonAction::None,
        hold_drag_contact: ButtonAction::Scroll,
        hold_drag_hover: ButtonAction::PcZoom,
    };
    let mut r = custom_keys(map, KeyBindings::default());
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    let mut all = r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    all.extend(r.feed(pen_move(120, 500.0, 700.0, PRIMARY)));
    assert_eq!(
        all,
        [
            CTRL_SPACE_DOWN[0],
            CTRL_SPACE_DOWN[1],
            "pen IC 500,500 p=32768",
            "pen IC 500,700 p=32768"
        ]
    );
    // Hover movement resolves it as keys too, not as a PC zoom.
    let mut r = custom_keys(map, KeyBindings::default());
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    let out = r.feed(hover(110, 500.0, 600.0, PRIMARY));
    assert_eq!(
        out,
        [CTRL_SPACE_DOWN[0], CTRL_SPACE_DOWN[1], "pen I 500,600"]
    );
}

#[test]
fn keys_press_starting_with_the_tip_down_is_forwarded_raw() {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(pen_down(0, 500.0, 500.0, 0));
    assert_eq!(
        r.feed(pen_move(10, 501.0, 500.0, PRIMARY)),
        ["pen ICB 501,500 p=32768"]
    );
    assert_eq!(r.tick(1000), Vec::<String>::new());
    assert_eq!(
        r.feed(pen_up(1100, 501.0, 500.0, PRIMARY)),
        ["pen IB 501,500"]
    );
    assert_eq!(r.feed(release(1200, 501.0, 500.0)), ["pen I 501,500"]);
}

#[test]
fn keys_with_an_empty_combo_behave_as_none() {
    let cfg = InputConfig {
        profile: Profile::GraphicsTablet,
        keys: KeyBindings {
            hold: KeyCombo::default(),
            ..KeyBindings::default()
        },
        ..InputConfig::default()
    };
    assert_eq!(cfg.button_map().hold, ButtonAction::None);
    // An invalid key with no modifiers sanitizes to empty too.
    let cfg2 = InputConfig {
        keys: KeyBindings {
            hold: KeyCombo {
                mods: 0x10,
                key: 0x03,
            },
            ..KeyBindings::default()
        },
        ..cfg.clone()
    };
    assert_eq!(cfg2.button_map().hold, ButtonAction::None);
    let mut r = Rig::with(cfg);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(r.tick(1000), Vec::<String>::new());
    assert_eq!(r.feed(release(1100, 500.0, 500.0)), ["pen I 500,500"]);
    // The click trigger is unaffected by the hold combo.
    assert_eq!(r.e.next_deadline(), Some(ms(1400)));

    // Custom: Keys on click with an empty click combo is None as well.
    let c = InputConfig {
        profile: Profile::Custom,
        custom: ButtonMap {
            click: ButtonAction::Keys,
            double_click: ButtonAction::Keys,
            ..ButtonMap::default()
        },
        ..InputConfig::default()
    };
    let m = c.button_map();
    assert_eq!(
        (m.click, m.double_click),
        (ButtonAction::None, ButtonAction::None)
    );
}

#[test]
fn keys_config_json_and_sanitizing() {
    let c = InputConfig::from_json(r#"{"profile":"graphics_tablet"}"#).unwrap();
    assert_eq!(c.profile, Profile::GraphicsTablet);
    assert_eq!(c.keys, KeyBindings::default());
    assert_eq!(
        c.keys.hold,
        KeyCombo {
            mods: KeyCombo::CTRL,
            key: 0x2C
        }
    );
    assert!(c.keys.click.is_empty() && c.keys.double_click.is_empty());

    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"click":"keys","hold_drag_hover":"keys"},
            "keys":{"hold":{"mods":3,"key":44},"click":{"mods":1}}}"#,
    )
    .unwrap();
    assert_eq!(c.custom.click, ButtonAction::Keys);
    assert_eq!(c.keys.hold, KeyCombo { mods: 3, key: 44 });
    assert_eq!(c.keys.click, KeyCombo { mods: 1, key: 0 });
    assert_eq!(c.keys.double_click, KeyCombo::default());
    // Keys is not valid on a drag trigger.
    assert_eq!(c.button_map().hold_drag_hover, ButtonAction::None);
    assert_eq!(c.button_map().click, ButtonAction::Keys);
    let j = serde_json::to_string(&c).unwrap();
    assert_eq!(InputConfig::from_json(&j).unwrap(), c);

    let k = KeyCombo {
        mods: 0xFF,
        key: 0x03,
    }
    .sanitized();
    assert_eq!(k, KeyCombo { mods: 0x0F, key: 0 });
    for (key, ok) in [
        (0x04, true),
        (0x73, true),
        (0x74, false),
        (0xE0, true),
        (0xE7, true),
        (0xE8, false),
        (0x2C, true),
    ] {
        let k = KeyCombo { mods: 0, key }.sanitized();
        assert_eq!(k.key, if ok { key } else { 0 }, "{key:#x}");
    }
}

// ---- double click + hold -----------------------------------------------------

const CTRL_ALT_SPACE_DOWN: [&str; 3] = ["key 0xe0 down", "key 0xe2 down", "key 0x2c down"];
const CTRL_ALT_SPACE_UP: [&str; 3] = ["key 0x2c up", "key 0xe2 up", "key 0xe0 up"];

/// A graphics-tablet rig with the second press of a double click down and unresolved
/// (first press at 100, release at 200, second press at 300; output drained).
fn graphics_tablet_second_press() -> Rig {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    assert_eq!(r.feed(press(300, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.view().scale, 1.0, "the double click is not fired yet");
    r
}

#[test]
fn graphics_tablet_double_hold_tip_down_presses_ctrl_alt_space_before_the_contact() {
    let mut r = graphics_tablet_second_press();
    assert_eq!(
        r.feed(pen_down(350, 500.0, 500.0, PRIMARY)),
        [
            CTRL_ALT_SPACE_DOWN[0],
            CTRL_ALT_SPACE_DOWN[1],
            CTRL_ALT_SPACE_DOWN[2],
            "pen IC 500,500 p=32768"
        ]
    );
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.e.view().scale, 1.0);
    assert_eq!(
        r.feed(pen_up(400, 500.0, 500.0, PRIMARY)),
        ["pen I 500,500"]
    );
    let mut want = CTRL_ALT_SPACE_UP.map(String::from).to_vec();
    want.push("pen I 500,500".into());
    assert_eq!(r.feed(release(500, 500.0, 500.0)), want);
    // The double click did not fire either.
    assert_eq!(r.e.view().scale, 1.0);
    assert_eq!(r.feed(release(600, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn graphics_tablet_double_hold_resolves_at_the_hold_time() {
    let mut r = graphics_tablet_second_press();
    assert_eq!(r.e.next_deadline(), Some(ms(600)));
    assert_eq!(r.tick(599), Vec::<String>::new());
    r.e.tick(ms(650), &mut r.out);
    for m in &r.out {
        assert!(matches!(m, Message::Key(k) if k.t_us == ms(600)));
    }
    assert_eq!(
        r.raw().iter().map(show).collect::<Vec<_>>(),
        CTRL_ALT_SPACE_DOWN
    );
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.feed(hover(700, 510.0, 500.0, PRIMARY)), ["pen I 510,500"]);
    let mut want = CTRL_ALT_SPACE_UP.map(String::from).to_vec();
    want.push("pen I 510,500".into());
    assert_eq!(r.feed(release(800, 510.0, 500.0)), want);
    assert_eq!(r.e.view().scale, 1.0);
}

#[test]
fn graphics_tablet_double_hold_resolves_at_the_hover_motion() {
    let mut r = graphics_tablet_second_press();
    assert_eq!(r.feed(hover(310, 520.0, 500.0, PRIMARY)), ["pen I 520,500"]);
    assert_eq!(r.e.next_deadline(), Some(ms(600)));
    assert_eq!(
        r.feed(hover(320, 530.0, 500.0, PRIMARY)),
        [
            CTRL_ALT_SPACE_DOWN[0],
            CTRL_ALT_SPACE_DOWN[1],
            CTRL_ALT_SPACE_DOWN[2],
            "pen I 530,500"
        ]
    );
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn graphics_tablet_quick_double_click_toggles_the_zoom_at_the_release() {
    let mut r = graphics_tablet_second_press();
    assert_eq!(r.feed(release(400, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.view().scale, 2.0);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn graphics_tablet_zoomed_double_hold_motion_pans_and_tip_down_presses_the_keys() {
    let mut r = Rig::profile(Profile::GraphicsTablet);
    r.feed(hover(0, 500.0, 500.0, 0));
    for (t, f) in [(10, true), (20, false), (30, true), (40, false)] {
        r.feed(if f {
            press(t, 500.0, 500.0)
        } else {
            release(t, 500.0, 500.0)
        });
    }
    assert_eq!(r.e.view().scale, 2.0);
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(150, 500.0, 500.0));
    r.feed(press(200, 500.0, 500.0));
    let before = r.e.view();
    let out = r.feed(hover(210, 600.0, 500.0, PRIMARY));
    assert!(keys_of(&out).is_empty(), "{out:?}");
    assert_ne!(r.e.view(), before, "panned");
    let out = r.feed(release(300, 600.0, 500.0));
    assert!(keys_of(&out).is_empty(), "{out:?}");

    // The tip touch presses the keys even when zoomed, like the first press.
    r.feed(press(1000, 600.0, 500.0));
    r.feed(release(1050, 600.0, 500.0));
    r.feed(press(1100, 600.0, 500.0));
    let out = r.feed(pen_down(1150, 600.0, 500.0, PRIMARY));
    assert_eq!(keys_of(&out), CTRL_ALT_SPACE_DOWN);
}

#[test]
fn double_hold_keys_are_released_when_the_pen_leaves_range() {
    let mut r = graphics_tablet_second_press();
    assert_eq!(r.tick(600), CTRL_ALT_SPACE_DOWN);
    r.feed(exit(700, 500.0, 500.0));
    assert_eq!(
        r.tick(740),
        [
            CTRL_ALT_SPACE_UP[0],
            CTRL_ALT_SPACE_UP[1],
            CTRL_ALT_SPACE_UP[2],
            "pen - 500,500"
        ]
    );
    assert_eq!(r.feed(release(800, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn pen_out_before_the_second_press_resolves_presses_nothing() {
    let mut r = graphics_tablet_second_press();
    r.feed(exit(400, 500.0, 500.0));
    let out = r.tick(440);
    assert!(keys_of(&out).is_empty(), "{out:?}");
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn custom_double_hold_eraser_switches_like_the_hold() {
    let map = ButtonMap {
        click: ButtonAction::RightClick,
        double_click: ButtonAction::LocalZoom,
        hold: ButtonAction::None,
        double_hold: ButtonAction::Eraser,
        hold_drag_contact: ButtonAction::None,
        hold_drag_hover: ButtonAction::None,
    };
    let mut r = custom_keys(map, KeyBindings::default());
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    r.feed(press(300, 500.0, 500.0));
    assert_eq!(r.tick(600), ["pen - 500,500", "pen IE 500,500"]);
    assert_eq!(
        r.feed(release(700, 500.0, 500.0)),
        ["pen - 500,500", "pen I 500,500"]
    );
    assert_eq!(r.e.view().scale, 1.0);
}

#[test]
fn custom_double_hold_none_keeps_the_instant_double_click() {
    let map = ButtonMap {
        double_hold: ButtonAction::None,
        ..ButtonMap::for_profile(Profile::GraphicsTablet)
    };
    let mut r = custom_keys(map, KeyBindings::default());
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(release(200, 500.0, 500.0));
    r.feed(press(300, 500.0, 500.0));
    assert_eq!(r.e.view().scale, 2.0, "fired at the press");
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.tick(1000), Vec::<String>::new());
    assert_eq!(r.feed(release(1100, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn double_hold_config_json_and_sanitizing() {
    let c = InputConfig::from_json(r#"{"profile":"graphics_tablet"}"#).unwrap();
    assert_eq!(
        c.keys.double_hold,
        KeyCombo {
            mods: KeyCombo::CTRL | KeyCombo::ALT,
            key: 0x2C
        }
    );
    assert_eq!(c.button_map().double_hold, ButtonAction::Keys);

    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"double_hold":"eraser"},
            "keys":{"double_hold":{"mods":2,"key":4}}}"#,
    )
    .unwrap();
    assert_eq!(c.button_map().double_hold, ButtonAction::Eraser);
    assert_eq!(c.keys.double_hold, KeyCombo { mods: 2, key: 4 });
    let j = serde_json::to_string(&c).unwrap();
    assert_eq!(InputConfig::from_json(&j).unwrap(), c);

    // Only Eraser, Keys and None are valid.
    for a in ["right_click", "local_zoom", "raw", "scroll", "pc_zoom"] {
        let c = InputConfig::from_json(&format!(
            r#"{{"profile":"custom","custom":{{"double_hold":"{a}"}}}}"#
        ))
        .unwrap();
        assert_eq!(c.button_map().double_hold, ButtonAction::None, "{a}");
    }

    // An empty combination turns Keys into None.
    let c = InputConfig::from_json(
        r#"{"profile":"graphics_tablet","keys":{"double_hold":{"mods":0,"key":0}}}"#,
    )
    .unwrap();
    assert_eq!(c.button_map().double_hold, ButtonAction::None);

    // Other profiles have no double hold.
    for p in [
        Profile::Raw,
        Profile::Desktop,
        Profile::Drawing,
        Profile::Custom,
    ] {
        assert_eq!(ButtonMap::for_profile(p).double_hold, ButtonAction::None);
    }
}

// ---- step zoom drag -----------------------------------------------------------

const ZOOM_IN_DOWN: [&str; 2] = CTRL_SPACE_DOWN;
const ZOOM_IN_UP: [&str; 2] = CTRL_SPACE_UP;

/// A Custom rig with `action` on both drags and the barrel pressed at (500,500),
/// the tip touching (output drained).
fn step_zoom_rig(action: ButtonAction) -> Rig {
    let mut r = custom_keys(
        ButtonMap {
            click: ButtonAction::None,
            double_click: ButtonAction::None,
            hold: ButtonAction::None,
            double_hold: ButtonAction::None,
            hold_drag_contact: action,
            hold_drag_hover: action,
        },
        KeyBindings::default(),
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r
}

fn step_zoom_touching() -> Rig {
    let mut r = step_zoom_rig(ButtonAction::StepZoom);
    assert_eq!(
        r.feed(pen_down(110, 500.0, 500.0, PRIMARY)),
        ["pen I 500,500"]
    );
    r
}

#[test]
fn step_zoom_up_gives_one_zoom_in_step_clicking_at_the_anchor() {
    let mut r = step_zoom_touching();
    assert_eq!(
        r.feed(pen_move(120, 520.0, 420.0, PRIMARY)),
        [
            ZOOM_IN_DOWN[0],
            ZOOM_IN_DOWN[1],
            "btn L+ 500,500",
            "btn L- 500,500",
            ZOOM_IN_UP[0],
            ZOOM_IN_UP[1],
            "pen I 520,420"
        ]
    );
    // Lifting after a drag is not a tap, and nothing stays pressed.
    assert_eq!(
        r.feed(pen_up(130, 520.0, 420.0, PRIMARY)),
        ["pen I 520,420"]
    );
}

#[test]
fn step_zoom_down_gives_a_zoom_out_step() {
    let mut r = step_zoom_touching();
    assert_eq!(
        r.feed(pen_move(120, 500.0, 580.0, PRIMARY)),
        [
            CTRL_ALT_SPACE_DOWN[0],
            CTRL_ALT_SPACE_DOWN[1],
            CTRL_ALT_SPACE_DOWN[2],
            "btn L+ 500,500",
            "btn L- 500,500",
            CTRL_ALT_SPACE_UP[0],
            CTRL_ALT_SPACE_UP[1],
            CTRL_ALT_SPACE_UP[2],
            "pen I 500,580"
        ]
    );
}

#[test]
fn step_zoom_keeps_the_leftover_travel() {
    let mut r = step_zoom_touching();
    assert_eq!(
        r.feed(pen_move(120, 500.0, 421.0, PRIMARY)),
        ["pen I 500,421"]
    );
    // 79 px so far; 2 more complete the step. Horizontal travel is ignored.
    let out = r.feed(pen_move(130, 900.0, 419.0, PRIMARY));
    assert_eq!(keys_of(&out), [ZOOM_IN_DOWN, ZOOM_IN_UP].concat());
    // The leftover is 1 px: 79 more are needed for the next step.
    assert_eq!(
        r.feed(pen_move(300, 900.0, 341.0, PRIMARY)),
        ["pen I 900,341"]
    );
    let out = r.feed(pen_move(310, 900.0, 339.0, PRIMARY));
    assert_eq!(keys_of(&out).len(), 4, "{out:?}");
}

#[test]
fn step_zoom_is_rate_limited_and_the_backlog_capped() {
    let mut r = step_zoom_touching();
    // 400 px up in one sample: one step now, the backlog is capped to 3 steps.
    let out = r.feed(pen_move(120, 500.0, 100.0, PRIMARY));
    assert_eq!(keys_of(&out).len(), 4, "{out:?}");
    let steps = |out: &[String]| out.iter().filter(|s| *s == "btn L+ 500,500").count();
    // Within 60 ms: nothing.
    assert_eq!(steps(&r.feed(pen_move(150, 500.0, 99.0, PRIMARY))), 0);
    assert_eq!(steps(&r.feed(pen_move(179, 500.0, 98.0, PRIMARY))), 0);
    // Then one step per 60 ms of pen samples, two more in total (3 were queued).
    assert_eq!(steps(&r.feed(pen_move(180, 500.0, 97.0, PRIMARY))), 1);
    assert_eq!(steps(&r.feed(pen_move(200, 500.0, 96.0, PRIMARY))), 0);
    assert_eq!(steps(&r.feed(pen_move(240, 500.0, 95.0, PRIMARY))), 1);
    // The backlog is empty (the pen moved 1 px per sample): no more steps.
    assert_eq!(steps(&r.feed(pen_move(300, 500.0, 94.0, PRIMARY))), 0);
    assert_eq!(steps(&r.feed(pen_move(400, 500.0, 93.0, PRIMARY))), 0);
}

#[test]
fn step_zoom_contact_drag_never_reports_contact() {
    let mut r = step_zoom_touching();
    for (t, y) in [(120, 450.0), (200, 400.0), (300, 300.0)] {
        let out = r.feed(pen_move(t, 500.0, y, PRIMARY));
        let pen = out.iter().find(|s| s.starts_with("pen ")).unwrap();
        assert!(pen.starts_with("pen I "), "{pen}");
    }
    let out = r.feed(pen_up(400, 500.0, 300.0, PRIMARY));
    assert!(out.iter().all(|s| !s.starts_with("pen IC")), "{out:?}");
}

#[test]
fn step_zoom_hover_drag_zooms_in_the_air() {
    let mut r = step_zoom_rig(ButtonAction::StepZoom);
    let out = r.feed(hover(120, 500.0, 420.0, PRIMARY));
    assert_eq!(
        out,
        [
            ZOOM_IN_DOWN[0],
            ZOOM_IN_DOWN[1],
            "btn L+ 500,500",
            "btn L- 500,500",
            ZOOM_IN_UP[0],
            ZOOM_IN_UP[1],
            "pen I 500,420"
        ]
    );
}

#[test]
fn step_zoom_leaves_no_keys_down_on_release_or_leaving_range() {
    let down = |out: &[String]| {
        let mut held = 0i32;
        for s in out.iter().filter(|s| s.starts_with("key ")) {
            held += if s.ends_with("down") { 1 } else { -1 };
        }
        held
    };
    let mut r = step_zoom_touching();
    let mut all = r.feed(pen_move(120, 500.0, 300.0, PRIMARY));
    all.extend(r.feed(release(130, 500.0, 300.0)));
    assert_eq!(down(&all), 0, "{all:?}");

    let mut r = step_zoom_touching();
    let mut all = r.feed(pen_move(120, 500.0, 300.0, PRIMARY));
    all.extend(r.feed(exit(130, 500.0, 300.0)));
    all.extend(r.tick(200));
    assert_eq!(down(&all), 0, "{all:?}");
    r.e.release_all(ms(300), &mut r.out);
    let out: Vec<String> = r.raw().iter().map(show).collect();
    assert!(keys_of(&out).is_empty(), "{out:?}");
}

#[test]
fn step_zoom_with_one_direction_unset_only_steps_the_other() {
    let mut r = custom_keys(
        ButtonMap {
            hold_drag_contact: ButtonAction::StepZoom,
            ..ButtonMap::for_profile(Profile::Raw)
        },
        KeyBindings {
            zoom_out: KeyCombo::default(),
            ..KeyBindings::default()
        },
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    let out = r.feed(pen_move(120, 500.0, 600.0, PRIMARY));
    assert!(
        keys_of(&out).is_empty() && !out.iter().any(|s| s.starts_with("btn")),
        "{out:?}"
    );
    let out = r.feed(pen_move(200, 500.0, 500.0, PRIMARY));
    assert_eq!(keys_of(&out).len(), 4, "{out:?}");
}

#[test]
fn step_zoom_config_json_and_sanitizing() {
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"hold_drag_contact":"step_zoom","hold_drag_hover":"step_zoom"},
            "keys":{"zoom_in":{"mods":2,"key":4}},"zoom_step_px":120}"#,
    )
    .unwrap();
    let m = c.button_map();
    assert_eq!(m.hold_drag_contact, ButtonAction::StepZoom);
    assert_eq!(m.hold_drag_hover, ButtonAction::StepZoom);
    assert_eq!(c.keys.zoom_in, KeyCombo { mods: 2, key: 4 });
    assert_eq!(
        c.keys.zoom_out,
        KeyCombo {
            mods: KeyCombo::CTRL | KeyCombo::ALT,
            key: 0x2C
        }
    );
    assert_eq!(crate::config::Params::from_config(&c).zoom_step, 120.0);

    // Only valid on the drag triggers.
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"click":"step_zoom","double_click":"step_zoom",
            "hold":"step_zoom","double_hold":"step_zoom"}}"#,
    )
    .unwrap();
    let m = c.button_map();
    assert_eq!(
        [m.click, m.double_click, m.hold, m.double_hold],
        [ButtonAction::None; 4]
    );

    // Both combinations empty: None; one empty: kept.
    let both = |zin: u8, zout: u8| {
        InputConfig::from_json(&format!(
            r#"{{"profile":"custom","custom":{{"hold_drag_hover":"step_zoom"}},
                "keys":{{"zoom_in":{{"mods":{zin},"key":0}},"zoom_out":{{"mods":{zout},"key":0}}}}}}"#
        ))
        .unwrap()
        .button_map()
        .hold_drag_hover
    };
    assert_eq!(both(0, 0), ButtonAction::None);
    assert_eq!(both(1, 0), ButtonAction::StepZoom);
    assert_eq!(both(0, 1), ButtonAction::StepZoom);

    // The step distance is clamped; a non-finite value falls back.
    let step = |v: f32| {
        crate::config::Params::from_config(&InputConfig {
            zoom_step_px: v,
            ..InputConfig::default()
        })
        .zoom_step
    };
    assert_eq!(step(1.0), 20.0);
    assert_eq!(step(9999.0), 400.0);
    assert_eq!(step(f32::NAN), 80.0);
    assert_eq!(step(100.0), 100.0);
    assert_eq!(InputConfig::default().zoom_step_px, 80.0);
}

// ---- wheel zoom drag ------------------------------------------------------------

fn wheel_zoom_rig() -> Rig {
    step_zoom_rig(ButtonAction::WheelZoom)
}

#[test]
fn wheel_zoom_hover_drag_gives_one_notch_per_step() {
    let mut r = wheel_zoom_rig();
    assert_eq!(
        r.feed(hover(120, 500.0, 420.0, PRIMARY)),
        ["scroll 0,120 m0 @500,500", "pen I 500,420"]
    );
    // Down: zoom out. 80 px up then 80 px down gives one notch each way.
    let mut r = wheel_zoom_rig();
    assert_eq!(
        r.feed(hover(120, 500.0, 580.0, PRIMARY)),
        ["scroll 0,-120 m0 @500,500", "pen I 500,580"]
    );
    // Less than a step: nothing, and no keys or clicks ever.
    let mut r = wheel_zoom_rig();
    assert_eq!(r.feed(hover(120, 500.0, 430.0, PRIMARY)), ["pen I 500,430"]);
}

#[test]
fn wheel_zoom_keeps_the_leftover_and_is_rate_limited() {
    let mut r = wheel_zoom_rig();
    assert_eq!(r.feed(hover(120, 500.0, 421.0, PRIMARY)), ["pen I 500,421"]);
    // 79 px so far; 2 more complete the step. Horizontal travel is ignored.
    let out = r.feed(hover(130, 900.0, 419.0, PRIMARY));
    assert_eq!(out[0], "scroll 0,120 m0 @500,500", "{out:?}");
    // The leftover is 1 px: 79 more are needed.
    assert_eq!(r.feed(hover(300, 900.0, 341.0, PRIMARY)), ["pen I 900,341"]);
    assert_eq!(
        r.feed(hover(310, 900.0, 339.0, PRIMARY))[0],
        "scroll 0,120 m0 @500,500"
    );

    // 400 px up at once: one notch now, the backlog is capped to 3 steps.
    let mut r = wheel_zoom_rig();
    let count = |out: &[String]| out.iter().filter(|s| s.starts_with("scroll ")).count();
    assert_eq!(count(&r.feed(hover(120, 500.0, 100.0, PRIMARY))), 1);
    assert_eq!(count(&r.feed(hover(150, 500.0, 99.0, PRIMARY))), 0);
    assert_eq!(count(&r.feed(hover(180, 500.0, 98.0, PRIMARY))), 1);
    assert_eq!(count(&r.feed(hover(240, 500.0, 97.0, PRIMARY))), 1);
    assert_eq!(count(&r.feed(hover(300, 500.0, 96.0, PRIMARY))), 0);
}

#[test]
fn wheel_zoom_contact_drag_forwards_no_contact_and_no_keys() {
    let mut r = wheel_zoom_rig();
    assert_eq!(
        r.feed(pen_down(110, 500.0, 500.0, PRIMARY)),
        ["pen I 500,500"]
    );
    let out = r.feed(pen_move(120, 500.0, 420.0, PRIMARY));
    assert_eq!(out, ["scroll 0,120 m0 @500,500", "pen I 500,420"]);
    let out = r.feed(pen_up(130, 500.0, 420.0, PRIMARY));
    assert_eq!(out, ["pen I 500,420"]);
}

#[test]
fn wheel_zoom_config_json() {
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"hold_drag_contact":"wheel_zoom","hold_drag_hover":"wheel_zoom"}}"#,
    )
    .unwrap();
    let m = c.button_map();
    assert_eq!(m.hold_drag_contact, ButtonAction::WheelZoom);
    assert_eq!(m.hold_drag_hover, ButtonAction::WheelZoom);
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"click":"wheel_zoom","hold":"wheel_zoom"}}"#,
    )
    .unwrap();
    assert_eq!(c.button_map().click, ButtonAction::None);
    assert_eq!(c.button_map().hold, ButtonAction::None);
}

// ---- pan drag -----------------------------------------------------------------------

const SPACE_DOWN: &str = "key 0x2c down";
const SPACE_UP: &str = "key 0x2c up";

/// A Custom rig with Pan on the contact drag and the barrel pressed at (500,500).
fn pan_rig(hold: ButtonAction, hover_drag: ButtonAction) -> Rig {
    let mut r = custom_keys(
        ButtonMap {
            click: ButtonAction::None,
            double_click: ButtonAction::None,
            hold,
            double_hold: ButtonAction::None,
            hold_drag_contact: ButtonAction::Pan,
            hold_drag_hover: hover_drag,
        },
        KeyBindings::default(),
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r
}

#[test]
fn pan_presses_space_before_the_contact_and_releases_after_the_lift() {
    let mut r = pan_rig(ButtonAction::None, ButtonAction::None);
    assert_eq!(
        r.feed(pen_down(110, 500.0, 500.0, PRIMARY)),
        [SPACE_DOWN, "pen IC 500,500 p=32768"]
    );
    // The drag carries the contact.
    let out = r.feed(pen_move(120, 540.0, 500.0, PRIMARY));
    assert_eq!(out, ["pen IC 540,500 p=32768"]);
    assert_eq!(
        r.feed(pen_up(130, 540.0, 500.0, PRIMARY)),
        ["pen I 540,500", SPACE_UP]
    );
    // The barrel is still held: the next touch pans again.
    assert_eq!(
        r.feed(pen_down(200, 540.0, 500.0, PRIMARY)),
        [SPACE_DOWN, "pen IC 540,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_up(210, 540.0, 500.0, PRIMARY)),
        ["pen I 540,500", SPACE_UP]
    );
    assert!(
        r.feed(release(300, 540.0, 500.0))
            .iter()
            .all(|s| !s.starts_with("key "))
    );
}

#[test]
fn pan_starts_at_the_tip_touch_after_the_hold_time_and_over_a_hover_drag() {
    // A deferred hold (right click) does not delay the pan.
    let mut r = pan_rig(ButtonAction::RightClick, ButtonAction::None);
    assert!(r.tick(400).is_empty());
    assert_eq!(r.feed(pen_down(410, 500.0, 500.0, PRIMARY))[0], SPACE_DOWN);
    // A hover drag in progress hands over to the pan.
    let mut r = pan_rig(ButtonAction::None, ButtonAction::WheelZoom);
    r.feed(hover(110, 500.0, 420.0, PRIMARY));
    let out = r.feed(pen_down(120, 500.0, 420.0, PRIMARY));
    assert_eq!(out, [SPACE_DOWN, "pen IC 500,420 p=32768"]);
}

#[test]
fn pan_keeps_the_keys_when_the_barrel_is_released_mid_touch() {
    let mut r = pan_rig(ButtonAction::None, ButtonAction::None);
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    let out = r.feed(release(120, 520.0, 500.0));
    assert!(out.iter().all(|s| !s.starts_with("key ")), "{out:?}");
    assert_eq!(
        r.feed(pen_up(130, 520.0, 500.0, 0)),
        ["pen I 520,500", SPACE_UP]
    );
    // Nothing is released twice.
    assert!(keys_of(&r.feed(exit(200, 520.0, 500.0))).is_empty());
}

#[test]
fn pan_releases_the_keys_on_cancel_and_leaving_range() {
    let mut r = pan_rig(ButtonAction::None, ButtonAction::None);
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    let out = r.feed(pen_at(ACTION_CANCEL, 120, 500.0, 500.0, PRIMARY));
    assert_eq!(keys_of(&out), [SPACE_UP]);

    let mut r = pan_rig(ButtonAction::None, ButtonAction::None);
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    let mut all = r.feed(exit(120, 500.0, 500.0));
    all.extend(r.tick(200));
    assert_eq!(keys_of(&all), [SPACE_UP]);

    let mut r = pan_rig(ButtonAction::None, ButtonAction::None);
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    let mut out = Vec::new();
    r.e.release_all(ms(130), &mut out);
    let out: Vec<String> = out.iter().map(show).collect();
    assert_eq!(keys_of(&out), [SPACE_UP]);
}

#[test]
fn pan_yields_to_a_keys_hold() {
    let mut r = pan_rig(ButtonAction::Keys, ButtonAction::None);
    // Hold keys (Ctrl+Space) are pressed at the touch, not the pan combination.
    let out = r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    assert_eq!(keys_of(&out), CTRL_SPACE_DOWN);
}

#[test]
fn pan_config_json_and_sanitizing() {
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"hold_drag_contact":"pan","hold_drag_hover":"pan"}}"#,
    )
    .unwrap();
    assert_eq!(c.keys.pan, KeyCombo { mods: 0, key: 0x2C });
    let m = c.button_map();
    assert_eq!(m.hold_drag_contact, ButtonAction::Pan);
    assert_eq!(m.hold_drag_hover, ButtonAction::None);

    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"click":"pan","hold":"pan","double_hold":"pan"},
            "keys":{"pan":{"mods":1,"key":4}}}"#,
    )
    .unwrap();
    assert_eq!(c.keys.pan, KeyCombo { mods: 1, key: 4 });
    let m = c.button_map();
    assert_eq!([m.click, m.hold, m.double_hold], [ButtonAction::None; 3]);

    // An empty combination disables the pan.
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"hold_drag_contact":"pan"},"keys":{"pan":{"mods":0,"key":0}}}"#,
    )
    .unwrap();
    assert_eq!(c.button_map().hold_drag_contact, ButtonAction::None);
}

// ---- pan / zoom drag ----------------------------------------------------------------

/// A Custom rig with `contact` on the contact drag, the barrel pressed in the air at (500,500).
fn pz_rig(contact: ButtonAction) -> Rig {
    let mut r = custom_keys(
        ButtonMap {
            click: ButtonAction::None,
            double_click: ButtonAction::None,
            hold: ButtonAction::None,
            double_hold: ButtonAction::None,
            hold_drag_contact: contact,
            hold_drag_hover: ButtonAction::None,
        },
        KeyBindings::default(),
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r
}

fn pz_touching() -> Rig {
    let mut r = pz_rig(ButtonAction::PanZoom);
    assert_eq!(
        r.feed(pen_down(110, 500.0, 500.0, PRIMARY)),
        ["pen I 500,500"]
    );
    r
}

#[test]
fn pan_zoom_vertical_drag_zooms_without_contact_or_keys() {
    let mut r = pz_touching();
    // Under the slop: nothing but hover.
    assert_eq!(
        r.feed(pen_move(120, 500.0, 490.0, PRIMARY)),
        ["pen I 500,490"]
    );
    // Past the slop (24 px) and 80 px of travel counted from the anchor.
    assert_eq!(
        r.feed(pen_move(130, 510.0, 420.0, PRIMARY)),
        ["scroll 0,120 m0 @500,500", "pen I 510,420"]
    );
    // Locked: a sideways move keeps zooming (down = zoom out), still no contact.
    let out = r.feed(pen_move(200, 900.0, 500.0, PRIMARY));
    assert_eq!(out, ["scroll 0,-120 m0 @500,500", "pen I 900,500"]);
    let out = r.feed(pen_move(300, 100.0, 500.0, PRIMARY));
    assert_eq!(out, ["pen I 100,500"]);
    assert_eq!(
        r.feed(pen_up(310, 100.0, 500.0, PRIMARY)),
        ["pen I 100,500"]
    );
}

#[test]
fn pan_zoom_first_step_counts_the_slop_distance() {
    let mut r = pz_touching();
    // 40 px up in one sample: past the slop but under one step: nothing yet.
    assert_eq!(
        r.feed(pen_move(120, 500.0, 460.0, PRIMARY)),
        ["pen I 500,460"]
    );
    // 40 px more: the step is due.
    assert_eq!(
        r.feed(pen_move(130, 500.0, 420.0, PRIMARY))[0],
        "scroll 0,120 m0 @500,500"
    );
}

#[test]
fn pan_zoom_other_drags_pan_from_the_anchor_and_stay_locked() {
    for (x, y) in [(560.0, 500.0), (560.0, 530.0)] {
        let mut r = pz_touching();
        assert_eq!(
            r.feed(pen_move(120, x, y, PRIMARY)),
            [
                SPACE_DOWN,
                "pen IC 500,500 p=32768",
                &format!("pen IC {x},{y} p=32768"),
            ]
        );
        // Locked: a vertical move now keeps panning.
        assert_eq!(
            r.feed(pen_move(200, x, y - 300.0, PRIMARY)),
            [format!("pen IC {x},{} p=32768", y - 300.0)]
        );
        assert_eq!(
            r.feed(pen_up(210, x, y - 300.0, PRIMARY)),
            [format!("pen I {x},{}", y - 300.0), SPACE_UP.to_string()]
        );
    }
}

#[test]
fn pan_zoom_tap_under_the_slop_does_nothing_and_the_next_touch_reclassifies() {
    let mut r = pz_touching();
    assert_eq!(
        r.feed(pen_move(120, 510.0, 505.0, PRIMARY)),
        ["pen I 510,505"]
    );
    assert_eq!(
        r.feed(pen_up(130, 510.0, 505.0, PRIMARY)),
        ["pen I 510,505"]
    );
    r.feed(pen_down(200, 510.0, 505.0, PRIMARY));
    let out = r.feed(pen_move(210, 600.0, 505.0, PRIMARY));
    assert_eq!(out[0], SPACE_DOWN, "{out:?}");
    assert_eq!(out[1], "pen IC 510,505 p=32768");
}

#[test]
fn pan_zoom_barrel_released_mid_touch_keeps_the_mode_until_the_lift() {
    // Pan: the keys stay until the lift and the contact keeps flowing.
    let mut r = pz_touching();
    r.feed(pen_move(120, 600.0, 500.0, PRIMARY));
    let out = r.feed(release(130, 610.0, 500.0));
    assert!(keys_of(&out).is_empty(), "{out:?}");
    assert_eq!(
        r.feed(pen_move(140, 620.0, 500.0, 0)),
        ["pen IC 620,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_up(150, 620.0, 500.0, 0)),
        ["pen I 620,500", SPACE_UP]
    );
    // Before the mode was picked: no contact until the lift, no keys.
    let mut r = pz_touching();
    r.feed(release(120, 500.0, 500.0));
    let out = r.feed(pen_move(130, 700.0, 500.0, 0));
    assert_eq!(out, ["pen I 700,500"]);
    assert_eq!(r.feed(pen_up(140, 700.0, 500.0, 0)), ["pen I 700,500"]);
}

#[test]
fn pan_zoom_releases_the_keys_on_cancel_pen_out_and_release_all() {
    let panning = || {
        let mut r = pz_touching();
        r.feed(pen_move(120, 600.0, 500.0, PRIMARY));
        r
    };
    let mut r = panning();
    let out = r.feed(pen_at(ACTION_CANCEL, 130, 600.0, 500.0, PRIMARY));
    assert_eq!(keys_of(&out), [SPACE_UP]);
    let mut r = panning();
    let mut all = r.feed(exit(130, 600.0, 500.0));
    all.extend(r.tick(200));
    assert_eq!(keys_of(&all), [SPACE_UP]);
    let mut r = panning();
    let mut out = Vec::new();
    r.e.release_all(ms(130), &mut out);
    let out: Vec<String> = out.iter().map(show).collect();
    assert_eq!(keys_of(&out), [SPACE_UP]);
}

#[test]
fn pan_zoom_with_an_empty_pan_combo_still_zooms_and_pans_without_keys() {
    let mut r = custom_keys(
        ButtonMap {
            hold_drag_contact: ButtonAction::PanZoom,
            ..ButtonMap::for_profile(Profile::Raw)
        },
        KeyBindings {
            pan: KeyCombo::default(),
            ..KeyBindings::default()
        },
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    let out = r.feed(pen_move(120, 600.0, 500.0, PRIMARY));
    assert_eq!(out, ["pen IC 500,500 p=32768", "pen IC 600,500 p=32768"]);
}

#[test]
fn pan_zoom_yields_to_a_keys_hold() {
    let mut r = custom_keys(
        ButtonMap {
            hold: ButtonAction::Keys,
            hold_drag_contact: ButtonAction::PanZoom,
            ..ButtonMap::for_profile(Profile::Raw)
        },
        KeyBindings::default(),
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    let out = r.feed(pen_down(110, 500.0, 500.0, PRIMARY));
    assert_eq!(keys_of(&out), CTRL_SPACE_DOWN);
}

/// A rig with the tip already down (drawing) at (500,500) and the barrel not pressed.
fn touching_first(contact: ButtonAction) -> Rig {
    let mut r = custom_keys(
        ButtonMap {
            hold_drag_contact: contact,
            ..ButtonMap::for_profile(Profile::Raw)
        },
        KeyBindings::default(),
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    assert_eq!(
        r.feed(pen_down(50, 500.0, 500.0, 0)),
        ["pen IC 500,500 p=32768"]
    );
    r
}

#[test]
fn barrel_while_touching_starts_pan_zoom_with_a_lift_sample() {
    let mut r = touching_first(ButtonAction::PanZoom);
    // The stroke ends (lift sample), then the drag classifies as usual.
    assert_eq!(r.feed(press(100, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(
        r.feed(pen_move(110, 540.0, 420.0, PRIMARY)),
        ["scroll 0,120 m0 @500,500", "pen I 540,420"]
    );
    // The real lift sends no key or release, just hover.
    assert_eq!(
        r.feed(pen_up(120, 540.0, 420.0, PRIMARY)),
        ["pen I 540,420"]
    );

    let mut r = touching_first(ButtonAction::PanZoom);
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(
        r.feed(pen_move(110, 600.0, 500.0, PRIMARY)),
        [
            SPACE_DOWN,
            "pen IC 500,500 p=32768",
            "pen IC 600,500 p=32768"
        ]
    );
    assert_eq!(
        r.feed(pen_up(120, 600.0, 500.0, PRIMARY)),
        ["pen I 600,500", SPACE_UP]
    );
}

#[test]
fn barrel_while_touching_starts_pan_with_the_keys_between_lift_and_contact() {
    let mut r = touching_first(ButtonAction::Pan);
    assert_eq!(
        r.feed(press(100, 520.0, 500.0)),
        ["pen I 520,500", SPACE_DOWN, "pen IC 520,500"]
    );
    assert_eq!(
        r.feed(pen_move(110, 560.0, 500.0, PRIMARY)),
        ["pen IC 560,500 p=32768"]
    );
    // Barrel released before the lift: the keys stay until the lift.
    let out = r.feed(release(120, 560.0, 500.0));
    assert!(keys_of(&out).is_empty(), "{out:?}");
    assert_eq!(
        r.feed(pen_up(130, 560.0, 500.0, 0)),
        ["pen I 560,500", SPACE_UP]
    );
}

#[test]
fn barrel_while_touching_pan_releases_the_keys_on_cancel() {
    let mut r = touching_first(ButtonAction::Pan);
    r.feed(press(100, 520.0, 500.0));
    let out = r.feed(pen_at(ACTION_CANCEL, 110, 520.0, 500.0, PRIMARY));
    assert_eq!(keys_of(&out), [SPACE_UP]);
}

#[test]
fn barrel_while_touching_stays_raw_for_other_configs() {
    for contact in [
        ButtonAction::Raw,
        ButtonAction::Scroll,
        ButtonAction::WheelZoom,
    ] {
        let mut r = touching_first(contact);
        let out = r.feed(press(100, 500.0, 500.0));
        assert_eq!(out, ["pen ICB 500,500"], "{contact:?}");
    }
}

#[test]
fn pan_zoom_config_json_and_sanitizing() {
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"hold_drag_contact":"pan_zoom","hold_drag_hover":"pan_zoom"}}"#,
    )
    .unwrap();
    let m = c.button_map();
    assert_eq!(m.hold_drag_contact, ButtonAction::PanZoom);
    assert_eq!(m.hold_drag_hover, ButtonAction::None);
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"click":"pan_zoom","hold":"pan_zoom","double_hold":"pan_zoom"}}"#,
    )
    .unwrap();
    let m = c.button_map();
    assert_eq!([m.click, m.hold, m.double_hold], [ButtonAction::None; 3]);
    let c = InputConfig::from_json(
        r#"{"profile":"custom","custom":{"hold_drag_contact":"pan_zoom"},"keys":{"pan":{"mods":0,"key":0}}}"#,
    )
    .unwrap();
    assert_eq!(c.button_map().hold_drag_contact, ButtonAction::PanZoom);
}

// ---- 9. drags win over the hold ----------------------------------------------

fn custom_map(hold: ButtonAction, contact: ButtonAction, hover: ButtonAction) -> Rig {
    custom_keys(
        ButtonMap {
            click: ButtonAction::Raw,
            double_click: ButtonAction::LocalZoom,
            hold,
            double_hold: ButtonAction::None,
            hold_drag_contact: contact,
            hold_drag_hover: hover,
        },
        KeyBindings::default(),
    )
}

/// Press at 500,500 and let the hold time elapse without moving.
fn pressed_past_the_hold(mut r: Rig) -> Rig {
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(r.tick(400), Vec::<String>::new());
    assert_eq!(r.e.next_deadline(), None);
    r
}

#[test]
fn hover_zoom_works_after_a_still_hold_with_a_raw_hold() {
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Raw,
        ButtonAction::Raw,
        ButtonAction::PcZoom,
    ));
    assert_eq!(
        r.feed(hover(500, 500.0, 452.0, PRIMARY)),
        ["scroll 0,120 m1 @500,500", "pen I 500,452"]
    );
    assert_eq!(r.feed(release(600, 500.0, 452.0)), ["pen I 500,452"]);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn tip_down_during_a_hover_zoom_does_not_draw() {
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Raw,
        ButtonAction::Raw,
        ButtonAction::PcZoom,
    ));
    r.feed(hover(500, 500.0, 452.0, PRIMARY));
    // Contact is reported as hover and the zoom goes on.
    assert_eq!(
        r.feed(pen_down(510, 500.0, 452.0, PRIMARY)),
        ["pen I 500,452"]
    );
    assert_eq!(
        r.feed(pen_move(520, 500.0, 404.0, PRIMARY)),
        ["scroll 0,120 m1 @500,500", "pen I 500,404"]
    );
    assert_eq!(
        r.feed(pen_up(530, 500.0, 404.0, PRIMARY)),
        ["pen I 500,404"]
    );
    assert_eq!(r.feed(release(540, 500.0, 404.0)), ["pen I 500,404"]);
}

#[test]
fn tip_down_during_a_hover_drag_becomes_the_contact_drag() {
    let mut r = custom_map(
        ButtonAction::Raw,
        ButtonAction::Scroll,
        ButtonAction::PcZoom,
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(hover(110, 500.0, 452.0, PRIMARY));
    r.feed(pen_down(120, 500.0, 452.0, PRIMARY));
    // Scrolling (not zooming) from now on, anchored where the press happened.
    assert_eq!(
        r.feed(pen_move(130, 500.0, 500.0, PRIMARY)),
        ["scroll 0,120 m0 @500,500", "pen I 500,500"]
    );
    assert_eq!(
        r.feed(pen_up(140, 500.0, 500.0, PRIMARY)),
        ["pen I 500,500"]
    );
    assert_eq!(r.feed(release(150, 500.0, 500.0)), ["pen I 500,500"]);
}

#[test]
fn tap_during_a_hover_drag_converted_to_a_contact_drag_clicks() {
    let mut r = custom_keys(
        ButtonMap {
            click: ButtonAction::RightClick,
            double_click: ButtonAction::None,
            hold: ButtonAction::Raw,
            double_hold: ButtonAction::None,
            hold_drag_contact: ButtonAction::Scroll,
            hold_drag_hover: ButtonAction::PcZoom,
        },
        KeyBindings::default(),
    );
    // Same as an ordinary contact drag: a tap without drag fires the click.
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    r.feed(hover(110, 500.0, 452.0, PRIMARY));
    r.feed(pen_down(120, 500.0, 452.0, PRIMARY));
    assert_eq!(
        r.feed(pen_up(130, 500.0, 452.0, PRIMARY)),
        ["pen I 500,452", "btn R+ 500,452", "btn R- 500,452"]
    );
}

#[test]
fn contact_zoom_works_after_a_still_hold_with_a_raw_hold() {
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Raw,
        ButtonAction::PcZoom,
        ButtonAction::Raw,
    ));
    assert_eq!(
        r.feed(pen_down(500, 500.0, 500.0, PRIMARY)),
        ["pen I 500,500"]
    );
    assert_eq!(
        r.feed(pen_move(510, 500.0, 452.0, PRIMARY)),
        ["scroll 0,120 m1 @500,500", "pen I 500,452"]
    );
    assert_eq!(
        r.feed(pen_up(520, 500.0, 452.0, PRIMARY)),
        ["pen I 500,452"]
    );
    assert_eq!(r.feed(release(530, 500.0, 452.0)), ["pen I 500,452"]);
}

#[test]
fn still_hold_released_without_a_drag_fires_the_raw_hold_once() {
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Raw,
        ButtonAction::Raw,
        ButtonAction::PcZoom,
    ));
    assert_eq!(
        r.feed(release(600, 500.0, 500.0)),
        ["pen IB 500,500", "pen I 500,500"]
    );
    assert_eq!(r.e.next_deadline(), None);
    assert_eq!(r.tick(2000), Vec::<String>::new());
}

#[test]
fn still_hold_released_without_a_drag_fires_one_shot_holds() {
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::RightClick,
        ButtonAction::Raw,
        ButtonAction::PcZoom,
    ));
    assert_eq!(
        r.feed(release(600, 500.0, 500.0)),
        ["btn R+ 500,500", "btn R- 500,500", "pen I 500,500"]
    );
    // The eraser has nothing to do without a tip touch or motion.
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Eraser,
        ButtonAction::Raw,
        ButtonAction::PcZoom,
    ));
    assert_eq!(r.feed(release(600, 500.0, 500.0)), ["pen I 500,500"]);
    assert_eq!(r.e.next_deadline(), None);
}

#[test]
fn hold_applies_at_the_tip_touch_when_only_the_hover_drag_is_set() {
    // Eraser hold, hover zoom, raw contact: the touch resolves the eraser first.
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Eraser,
        ButtonAction::Raw,
        ButtonAction::PcZoom,
    ));
    assert_eq!(
        r.feed(pen_down(500, 500.0, 500.0, PRIMARY)),
        ["pen - 500,500", "pen IE 500,500", "pen ICE 500,500 p=32768"]
    );
    assert_eq!(
        r.feed(pen_up(510, 500.0, 500.0, PRIMARY)),
        ["pen IE 500,500"]
    );
    assert_eq!(
        r.feed(release(520, 500.0, 500.0)),
        ["pen - 500,500", "pen I 500,500"]
    );

    // Raw hold: the barrel is held during the contact.
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Raw,
        ButtonAction::Raw,
        ButtonAction::PcZoom,
    ));
    assert_eq!(
        r.feed(pen_down(500, 500.0, 500.0, PRIMARY)),
        ["pen IB 500,500", "pen ICB 500,500 p=32768"]
    );
}

#[test]
fn hold_applies_at_the_hover_motion_when_only_the_contact_drag_is_set() {
    let mut r = pressed_past_the_hold(custom_map(
        ButtonAction::Eraser,
        ButtonAction::PcZoom,
        ButtonAction::Raw,
    ));
    assert_eq!(
        r.feed(hover(500, 600.0, 500.0, PRIMARY)),
        ["pen - 600,500", "pen IE 600,500"]
    );
    // The eraser holds: no zoom, the tip touch draws with the eraser.
    assert_eq!(
        r.feed(pen_down(510, 600.0, 500.0, PRIMARY)),
        ["pen ICE 600,500 p=32768"]
    );
}

#[test]
fn drags_before_the_hold_time_keep_their_mapping_with_a_raw_hold() {
    let mut r = custom_map(
        ButtonAction::Raw,
        ButtonAction::PcZoom,
        ButtonAction::Scroll,
    );
    r.feed(hover(0, 500.0, 500.0, 0));
    r.feed(press(100, 500.0, 500.0));
    assert_eq!(
        r.feed(hover(110, 500.0, 548.0, PRIMARY)),
        ["scroll 0,120 m0 @500,500", "pen I 500,548"]
    );
}
