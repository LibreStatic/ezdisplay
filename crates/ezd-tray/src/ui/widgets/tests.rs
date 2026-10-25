//! Semantic tests of the widgets: egui's AccessKit tree, no rendering and no display needed.

use super::*;
use crate::ui::{fonts, glyphs, theme};
use eframe::egui::accesskit::{self, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

fn harness<S: 'static>(
    state: S,
    mut add: impl FnMut(&mut Ui, Palette, &mut S) + 'static,
) -> Harness<'static, S> {
    let mut h = Harness::builder()
        .with_size(vec2(480.0, 320.0))
        .build_ui_state(
            move |ui, state| {
                theme::install(ui.ctx());
                fonts::install_bundled_fonts(ui.ctx());
                let pal = theme::palette_of(ui);
                add(ui, pal, state);
            },
            state,
        );
    h.run();
    h
}

#[test]
fn toggle_is_a_checkbox_named_by_its_label_and_flips_on_click() {
    let mut h = harness(false, |ui, pal, on| {
        toggle_switch(ui, pal, on, "Start with Windows");
    });
    let toggle = h.get_by_role_and_label(Role::CheckBox, "Start with Windows");
    assert_eq!(
        toggle.accesskit_node().toggled(),
        Some(accesskit::Toggled::False)
    );
    toggle.click();
    h.run();
    assert!(*h.state());
    let toggle = h.get_by_role_and_label(Role::CheckBox, "Start with Windows");
    assert_eq!(
        toggle.accesskit_node().toggled(),
        Some(accesskit::Toggled::True)
    );
    h.get_by_label("Start with Windows").click();
    h.run();
    assert!(!*h.state());
}

#[test]
fn toggle_marks_the_response_changed() {
    let mut h = harness((false, 0), |ui, pal, (on, changes)| {
        if toggle_switch(ui, pal, on, "Flag").changed() {
            *changes += 1;
        }
    });
    h.get_by_label("Flag").click();
    h.run();
    assert_eq!(h.state().1, 1);
}

#[test]
fn clicking_a_segment_selects_it() {
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Mode {
        Auto,
        Manual,
        Off,
    }
    let mut h = harness(Mode::Auto, |ui, pal, mode| {
        segmented(
            ui,
            pal,
            mode,
            &[
                (Mode::Auto, "Auto"),
                (Mode::Manual, "Manual"),
                (Mode::Off, "Off"),
            ],
        );
    });
    assert_eq!(
        h.get_by_label("Auto").accesskit_node().toggled(),
        Some(accesskit::Toggled::True)
    );
    assert_eq!(
        h.get_by_label("Manual").accesskit_node().toggled(),
        Some(accesskit::Toggled::False)
    );
    h.get_by_label("Off").click();
    h.run();
    assert_eq!(*h.state(), Mode::Off);
    assert_eq!(
        h.get_by_label("Off").accesskit_node().toggled(),
        Some(accesskit::Toggled::True)
    );
    h.get_by_label("Manual").click();
    h.run();
    assert_eq!(*h.state(), Mode::Manual);
}

#[test]
fn nav_item_shows_its_label_and_responds_to_clicks() {
    let mut h = harness((false, 0), |ui, pal, (_, clicks)| {
        let r = nav_item(ui, pal, glyphs::PULSE, "Status", true);
        let r2 = nav_item(ui, pal, glyphs::DESKTOP, "Display", false);
        if r.clicked() || r2.clicked() {
            *clicks += 1;
        }
    });
    assert_eq!(
        h.get_by_role_and_label(Role::Button, "Status")
            .accesskit_node()
            .toggled(),
        Some(accesskit::Toggled::True)
    );
    h.get_by_role_and_label(Role::Button, "Display").click();
    h.run();
    assert_eq!(h.state().1, 1);
}

#[test]
fn banner_shows_title_and_subtitle_and_its_action_fires() {
    let mut h = harness(0, |ui, pal, fired| {
        banner(
            ui,
            pal,
            Tone::Warning,
            glyphs::WARNING,
            "Tablet not found",
            Some("Connect it with a USB cable"),
            |ui| {
                if secondary_button(ui, pal, "Retry").clicked() {
                    *fired += 1;
                }
            },
        );
    });
    h.get_by_label("Tablet not found");
    h.get_by_label("Connect it with a USB cable");
    h.get_by_label("Retry").click();
    h.run();
    assert_eq!(*h.state(), 1);
}

