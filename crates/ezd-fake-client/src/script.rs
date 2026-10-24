//! Scripted input for the fake client: pen strokes, taps, two-finger drags, scrolling and keys.

use ezd_client_core::{ClientHandle, now_us};
use ezd_proto::{
    Key, Message, NORM_MAX, PenSample, PointerButton, Scroll, TouchContact, TouchFrame,
};
use std::time::Duration;

/// A canned input sequence the fake client plays once per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputScript {
    /// One finger tap in the middle.
    Tap,
    /// Pen hover, then a horizontal stroke with pressure and tilt ramps.
    Drag,
    /// Pen stroke along a circle with varying pressure and rotating tilt.
    Circle,
    /// Two fingers moving apart.
    Touch2,
    /// Right click followed by wheel scrolling and Ctrl+wheel zoom.
    Scroll,
    /// Pen hover, Ctrl+Space held (the graphics-tablet-style pen-button shortcut) while the pen draws a
    /// short stroke, then released.
    Keys,
}

fn norm(f: f64) -> u16 {
    (f * f64::from(NORM_MAX))
        .round()
        .clamp(0.0, f64::from(NORM_MAX)) as u16
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn ms(v: f64) -> Duration {
    Duration::from_secs_f64(v / 1000.0)
}

#[allow(clippy::too_many_arguments)]
fn pen(x: f64, y: f64, pressure: f64, tilt_x: f64, tilt_y: f64, flags: u8) -> Message {
    Message::Pen(PenSample {
        t_us: 0,
        x: norm(x),
        y: norm(y),
        pressure: norm(pressure),
        tilt_x: tilt_x as i16,
        tilt_y: tilt_y as i16,
        flags,
    })
}

fn touch(contacts: &[(u8, f64, f64, u8)]) -> Message {
    Message::Touch(TouchFrame {
        t_us: 0,
        contacts: contacts
            .iter()
            .map(|&(id, x, y, phase)| TouchContact {
                id,
                x: norm(x),
                y: norm(y),
                phase,
            })
            .collect(),
    })
}

/// The messages of `script` with their offsets from the start, `t_us` zeroed (the runner stamps
/// it with [`now_us`] when sending).
pub fn script_messages(script: InputScript) -> Vec<(Duration, Message)> {
    const HOVER: u8 = PenSample::IN_RANGE;
    const DRAW: u8 = PenSample::IN_RANGE | PenSample::CONTACT;
    let hz240 = 1000.0 / 240.0;
    let mut v: Vec<(Duration, Message)> = Vec::new();
    match script {
        InputScript::Tap => {
            v.push((ms(0.0), touch(&[(0, 0.5, 0.5, TouchContact::DOWN)])));
            v.push((ms(16.0), touch(&[(0, 0.5, 0.5, TouchContact::MOVE)])));
            v.push((ms(32.0), touch(&[(0, 0.5, 0.5, TouchContact::MOVE)])));
            v.push((ms(48.0), touch(&[(0, 0.5, 0.5, TouchContact::UP)])));
        }
        InputScript::Drag => {
            for i in 0..5 {
                v.push((
                    ms(f64::from(i) * hz240),
                    pen(0.3, 0.5, 0.0, 0.0, 0.0, HOVER),
                ));
            }
            let t0 = 5.0 * hz240;
            let n = 120;
            for i in 0..=n {
                let k = f64::from(i) / f64::from(n);
                v.push((
                    ms(t0 + f64::from(i) * hz240),
                    pen(
                        lerp(0.3, 0.7, k),
                        0.5,
                        lerp(0.2, 0.8, k),
                        lerp(0.0, 3000.0, k),
                        0.0,
                        DRAW,
                    ),
                ));
            }
            let t1 = t0 + f64::from(n + 1) * hz240;
            v.push((ms(t1), pen(0.7, 0.5, 0.0, 3000.0, 0.0, HOVER)));
            v.push((ms(t1 + hz240), pen(0.7, 0.5, 0.0, 0.0, 0.0, 0)));
        }
        InputScript::Circle => {
            let point = |k: f64| {
                let a = k * std::f64::consts::TAU;
                (0.5 + 0.2 * a.cos(), 0.5 + 0.2 * a.sin(), a)
            };
            let (x, y, _) = point(0.0);
            for i in 0..5 {
                v.push((ms(f64::from(i) * hz240), pen(x, y, 0.0, 0.0, 0.0, HOVER)));
            }
            let t0 = 5.0 * hz240;
            let n = 240;
            for i in 0..=n {
                let k = f64::from(i) / f64::from(n);
                let (x, y, a) = point(k);
                v.push((
                    ms(t0 + f64::from(i) * hz240),
                    pen(
                        x,
                        y,
                        0.5 + 0.3 * (a * 3.0).sin(),
                        4500.0 * a.cos(),
                        4500.0 * a.sin(),
                        DRAW,
                    ),
                ));
            }
            let t1 = t0 + f64::from(n + 1) * hz240;
            let (x, y, _) = point(1.0);
            v.push((ms(t1), pen(x, y, 0.0, 0.0, 0.0, HOVER)));
            v.push((ms(t1 + hz240), pen(x, y, 0.0, 0.0, 0.0, 0)));
        }
        InputScript::Touch2 => {
            let (d, m) = (TouchContact::DOWN, TouchContact::MOVE);
            v.push((ms(0.0), touch(&[(0, 0.4, 0.5, d), (1, 0.6, 0.5, d)])));
            let step = 1000.0 / 120.0;
            let n = 48;
            for i in 1..=n {
                let k = f64::from(i) / f64::from(n);
                v.push((
                    ms(f64::from(i) * step),
                    touch(&[
                        (0, lerp(0.4, 0.3, k), 0.5, m),
                        (1, lerp(0.6, 0.7, k), 0.5, m),
                    ]),
                ));
            }
            let u = TouchContact::UP;
            v.push((
                ms(f64::from(n + 1) * step),
                touch(&[(0, 0.3, 0.5, u), (1, 0.7, 0.5, u)]),
            ));
        }
        InputScript::Scroll => {
            let (x, y) = (norm(0.5), norm(0.5));
            let button = |pressed| {
                Message::PointerButton(PointerButton {
                    t_us: 0,
                    x,
                    y,
                    button: PointerButton::RIGHT,
                    pressed,
                })
            };
            let scroll = |dy, modifiers| {
                Message::Scroll(Scroll {
                    t_us: 0,
                    x,
                    y,
                    dx: 0,
                    dy,
                    modifiers,
                })
            };
            v.push((ms(0.0), button(true)));
            v.push((ms(50.0), button(false)));
            for i in 0..5 {
                v.push((ms(100.0 + 50.0 * f64::from(i)), scroll(120, 0)));
            }
            for i in 0..3 {
                v.push((ms(350.0 + 50.0 * f64::from(i)), scroll(-120, Scroll::CTRL)));
            }
        }
        InputScript::Keys => {
            let key = |usage, pressed| {
                Message::Key(Key {
                    t_us: 0,
                    usage,
                    pressed,
                })
            };
            for i in 0..5 {
                v.push((
                    ms(f64::from(i) * hz240),
                    pen(0.4, 0.5, 0.0, 0.0, 0.0, HOVER),
                ));
            }
            v.push((ms(25.0), key(Key::LEFT_CTRL, true)));
            v.push((ms(30.0), key(Key::SPACE, true)));
            // Short pause so the shortcut is active before the pen touches down.
            let t0 = 80.0;
            let step = 1000.0 / 60.0;
            let n = 18; // ~300 ms
            for i in 0..=n {
                let k = f64::from(i) / f64::from(n);
                v.push((
                    ms(t0 + f64::from(i) * step),
                    pen(lerp(0.4, 0.6, k), 0.5, lerp(0.2, 0.8, k), 0.0, 0.0, DRAW),
                ));
            }
            let t1 = t0 + f64::from(n + 1) * step;
            v.push((ms(t1), pen(0.6, 0.5, 0.0, 0.0, 0.0, HOVER)));
            v.push((ms(t1 + 10.0), key(Key::SPACE, false)));
            v.push((ms(t1 + 15.0), key(Key::LEFT_CTRL, false)));
            v.push((ms(t1 + 25.0), pen(0.6, 0.5, 0.0, 0.0, 0.0, 0)));
        }
    }
    v
}

fn stamp(msg: &mut Message, t: u64) {
    match msg {
        Message::Pen(m) => m.t_us = t,
        Message::Touch(m) => m.t_us = t,
        Message::PointerButton(m) => m.t_us = t,
        Message::Scroll(m) => m.t_us = t,
        Message::Key(m) => m.t_us = t,
        _ => {}
    }
}

/// Plays `script` through `handle` in real time. Returns the number of messages sent.
pub async fn play_script(script: InputScript, handle: &ClientHandle) -> u64 {
    let start = tokio::time::Instant::now();
    let mut sent = 0;
    for (at, mut msg) in script_messages(script) {
        tokio::time::sleep_until(start + at).await;
        stamp(&mut msg, now_us());
        if handle.send_input(&msg) {
            sent += 1;
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_script_wraps_the_stroke_in_ctrl_space() {
        let v = script_messages(InputScript::Keys);
        assert!(v.windows(2).all(|w| w[0].0 <= w[1].0), "offsets ascend");
        let pos = |f: &dyn Fn(&Message) -> bool| v.iter().position(|(_, m)| f(m)).unwrap();
        let key = |usage: u16, pressed: bool| move |m: &Message| matches!(m, Message::Key(k) if k.usage == usage && k.pressed == pressed);
        let ctrl_down = pos(&key(Key::LEFT_CTRL, true));
        let space_down = pos(&key(Key::SPACE, true));
        let space_up = pos(&key(Key::SPACE, false));
        let ctrl_up = pos(&key(Key::LEFT_CTRL, false));
        let first_contact =
            pos(&|m| matches!(m, Message::Pen(p) if p.flags & PenSample::CONTACT != 0));
        let last_contact = v
            .iter()
            .rposition(|(_, m)| matches!(m, Message::Pen(p) if p.flags & PenSample::CONTACT != 0))
            .unwrap();
        assert!(ctrl_down < space_down && space_down < first_contact);
        assert!(last_contact < space_up && space_up < ctrl_up);
        // A pause between the shortcut and the first contact sample.
        assert!(v[first_contact].0 - v[space_down].0 >= Duration::from_millis(40));
        assert!(matches!(&v.last().unwrap().1, Message::Pen(p) if p.flags == 0));
    }
}
