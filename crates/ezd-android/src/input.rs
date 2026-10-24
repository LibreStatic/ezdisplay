//! Unpacking of the flat arrays Kotlin sends for one `MotionEvent`, and clock conversion.
//!
//! Layout (see `NativeBridge.inputEvent`):
//!
//! * `ints`: `[actionMasked, actionIndex, buttonState, flags, pointerCount, historySize]`, then
//!   per pointer `[pointerId, toolType]`.
//! * `floats`: `[vscroll, hscroll]`, then for every step (history oldest first, then the
//!   current one) and every pointer `[x, y, pressure, tilt, orientation]`.
//! * `times_ns`: one `CLOCK_MONOTONIC` event time per step.

use ezd_input::{Motion, MotionStep, PointerSample};

const INT_HEADER: usize = 6;
const INTS_PER_POINTER: usize = 2;
const FLOAT_HEADER: usize = 2;
const FLOATS_PER_SAMPLE: usize = 5;
const MAX_POINTERS: usize = 32;
const MAX_HISTORY: usize = 1024;

/// Builds a [`Motion`] from the packed arrays. `to_us` converts one event time
/// (monotonic nanoseconds) to the client microsecond clock.
pub fn unpack_motion(
    ints: &[i32],
    floats: &[f32],
    times_ns: &[i64],
    to_us: impl Fn(i64) -> u64,
) -> Result<Motion, String> {
    if ints.len() < INT_HEADER {
        return Err(format!("ints too short: {}", ints.len()));
    }
    let pointers = usize::try_from(ints[4]).unwrap_or(0);
    let history = usize::try_from(ints[5]).map_err(|_| format!("bad historySize {}", ints[5]))?;
    if !(1..=MAX_POINTERS).contains(&pointers) {
        return Err(format!("bad pointerCount {}", ints[4]));
    }
    if history > MAX_HISTORY {
        return Err(format!("bad historySize {history}"));
    }
    let action_index =
        usize::try_from(ints[1]).map_err(|_| format!("bad actionIndex {}", ints[1]))?;
    let steps = history + 1;
    let need_ints = INT_HEADER + pointers * INTS_PER_POINTER;
    let need_floats = FLOAT_HEADER + steps * pointers * FLOATS_PER_SAMPLE;
    if ints.len() < need_ints {
        return Err(format!("ints too short: {} < {need_ints}", ints.len()));
    }
    if floats.len() < need_floats {
        return Err(format!(
            "floats too short: {} < {need_floats}",
            floats.len()
        ));
    }
    if times_ns.len() < steps {
        return Err(format!("times too short: {} < {steps}", times_ns.len()));
    }
    let ids = &ints[INT_HEADER..need_ints];
    let out_steps = (0..steps)
        .map(|s| {
            let base = FLOAT_HEADER + s * pointers * FLOATS_PER_SAMPLE;
            let pointers = (0..pointers)
                .map(|p| {
                    let f = &floats[base + p * FLOATS_PER_SAMPLE..][..FLOATS_PER_SAMPLE];
                    PointerSample {
                        id: ids[p * INTS_PER_POINTER],
                        tool_type: ids[p * INTS_PER_POINTER + 1],
                        x: f[0],
                        y: f[1],
                        pressure: f[2],
                        tilt: f[3],
                        orientation: f[4],
                    }
                })
                .collect();
            MotionStep {
                t_us: to_us(times_ns[s]),
                pointers,
            }
        })
        .collect();
    Ok(Motion {
        action: ints[0],
        action_index,
        button_state: ints[2],
        flags: ints[3],
        vscroll: floats[0],
        hscroll: floats[1],
        steps: out_steps,
    })
}

/// Maps a monotonic event time to the `now_us` epoch given one simultaneous reading of both
/// clocks. An event in the future maps to `now_us`; the result saturates at 0.
pub fn mono_to_now_us(event_ns: i64, mono_now_ns: i64, now_us: u64) -> u64 {
    let age_us = (mono_now_ns.saturating_sub(event_ns) / 1000).max(0) as u64;
    now_us.saturating_sub(age_us)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(ns: i64) -> u64 {
        mono_to_now_us(ns, 10_000_000, 5_000_000)
    }

    #[test]
    fn single_pointer_no_history() {
        let ints = [2, 0, 32, 0, 1, 0, 7, 2];
        let floats = [0.0, 0.0, 10.0, 20.0, 0.5, 0.25, -0.5];
        let m = unpack_motion(&ints, &floats, &[10_000_000], conv).unwrap();
        assert_eq!((m.action, m.action_index, m.button_state), (2, 0, 32));
        assert_eq!(m.steps.len(), 1);
        assert_eq!(m.steps[0].t_us, 5_000_000);
        let p = m.steps[0].pointers[0];
        assert_eq!((p.id, p.tool_type, p.x, p.y), (7, 2, 10.0, 20.0));
        assert_eq!((p.pressure, p.tilt, p.orientation), (0.5, 0.25, -0.5));
    }

    #[test]
    fn two_pointers_two_history_steps() {
        let ints = [5, 1, 0, 3, 2, 2, 0, 1, 1, 1];
        let mut floats = vec![1.5, -2.5];
        for s in 0..3 {
            for p in 0..2 {
                let v = (s * 10 + p) as f32;
                floats.extend([v, v + 0.1, 0.0, 0.0, 0.0]);
            }
        }
        let times = [8_000_000, 9_000_000, 10_000_000, 99];
        let m = unpack_motion(&ints, &floats, &times, conv).unwrap();
        assert_eq!(
            (m.vscroll, m.hscroll, m.flags, m.action_index),
            (1.5, -2.5, 3, 1)
        );
        assert_eq!(m.steps.len(), 3);
        assert_eq!(m.steps[0].t_us, 5_000_000 - 2000);
        assert_eq!(m.steps[1].t_us, 5_000_000 - 1000);
        assert_eq!(m.steps[2].pointers[1].x, 21.0);
        assert_eq!(m.steps[2].pointers[1].id, 1);
        assert_eq!(m.steps[1].pointers[0].tool_type, 1);
    }

    #[test]
    fn short_or_bad_arrays_are_errors() {
        let ints = [2, 0, 0, 0, 1, 0, 7, 2];
        let floats = [0.0; 7];
        assert!(unpack_motion(&ints[..5], &floats, &[0], conv).is_err());
        assert!(unpack_motion(&ints[..7], &floats, &[0], conv).is_err());
        assert!(unpack_motion(&ints, &floats[..6], &[0], conv).is_err());
        assert!(unpack_motion(&ints, &floats, &[], conv).is_err());
        let mut bad = ints;
        bad[4] = 0;
        assert!(unpack_motion(&bad, &floats, &[0], conv).is_err());
        bad[4] = 33;
        assert!(unpack_motion(&bad, &floats, &[0], conv).is_err());
        bad[4] = 1;
        bad[5] = 1025;
        assert!(unpack_motion(&bad, &floats, &[0], conv).is_err());
        bad[5] = -1;
        assert!(unpack_motion(&bad, &floats, &[0], conv).is_err());
        bad[5] = 0;
        bad[1] = -1;
        assert!(unpack_motion(&bad, &floats, &[0], conv).is_err());
    }

    #[test]
    fn time_conversion() {
        assert_eq!(mono_to_now_us(9_000_000, 10_000_000, 5_000), 4_000);
        assert_eq!(mono_to_now_us(11_000_000, 10_000_000, 5_000), 5_000);
        assert_eq!(mono_to_now_us(0, 10_000_000_000, 5_000), 0);
        assert_eq!(mono_to_now_us(i64::MIN, i64::MAX, 5), 0);
    }
}