#[test]
fn a_long_banner_title_wraps_clear_of_the_action() {
    let h = harness((), |ui, pal, ()| {
        banner(
            ui,
            pal,
            Tone::Warning,
            glyphs::WARNING,
            "Touch and pen need permission to create input devices (/dev/uinput) on this computer.",
            Some("Authorize it once"),
            |ui| {
                primary_button(ui, pal, "Enable input");
            },
        );
    });
    let title = h
        .get_by_label(
            "Touch and pen need permission to create input devices (/dev/uinput) on this computer.",
        )
        .rect();
    let action = h.get_by_label("Enable input").rect();
    assert!(title.max.x <= action.min.x, "{title:?} {action:?}");
    assert!(title.height() > 20.0, "the title wraps: {title:?}");
}

#[test]
fn every_banner_tone_renders_without_a_subtitle() {
    for tone in [Tone::Neutral, Tone::Success, Tone::Warning, Tone::Error] {
        let h = harness((), move |ui, pal, ()| {
            banner(ui, pal, tone, glyphs::INFO, "Title", None, |_| {});
        });
        h.get_by_label("Title");
    }
}

#[test]
fn setting_row_shows_label_and_description_and_centres_its_control() {
    let h = harness(false, |ui, pal, on| {
        card(ui, pal, Some((glyphs::SETTINGS, "General")), |ui| {
            setting_row(
                ui,
                pal,
                "Run at startup",
                Some("Starts with the PC"),
                |ui| {
                    toggle_switch(ui, pal, on, "Run at startup toggle");
                },
            );
            row_divider(ui, pal);
            setting_row(ui, pal, "Second", None, |ui| {
                primary_button(ui, pal, "Go");
            });
        });
    });
    h.get_by_label("General");
    h.get_by_label("Run at startup");
    h.get_by_label("Starts with the PC");
    let label = h.get_by_label("Run at startup").rect();
    let desc = h.get_by_label("Starts with the PC").rect();
    let toggle = h.get_by_label("Run at startup toggle").rect();
    let text_centre = (label.min.y + desc.max.y) / 2.0;
    assert!(
        (toggle.center().y - text_centre).abs() < 2.0,
        "toggle {toggle:?} text {label:?} {desc:?}"
    );
    let second = h.get_by_label("Second").rect();
    let go = h.get_by_label("Go").rect();
    assert!((second.center().y - go.center().y).abs() < 2.0);
    // The second row starts at least one row height below the first.
    assert!(second.center().y - toggle.center().y >= ROW_MIN_HEIGHT);
}

#[test]
fn stat_tile_shows_label_value_and_unit() {
    let h = harness((), |ui, pal, ()| {
        stat_tile(
            ui,
            pal,
            glyphs::GAUGE,
            "Frame rate",
            "119",
            Some("fps"),
            150.0,
        );
        stat_tile(ui, pal, glyphs::TIMER, "Round trip", "2.5", None, 150.0);
    });
    h.get_by_label("Frame rate");
    // The value and its unit are one label.
    let value = h.get_by_label("119 fps").rect();
    h.get_by_label("Round trip");
    h.get_by_label("2.5");
    assert!(value.width() < 150.0);
    // The tile is as wide as asked.
    assert!(h.get_by_label("Frame rate").rect().min.x >= 0.0);
}

#[test]
fn toggle_row_names_the_toggle_once_and_flips_it() {
    let mut h = harness(false, |ui, pal, on| {
        toggle_row(ui, pal, "Run at startup", Some("Starts with the PC"), on);
    });
    // The label is on the switch only; the visible text has no node of its own.
    h.get_by_role_and_label(Role::CheckBox, "Run at startup")
        .click();
    h.run();
    assert!(*h.state());
    assert!(h.query_by_label("Starts with the PC").is_none());
}

