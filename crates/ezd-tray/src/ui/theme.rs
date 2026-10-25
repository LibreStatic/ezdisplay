//! Colours of the window: the seed scheme of the Android app (Material You), light and dark.

use eframe::egui::{self, Color32, Theme};

/// The colour roles the window uses.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub primary: Color32,
    pub on_primary: Color32,
    pub primary_container: Color32,
    pub surface: Color32,
    pub on_surface: Color32,
    pub surface_container: Color32,
    /// Secondary text.
    pub on_surface_variant: Color32,
    pub outline: Color32,
    pub error: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const LIGHT: Palette = Palette {
    primary: rgb(0x595992),
    on_primary: rgb(0xFFFFFF),
    primary_container: rgb(0xE2DFFF),
    surface: rgb(0xFCF8FF),
    on_surface: rgb(0x1B1B21),
    surface_container: rgb(0xF0ECF4),
    on_surface_variant: rgb(0x46464F),
    outline: rgb(0x777680),
    error: rgb(0xBA1A1A),
};

pub const DARK: Palette = Palette {
    primary: rgb(0xC2C1FF),
    on_primary: rgb(0x2B2A60),
    primary_container: rgb(0x414178),
    surface: rgb(0x131318),
    on_surface: rgb(0xE5E1E9),
    surface_container: rgb(0x1F1F25),
    on_surface_variant: rgb(0xC7C5D0),
    outline: rgb(0x918F9A),
    error: rgb(0xFFB4AB),
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
            v.panel_fill = p.surface;
            v.window_fill = p.surface;
            v.extreme_bg_color = p.surface_container;
            v.faint_bg_color = p.surface_container;
            v.hyperlink_color = p.primary;
            v.error_fg_color = p.error;
            v.selection.bg_fill = p.primary_container;
            v.selection.stroke = egui::Stroke::new(1.0, p.on_surface);
            v.widgets.noninteractive.bg_fill = p.surface;
            v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.outline);
            v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.on_surface);
            for w in [
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.fg_stroke = egui::Stroke::new(1.0, p.on_surface);
            }
            v.widgets.inactive.bg_fill = p.surface_container;
            v.widgets.inactive.weak_bg_fill = p.surface_container;
            v.widgets.hovered.bg_fill = p.primary_container;
            v.widgets.hovered.weak_bg_fill = p.primary_container;
            v.widgets.active.bg_fill = p.primary_container;
            v.widgets.active.weak_bg_fill = p.primary_container;
            v.widgets.open.bg_fill = p.surface_container;
            v.widgets.open.weak_bg_fill = p.surface_container;
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
                ("on_surface/surface", p.on_surface, p.surface),
                (
                    "on_surface/surface_container",
                    p.on_surface,
                    p.surface_container,
                ),
                (
                    "on_surface/primary_container",
                    p.on_surface,
                    p.primary_container,
                ),
                (
                    "on_surface_variant/surface",
                    p.on_surface_variant,
                    p.surface,
                ),
                (
                    "on_surface_variant/surface_container",
                    p.on_surface_variant,
                    p.surface_container,
                ),
                ("on_primary/primary", p.on_primary, p.primary),
                ("primary/surface (links)", p.primary, p.surface),
                (
                    "primary/surface_container (sidebar title)",
                    p.primary,
                    p.surface_container,
                ),
                ("error/surface", p.error, p.surface),
            ];
            for (what, fg, bg) in pairs {
                let c = contrast(fg, bg);
                assert!(c >= 4.5, "{name} {what}: {c:.2}");
            }
        }
    }
}
