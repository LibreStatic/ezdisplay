//! A QR code drawn with the painter: black modules on a white square, which scanners need
//! whatever the theme is.

use eframe::egui::{self, Color32, Role, Sense, Ui, WidgetInfo, vec2};
use qrcode::{Color, EcLevel, QrCode};

/// Width of the quiet zone, in modules (the standard asks for four).
const QUIET_MODULES: usize = 4;

/// The modules of `text` row by row (`true` is dark) and the width of the square, or `None` when
/// the text does not fit in a code. Low error correction keeps the code small.
pub fn modules(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = QrCode::with_error_correction_level(text, EcLevel::L).ok()?;
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == Color::Dark)
        .collect();
    Some((code.width(), dark))
}

/// Draws the code of `text` in at most `side` points, with whole points per module so the edges
/// stay sharp. `label` names it for assistive technology. Draws nothing for text that does not
/// fit.
pub fn show(ui: &mut Ui, text: &str, side: f32, label: &str) {
    let Some((width, dark)) = modules(text) else {
        return;
    };
    let total = width + 2 * QUIET_MODULES;
    let cell = (side / total as f32).floor().max(1.0);
    let size = cell * total as f32;
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, Color32::WHITE);
    let origin = rect.min + vec2(cell, cell) * QUIET_MODULES as f32;
    for (i, _) in dark.iter().enumerate().filter(|(_, d)| **d) {
        let (x, y) = ((i % width) as f32, (i / width) as f32);
        let min = origin + vec2(x, y) * cell;
        painter.rect_filled(
            egui::Rect::from_min_size(min, vec2(cell, cell)),
            0.0,
            Color32::BLACK,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, label));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_uri_fits_in_a_square_code() {
        let (width, dark) = modules("ezd://pair?a=192.168.1.20:27183&pin=123456").unwrap();
        assert_eq!(dark.len(), width * width);
        // Version 1 is 21 modules wide; the code grows by 4 per version.
        assert!(width >= 21 && (width - 21) % 4 == 0);
        // The top-left finder pattern starts with a dark module.
        assert!(dark[0]);
    }

    #[test]
    fn text_too_long_for_a_code_gives_nothing() {
        assert!(modules(&"x".repeat(10_000)).is_none());
    }
}
