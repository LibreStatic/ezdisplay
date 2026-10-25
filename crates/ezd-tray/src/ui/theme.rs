//! Colours of the window (Fluent style), light and dark.

use eframe::egui::{self, Color32, Theme};

/// The colour roles the window uses (Fluent style).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Behind the pages.
    pub window_bg: Color32,
    /// Behind the navigation column.
    pub nav_bg: Color32,
    /// Cards, banners and controls.
    pub card: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    /// Accent for text and links on `window_bg` and `card`; the dark `accent` is too dark for that.
    pub accent_text: Color32,
    /// Fill of the selected navigation row.
    pub nav_selected: Color32,
    /// Icons and dots, not body text.
    pub success: Color32,
    /// Icons and dots, not body text.
    pub warning: Color32,
    /// Fill of a warning banner.
    pub warning_container: Color32,
    pub error: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const LIGHT: Palette = Palette {
    window_bg: rgb(0xF7F8FA),
    nav_bg: rgb(0xF2F4F8),
    card: rgb(0xFFFFFF),
    border: rgb(0xDFE2EA),
    text: rgb(0x171925),
    text_secondary: rgb(0x697184),
    accent: rgb(0x2C2A79),
    on_accent: rgb(0xFFFFFF),
    accent_text: rgb(0x2C2A79),
    nav_selected: rgb(0xE9EAF9),
    success: rgb(0x2E9B38),
    warning: rgb(0xC88600),
    warning_container: rgb(0xFFF3D6),
    error: rgb(0xC62828),
};

pub const DARK: Palette = Palette {
    window_bg: rgb(0x181A20),
    nav_bg: rgb(0x1C1E25),
    card: rgb(0x24262D),
    border: rgb(0x3B3E49),
    text: rgb(0xF4F4F7),
    text_secondary: rgb(0xA6AABA),
    accent: rgb(0x625FF0),
    on_accent: rgb(0xFFFFFF),
    accent_text: rgb(0x9896F5),
    nav_selected: rgb(0x30335A),
    success: rgb(0x4BD35C),
    warning: rgb(0xFFBD45),
    warning_container: rgb(0x3A2E14),
    error: rgb(0xEF6666),
};

pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Light => LIGHT,
        Theme::Dark => DARK,
    }
}

/// The palette of the theme `ui` is drawn in.
pub fn palette_of(ui: &egui::Ui) -> Palette {
    palette(ui.ctx().theme())
}

/// Installs the palettes for both themes; egui picks the one of the system.
pub fn install(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        let p = palette(theme);
        ctx.style_mut_of(theme, |style| {
            let v = &mut style.visuals;
            v.panel_fill = p.window_bg;
            v.window_fill = p.window_bg;
            v.extreme_bg_color = p.card;
            v.faint_bg_color = p.nav_bg;
            v.hyperlink_color = p.accent_text;
            v.error_fg_color = p.error;
            v.warn_fg_color = p.warning;
            v.selection.bg_fill = p.nav_selected;
            v.selection.stroke = egui::Stroke::new(1.0, p.accent);
            v.widgets.noninteractive.bg_fill = p.window_bg;
            v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
            v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
            for w in [
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.fg_stroke = egui::Stroke::new(1.0, p.text);
            }
            // Hover and press tint the card with the accent.
            let hover = p.card.lerp_to_gamma(p.accent, 0.08);
            let press = p.card.lerp_to_gamma(p.accent, 0.16);
            v.widgets.inactive.bg_fill = p.card;
            v.widgets.inactive.weak_bg_fill = p.card;
            v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.border);
            v.widgets.hovered.bg_fill = hover;
            v.widgets.hovered.weak_bg_fill = hover;
            v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.border);
            v.widgets.active.bg_fill = press;
            v.widgets.active.weak_bg_fill = press;
            v.widgets.active.bg_stroke = egui::Stroke::new(1.0, p.accent);
            v.widgets.open.bg_fill = hover;
            v.widgets.open.weak_bg_fill = hover;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_pairs_have_at_least_4_5_to_1_contrast() {
        for (name, p) in [("light", LIGHT), ("dark", DARK)] {
            let pairs = [
                ("text/window_bg", p.text, p.window_bg),
                ("text/nav_bg", p.text, p.nav_bg),
                ("text/card", p.text, p.card),
                ("text_secondary/window_bg", p.text_secondary, p.window_bg),
                ("text_secondary/card", p.text_secondary, p.card),
                ("on_accent/accent", p.on_accent, p.accent),
                ("accent_text/window_bg (links)", p.accent_text, p.window_bg),
                ("accent_text/card (links)", p.accent_text, p.card),
                ("text/nav_selected", p.text, p.nav_selected),
                ("accent_text/nav_selected", p.accent_text, p.nav_selected),
                ("error/window_bg", p.error, p.window_bg),
                ("text/warning_container", p.text, p.warning_container),
            ];
            for (what, fg, bg) in pairs {
                let c = contrast(fg, bg);
                assert!(c >= 4.5, "{name} {what}: {c:.2}");
            }
        }
    }

    #[test]
    fn status_colours_have_at_least_3_to_1_contrast_on_cards() {
        for (name, p) in [("light", LIGHT), ("dark", DARK)] {
            for (what, fg) in [("success", p.success), ("warning", p.warning)] {
                let c = contrast(fg, p.card);
                assert!(c >= 3.0, "{name} {what}/card: {c:.2}");
            }
        }
    }
}