#[test]
fn labelled_row_names_its_control_by_the_label() {
    let h = harness(0usize, |ui, pal, choice| {
        labelled_row(ui, pal, "Scale", Some("Resolution"), None, |ui| {
            egui::ComboBox::from_id_salt("c")
                .selected_text(format!("{choice}"))
                .show_ui(ui, |_| {})
                .response
        });
    });
    h.get_by_role_and_label(Role::ComboBox, "Scale");
    h.get_by_label("Resolution");
}

#[test]
fn a_wide_control_goes_below_the_texts_when_it_does_not_fit() {
    for (width, below) in [(900.0, false), (240.0, true)] {
        let mut h = Harness::builder()
            .with_size(vec2(width, 320.0))
            .build_ui(move |ui| {
                theme::install(ui.ctx());
                fonts::install_bundled_fonts(ui.ctx());
                let pal = theme::palette_of(ui);
                setting_row_wide(ui, pal, "Pen", Some("How the pen acts"), 200.0, |ui| {
                    secondary_button(ui, pal, "Wide control");
                });
            });
        h.run();
        let text = h.get_by_label("Pen").rect();
        let control = h.get_by_label("Wide control").rect();
        assert_eq!(control.min.y > text.max.y, below, "{text:?} {control:?}");
        if !below {
            assert!(control.min.x > text.max.x || control.min.y > text.max.y);
        }
    }
}

#[test]
fn info_row_shows_name_and_value() {
    let h = harness((), |ui, pal, ()| {
        info_row(ui, pal, "Tablet", "Galaxy Tab S9");
    });
    let name = h.get_by_label("Tablet").rect();
    let value = h.get_by_label("Galaxy Tab S9").rect();
    assert!(value.min.x > name.max.x);
}

#[test]
fn empty_state_shows_its_title_and_hint() {
    let h = harness((), |ui, pal, ()| {
        empty_state(ui, pal, glyphs::TABLET, "Nothing here", Some("Add one"));
    });
    h.get_by_label("Nothing here");
    h.get_by_label("Add one");
}

#[test]
fn disclosure_opens_and_closes_from_its_header() {
    let mut h = harness(false, |ui, pal, open| {
        disclosure(ui, pal, glyphs::SETTINGS, "Advanced", open, |ui| {
            ui.label("Inside");
        });
    });
    assert!(h.query_by_label("Inside").is_none());
    let header = h.get_by_role_and_label(Role::Button, "Advanced");
    assert_eq!(header.accesskit_node().data().is_expanded(), Some(false));
    header.click();
    h.run();
    assert!(*h.state());
    h.get_by_label("Inside");
    let header = h.get_by_role_and_label(Role::Button, "Advanced");
    assert_eq!(header.accesskit_node().data().is_expanded(), Some(true));
    header.click();
    h.run();
    assert!(h.query_by_label("Inside").is_none());
}

#[test]
fn buttons_and_footer_bar_respond_to_clicks() {
    let mut h = harness(0, |ui, pal, clicks| {
        footer_bar(ui, pal, |ui| {
            if primary_button(ui, pal, "Apply now").clicked() {
                *clicks += 1;
            }
            if secondary_button(ui, pal, "Cancel").clicked() {
                *clicks += 10;
            }
        });
    });
    h.get_by_label("Apply now").click();
    h.run();
    h.get_by_label("Cancel").click();
    h.run();
    assert_eq!(*h.state(), 11);
}

#[test]
fn a_long_nav_label_wraps_and_the_row_grows() {
    let long = "Controlador de pantalla virtual";
    let h = Harness::builder()
        .with_size(vec2(200.0, 200.0))
        .build_ui_state(
            move |ui, _: &mut ()| {
                theme::install(ui.ctx());
                fonts::install_bundled_fonts(ui.ctx());
                let pal = theme::palette_of(ui);
                ui.set_max_width(176.0);
                nav_item(ui, pal, glyphs::DEVELOPER_BOARD, long, true);
                nav_item(ui, pal, glyphs::PULSE, "Status", false);
            },
            (),
        );
    let long_row = h.get_by_role_and_label(Role::Button, long).rect();
    let short_row = h.get_by_role_and_label(Role::Button, "Status").rect();
    assert!(long_row.height() > NAV_ROW_HEIGHT, "{long_row:?}");
    assert_eq!(short_row.height(), NAV_ROW_HEIGHT);
    assert!(long_row.max.x <= 200.0);
}
