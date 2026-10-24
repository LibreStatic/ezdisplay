//! Pen tilt conversion.

/// Converts Android's `AXIS_TILT` / `AXIS_ORIENTATION` (radians) into protocol
/// `(tilt_x, tilt_y)` in hundredths of a degree, clamped to +-9000.
///
/// Orientation 0 means the pen top leans toward the top of the screen (away
/// from the user), which gives a negative `tilt_y`; orientation +PI/2 means
/// leaning right, which gives a positive `tilt_x`.
pub fn tilt_xy(tilt: f32, orientation: f32) -> (i16, i16) {
    if !tilt.is_finite() || !orientation.is_finite() {
        return (0, 0);
    }
    let max = 89.9_f64.to_radians();
    let t = f64::from(tilt).clamp(0.0, max).tan();
    let o = f64::from(orientation);
    let tx = (t * o.sin()).atan();
    let ty = -(t * o.cos()).atan();
    (hundredths(tx), hundredths(ty))
}

fn hundredths(rad: f64) -> i16 {
    (rad.to_degrees() * 100.0).round().clamp(-9000.0, 9000.0) as i16
}
