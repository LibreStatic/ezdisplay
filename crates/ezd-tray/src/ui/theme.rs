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
    /// Accent for text and links on `window_bg` and `card`.
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
    window_bg: rgb(0xECEFEE),
    nav_bg: rgb(0xE4E8E7),
    card: rgb(0xF4F6F5),
    border: rgb(0xCBD2CF),
    text: rgb(0x202928),
    text_secondary: rgb(0x58635F),
    accent: rgb(0x006E65),
    on_accent: rgb(0xFFFFFF),
    accent_text: rgb(0x005D56),
    nav_selected: rgb(0xD6E5E0),
    success: rgb(0x146F42),
    warning: rgb(0x815400),
    warning_container: rgb(0xF8EACA),
    error: rgb(0xB52F38),
};

pub const DARK: Palette = Palette {
    window_bg: rgb(0x101315),
    nav_bg: rgb(0x141819),
    card: rgb(0x090C0D),
    border: rgb(0x303839),
    text: rgb(0xE7ECEB),
    text_secondary: rgb(0xB0BCB9),
    accent: rgb(0x50D8BF),
    on_accent: rgb(0x082C25),
    accent_text: rgb(0x72E3D1),
    nav_selected: rgb(0x20312E),
    success: rgb(0x62DDA1),
    warning: rgb(0xF0BF6B),
    warning_container: rgb(0x3B3225),
    error: rgb(0xFF8F91),
};

pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Light => LIGHT,
        Theme::Dark => DARK,
    }
}

/// Minimum contrast of text against its background (WCAG AA).
const MIN_CONTRAST: f64 = 4.5;

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

/// WCAG contrast ratio of two opaque colours.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Moves `c` toward `target` in small steps until `ok` holds; `c` itself when it already does.
/// `target` itself is returned if no step satisfies `ok`.
fn shift_until(c: Color32, target: Color32, ok: impl Fn(Color32) -> bool) -> Color32 {
    const STEPS: u32 = 40;
    (0..=STEPS)
        .map(|i| c.lerp_to_gamma(target, i as f32 / STEPS as f32))
        .find(|&x| ok(x))
        .unwrap_or(target)
}

/// The palette of `theme` with the accent roles (`accent`, `on_accent`, `accent_text`,
/// `nav_selected`) derived from the system accent `accent`. Every text pair keeps at least 4.5:1:
/// in light mode the accent is darkened under white text, in dark mode it is lightened under
/// near-black text (as Windows 11 does). Neutrals and status colours stay as they are.
pub fn palette_with_accent(theme: Theme, accent: Color32) -> Palette {
    let mut p = palette(theme);
    let accent = Color32::from_rgb(accent.r(), accent.g(), accent.b());
    let (toward, on_accent, mix) = match theme {
        Theme::Light => (Color32::BLACK, Color32::WHITE, 0.2),
        Theme::Dark => (Color32::WHITE, Color32::BLACK, 0.25),
    };
    // Light: darken until white text reads on it; dark: lighten until near-black text does.
    p.on_accent = on_accent;
    p.accent = shift_until(accent, toward, |c| contrast(on_accent, c) >= MIN_CONTRAST);
    // The selected row: the navigation fill tinted with the accent, as much as the text allows.
    let mut m = mix;
    p.nav_selected = p.nav_bg.lerp_to_gamma(p.accent, m);
    while m > 0.0 && contrast(p.text, p.nav_selected) < MIN_CONTRAST {
        m = (m - 0.02).max(0.0);
        p.nav_selected = p.nav_bg.lerp_to_gamma(p.accent, m);
    }
    // Accent text and links on the page, cards and the selected row.
    let (window_bg, card, nav_selected) = (p.window_bg, p.card, p.nav_selected);
    p.accent_text = shift_until(accent, toward, |c| {
        [window_bg, card, nav_selected]
            .iter()
            .all(|&bg| contrast(c, bg) >= MIN_CONTRAST)
    });
    p
}

fn palette_id(theme: Theme) -> egui::Id {
    egui::Id::NULL.with(("ezd-tray-palette", theme == Theme::Dark))
}

/// The palette of the theme `ui` is drawn in: the one installed in the context, else the brand one.
pub fn palette_of(ui: &egui::Ui) -> Palette {
    let ctx = ui.ctx();
    let theme = ctx.theme();
    ctx.data(|d| d.get_temp::<Palette>(palette_id(theme)))
        .unwrap_or_else(|| palette(theme))
}

/// Installs the brand palettes for both themes; egui picks the one of the system.
pub fn install(ctx: &egui::Context) {
    install_accent(ctx, None);
}

/// Installs the palettes for both themes with the accent derived from `accent`; `None` restores
/// the brand palettes.
pub fn install_accent(ctx: &egui::Context, accent: Option<Color32>) {
    for theme in [Theme::Light, Theme::Dark] {
        let p = match accent {
            Some(a) => palette_with_accent(theme, a),
            None => palette(theme),
        };
        ctx.data_mut(|d| d.insert_temp(palette_id(theme), p));
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

    const ACCENTS: [u32; 12] = [
        0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0x00FFFF, 0xFF00FF, 0xFAFAFA, 0x050505, 0x808080,
        0x0078D4, 0x4F7A9A, 0x625FF0,
    ];

    fn hex(h: u32) -> Color32 {
        rgb(h)
    }

    #[test]
    fn derived_palettes_keep_the_text_contrast() {
        for theme in [Theme::Light, Theme::Dark] {
            for h in ACCENTS {
                let p = palette_with_accent(theme, hex(h));
                let pairs = [
                    ("text/nav_selected", p.text, p.nav_selected),
                    ("on_accent/accent", p.on_accent, p.accent),
                    ("accent_text/window_bg", p.accent_text, p.window_bg),
                    ("accent_text/card", p.accent_text, p.card),
                    ("accent_text/nav_selected", p.accent_text, p.nav_selected),
                ];
                for (what, fg, bg) in pairs {
                    let c = contrast(fg, bg);
                    assert!(c >= 4.5, "{theme:?} {h:06X} {what}: {c:.2}");
                }
                let base = palette(theme);
                assert_eq!(p.window_bg, base.window_bg);
                assert_eq!(p.card, base.card);
                assert_eq!(p.text, base.text);
                assert_eq!(p.error, base.error);
            }
        }
    }

    #[test]
    fn a_passing_accent_is_kept() {
        // Dark blue passes under white text and as text on light surfaces.
        let p = palette_with_accent(Theme::Light, hex(0x1F3A8A));
        assert_eq!(p.accent, hex(0x1F3A8A));
        assert_eq!(p.accent_text, hex(0x1F3A8A));
        // Light blue passes under black text and as text on dark surfaces.
        let p = palette_with_accent(Theme::Dark, hex(0x8AB4F8));
        assert_eq!(p.accent, hex(0x8AB4F8));
        assert_eq!(p.accent_text, hex(0x8AB4F8));
    }

    #[test]
    fn a_failing_accent_moves_in_the_direction_of_the_theme() {
        let light = palette_with_accent(Theme::Light, hex(0xFFFF00));
        assert!(luminance(light.accent) < luminance(hex(0xFFFF00)));
        let dark = palette_with_accent(Theme::Dark, hex(0x050505));
        assert!(luminance(dark.accent) > luminance(hex(0x050505)));
    }
}
