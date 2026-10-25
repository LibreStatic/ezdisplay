//! Fonts of the window: egui's defaults, the bundled icon font and, on Windows, the system UI font.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

const ICON_FONT: &str = "fluent-icons";
const ICON_FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/fluent-icons-subset.ttf");

/// Installs the fonts. The icon font comes after egui's defaults in both families, so a glyph of
/// [`super::glyphs`] renders inside normal text. On Windows the system UI font goes first; it is
/// read from the system and never bundled.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    add_icon_font(&mut fonts);
    if let Some(bytes) = system_ui_font() {
        add_system_font(&mut fonts, bytes);
    }
    ctx.set_fonts(fonts);
}

/// [`install_fonts`] without the system font, so tests lay out the same on every platform.
#[cfg(test)]
pub fn install_bundled_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    add_icon_font(&mut fonts);
    ctx.set_fonts(fonts);
}

fn add_icon_font(fonts: &mut FontDefinitions) {
    fonts.font_data.insert(
        ICON_FONT.to_owned(),
        Arc::new(FontData::from_static(ICON_FONT_BYTES)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(ICON_FONT.to_owned());
    }
}

fn add_system_font(fonts: &mut FontDefinitions, bytes: Vec<u8>) {
    const NAME: &str = "system-ui";
    fonts
        .font_data
        .insert(NAME.to_owned(), Arc::new(FontData::from_owned(bytes)));
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, NAME.to_owned());
}

/// Segoe UI, as the Windows shell draws its text.
#[cfg(windows)]
fn system_ui_font() -> Option<Vec<u8>> {
    let dir = std::path::Path::new(&std::env::var_os("WINDIR")?).join("Fonts");
    ["SegUIVar.ttf", "segoeui.ttf"]
        .iter()
        .find_map(|name| std::fs::read(dir.join(name)).ok())
}

#[cfg(not(windows))]
fn system_ui_font() -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::glyphs;
    use egui::FontId;
    use egui_kittest::Harness;

    #[test]
    fn icon_font_is_a_fallback_after_the_defaults() {
        let mut fonts = FontDefinitions::default();
        add_icon_font(&mut fonts);
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            assert_eq!(fonts.families[&family].last().unwrap(), ICON_FONT);
        }
        add_system_font(&mut fonts, ICON_FONT_BYTES.to_vec());
        assert_eq!(fonts.families[&FontFamily::Proportional][0], "system-ui");
    }

    /// Every constant of `glyphs` is in the embedded font. Only the icon font is installed here, so
    /// a glyph egui knows can only come from it.
    #[test]
    fn every_glyph_constant_is_in_the_embedded_font() {
        let mut h = Harness::new_ui(|ui| {
            let mut fonts = FontDefinitions::empty();
            add_icon_font(&mut fonts);
            ui.ctx().set_fonts(fonts);
            ui.label(glyphs::PULSE);
        });
        h.run();
        let missing: Vec<String> = h.ctx.fonts_mut(|f| {
            glyphs::ALL
                .iter()
                .filter(|g| {
                    let c = g.chars().next().unwrap();
                    g.chars().count() != 1 || !f.has_glyph(&FontId::proportional(14.0), c)
                })
                .map(|g| format!("U+{:04X}", g.chars().next().unwrap() as u32))
                .collect()
        });
        assert!(missing.is_empty(), "not in the icon font: {missing:?}");
    }

    #[test]
    fn install_fonts_makes_icons_available_in_normal_text() {
        let mut h = Harness::new_ui(|ui| {
            install_fonts(ui.ctx());
            ui.label("x");
        });
        h.run();
        let ok = h.ctx.fonts_mut(|f| {
            f.has_glyphs(
                &FontId::proportional(14.0),
                &[glyphs::PULSE, glyphs::SETTINGS].concat(),
            ) && f.has_glyph(&FontId::monospace(14.0), '\u{E9DC}')
        });
        assert!(ok);
    }
}
