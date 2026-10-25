//! Fluent-style building blocks of the window: navigation rows, cards, setting rows, switches,
//! segmented controls, banners and buttons. No shadows or blur, only fills, 1 px borders and rounded
//! corners.

use super::theme::Palette;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, InnerResponse, Layout, Margin,
    Rect, Response, RichText, Sense, Stroke, StrokeKind, TextFormat, Ui, WidgetInfo,
    accesskit::Role, text::LayoutJob, vec2,
};

/// Corner radius of cards and banners.
pub const CARD_RADIUS: u8 = 8;
/// Corner radius of controls.
pub const CONTROL_RADIUS: u8 = 6;
/// Inner margin of cards.
pub const CARD_PADDING: i8 = 16;
/// Least height of a setting row.
pub const ROW_MIN_HEIGHT: f32 = 56.0;
/// Height of a navigation row.
pub const NAV_ROW_HEIGHT: f32 = 40.0;
const CONTROL_HEIGHT: f32 = 32.0;
const ICON_SIZE: f32 = 20.0;

/// How a [`banner`] reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Success,
    Warning,
    Error,
}

fn border_stroke(pal: Palette) -> Stroke {
    Stroke::new(1.0, pal.border)
}

/// The faint accent tint of hovered rows.
fn hover_fill(pal: Palette) -> Color32 {
    pal.nav_bg.lerp_to_gamma(pal.accent, 0.08)
}

/// Dark or light, whichever reads on `bg`.
fn on_color(pal: Palette, bg: Color32) -> Color32 {
    let luma = 0.299 * f32::from(bg.r()) + 0.587 * f32::from(bg.g()) + 0.114 * f32::from(bg.b());
    if luma > 150.0 {
        crate::ui::theme::LIGHT.text
    } else {
        pal.on_accent
    }
}

/// A 1 px line across the available width.
pub fn divider(ui: &mut Ui, pal: Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, pal.border);
}

/// A full-width navigation row: icon and label. The selected row has an accent bar at its left
/// edge.
pub fn nav_item(ui: &mut Ui, pal: Palette, icon: &str, label: &str, selected: bool) -> Response {
    let width = ui.available_width();
    let color = if selected { pal.accent_text } else { pal.text };
    // The label wraps into a second line and the row grows with it; it is never cut.
    let galley = ui.painter().layout(
        label.to_owned(),
        FontId::proportional(15.0),
        color,
        (width - 46.0 - 8.0).max(40.0),
    );
    let height = NAV_ROW_HEIGHT.max(galley.size().y + 16.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, label));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(CONTROL_RADIUS);
        if selected {
            painter.rect_filled(rect, radius, pal.nav_selected);
            let bar = Rect::from_center_size(
                rect.left_center() + vec2(1.5, 0.0),
                vec2(3.0, NAV_ROW_HEIGHT * 0.5),
            );
            painter.rect_filled(bar, CornerRadius::same(2), pal.accent);
        } else if response.hovered() {
            painter.rect_filled(rect, radius, hover_fill(pal));
        }
        let y = rect.center().y;
        painter.text(
            egui::pos2(rect.left() + 14.0, y),
            Align2::LEFT_CENTER,
            icon,
            FontId::proportional(ICON_SIZE),
            color,
        );
        let pos = egui::pos2(rect.left() + 46.0, y - galley.size().y / 2.0);
        painter.galley(pos, galley, color);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A bordered surface with rounded corners. With a `title` (icon and text) it starts with a header
/// row and a divider.
pub fn card<R>(
    ui: &mut Ui,
    pal: Palette,
    title: Option<(&str, &str)>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    surface(ui, pal.card, border_stroke(pal), |ui| {
        // Rows and dividers sit flush; the contents add their own space.
        ui.spacing_mut().item_spacing.y = 0.0;
        if let Some((icon, title)) = title {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(ICON_SIZE).color(pal.accent_text));
                ui.label(RichText::new(title).size(17.0).color(pal.text));
            });
            ui.add_space(10.0);
            divider(ui, pal);
        }
        add_contents(ui)
    })
}

fn surface<R>(
    ui: &mut Ui,
    fill: Color32,
    stroke: Stroke,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(CARD_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
}

/// A divider to draw between two [`setting_row`]s.
pub fn row_divider(ui: &mut Ui, pal: Palette) {
    divider(ui, pal);
}

/// Gap between the texts and the control of a row.
const ROW_GAP: f32 = 16.0;
/// Least width of the texts of a row before the control moves below them.
const MIN_TEXT_WIDTH: f32 = 140.0;
/// Space between the label and the description.
const TEXT_GAP: f32 = 2.0;

/// Text that is drawn but has no AccessKit node, for rows whose control carries the label.
pub fn static_text(ui: &mut Ui, text: &str, size: f32, color: Color32) {
    let galley = ui.painter().layout(
        text.to_owned(),
        FontId::proportional(size),
        color,
        ui.available_width(),
    );
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

/// The shared layout of the setting rows. `accessible` makes the texts labels of the AccessKit
/// tree; `control_width` is the width the control needs (the control goes below the texts when it
/// does not fit beside them), `None` gives the texts 60 % of the row. The control closure gets the
/// id of the label when there is one.
fn row_impl<R>(
    ui: &mut Ui,
    pal: Palette,
    label: &str,
    description: Option<&str>,
    accessible: bool,
    control_width: Option<f32>,
    add_control: impl FnOnce(&mut Ui, Option<Id>) -> R,
) -> InnerResponse<R> {
    let width = ui.available_width();
    let origin = ui.cursor().min;
    let stacked = control_width.is_some_and(|cw| width - cw - ROW_GAP < MIN_TEXT_WIDTH);
    let text_width = match control_width {
        _ if stacked => width,
        Some(cw) => width - cw - ROW_GAP,
        None => width * 0.6,
    };
    let measure = |ui: &Ui, text: &str, size: f32| {
        ui.painter()
            .layout(
                text.to_owned(),
                FontId::proportional(size),
                pal.text,
                text_width,
            )
            .size()
            .y
    };
    let mut text_height = measure(ui, label, 14.0);
    if let Some(description) = description {
        text_height += TEXT_GAP + measure(ui, description, 12.0);
    }
    let text_rect = Rect::from_min_size(origin, vec2(text_width, ROW_MIN_HEIGHT));
    let mut texts = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    // egui has no vertical centring in a fixed-height row: pad by hand.
    let pad = ((ROW_MIN_HEIGHT - text_height) / 2.0).max(12.0);
    texts.add_space(pad);
    let mut label_id = None;
    if accessible {
        label_id = Some(
            texts
                .label(RichText::new(label).size(14.0).color(pal.text))
                .id,
        );
    } else {
        static_text(&mut texts, label, 14.0, pal.text);
    }
    if let Some(description) = description {
        texts.add_space(TEXT_GAP);
        if accessible {
            texts.label(
                RichText::new(description)
                    .size(12.0)
                    .color(pal.text_secondary),
            );
        } else {
            static_text(&mut texts, description, 12.0, pal.text_secondary);
        }
    }
    let texts_height = texts.min_rect().height();
    if stacked {
        let below = Rect::from_min_size(
            egui::pos2(origin.x, origin.y + texts_height),
            vec2(width, CONTROL_HEIGHT),
        );
        let mut control = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(below)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let inner = add_control(&mut control, label_id);
        let height = texts_height + control.min_rect().height().max(CONTROL_HEIGHT) + pad;
        let row = Rect::from_min_size(origin, vec2(width, height));
        return InnerResponse::new(inner, ui.allocate_rect(row, Sense::hover()));
    }
    let height = (texts_height + pad).max(ROW_MIN_HEIGHT);
    let row = Rect::from_min_size(origin, vec2(width, height));
    let mut control = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row)
            .layout(Layout::right_to_left(Align::Center)),
    );
    let inner = add_control(&mut control, label_id);
    InnerResponse::new(inner, ui.allocate_rect(row, Sense::hover()))
}

/// A label with an optional description on the left and its control, right-aligned and centred
/// vertically, on the right.
pub fn setting_row<R>(
    ui: &mut Ui,
    pal: Palette,
    label: &str,
    description: Option<&str>,
    add_control: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    row_impl(ui, pal, label, description, true, None, |ui, _| {
        add_control(ui)
    })
}

/// A [`setting_row`] for a control that needs `control_width` points: when it does not fit beside
/// the texts it goes below them.
pub fn setting_row_wide<R>(
    ui: &mut Ui,
    pal: Palette,
    label: &str,
    description: Option<&str>,
    control_width: f32,
    add_control: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    row_impl(
        ui,
        pal,
        label,
        description,
        true,
        Some(control_width),
        |ui, _| add_control(ui),
    )
}

/// A [`setting_row`] whose control is named by the row's label for screen readers (combo boxes,
/// number fields). `control_width` is how wide the control is: when it does not fit beside the
/// texts it goes below them.
pub fn labelled_row(
    ui: &mut Ui,
    pal: Palette,
    label: &str,
    description: Option<&str>,
    control_width: Option<f32>,
    add_control: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    row_impl(
        ui,
        pal,
        label,
        description,
        true,
        control_width,
        |ui, label_id| {
            let response = add_control(ui);
            match label_id {
                Some(id) => response.labelled_by(id),
                None => response,
            }
        },
    )
    .inner
}

/// A [`setting_row`] with a [`toggle_switch`]. The switch is the only node carrying the label, so
/// the text is found once.
pub fn toggle_row(
    ui: &mut Ui,
    pal: Palette,
    label: &str,
    description: Option<&str>,
    on: &mut bool,
) -> Response {
    row_impl(ui, pal, label, description, false, None, |ui, _| {
        toggle_switch(ui, pal, on, label)
    })
    .inner
}

/// A compact read-only line: the name on the left, the value (wrapping) on the right.
pub fn info_row(ui: &mut Ui, pal: Palette, label: &str, value: &str) {
    let width = ui.available_width();
    let label_width = (width * 0.38).min(190.0);
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        let gap = ui.spacing().item_spacing.x;
        ui.allocate_ui_with_layout(vec2(label_width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(label_width);
            ui.label(RichText::new(label).size(13.0).color(pal.text_secondary));
        });
        let value_width = (width - label_width - gap).max(60.0);
        ui.allocate_ui_with_layout(vec2(value_width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(value_width);
            ui.label(RichText::new(value).size(14.0).color(pal.text));
        });
    });
    ui.add_space(8.0);
}

/// An icon, a title and an optional hint, for lists that have nothing to show yet.
pub fn empty_state(ui: &mut Ui, pal: Palette, icon: &str, title: &str, hint: Option<&str>) {
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon).size(28.0).color(pal.text_secondary));
        ui.label(RichText::new(title).size(14.0).color(pal.text));
        if let Some(hint) = hint {
            ui.label(RichText::new(hint).size(12.0).color(pal.text_secondary));
        }
    });
    ui.add_space(8.0);
}

/// A card that collapses: a clickable header (icon, title and a chevron) and, while `open`, a
/// divider and the contents. The header is a button named by `title` that reports its expanded
/// state.
pub fn disclosure<R>(
    ui: &mut Ui,
    pal: Palette,
    icon: &str,
    title: &str,
    open: &mut bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    surface(ui, pal.card, border_stroke(pal), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        if response.clicked() {
            *open = !*open;
        }
        response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), title));
        let expanded = *open;
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_expanded(expanded);
        });
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let y = rect.center().y;
            painter.text(
                egui::pos2(rect.left(), y),
                Align2::LEFT_CENTER,
                icon,
                FontId::proportional(ICON_SIZE),
                pal.accent_text,
            );
            painter.text(
                egui::pos2(rect.left() + ICON_SIZE + 8.0, y),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(17.0),
                pal.text,
            );
            painter.text(
                egui::pos2(rect.right(), y),
                Align2::RIGHT_CENTER,
                if *open {
                    super::glyphs::CHEVRON_UP
                } else {
                    super::glyphs::CHEVRON_DOWN
                },
                FontId::proportional(ICON_SIZE),
                pal.text_secondary,
            );
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if *open {
            ui.add_space(10.0);
            divider(ui, pal);
            Some(add_contents(ui))
        } else {
            None
        }
    })
    .inner
}

/// A pill-shaped switch of 40 by 20 points with the semantics of a checkbox named `label`.
pub fn toggle_switch(ui: &mut Ui, pal: Palette, on: &mut bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(Role::CheckBox, ui.is_enabled(), *on, label));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.1);
        let painter = ui.painter();
        let pill = CornerRadius::same(10);
        let knob_x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        if *on {
            painter.rect_filled(rect, pill, pal.accent);
            painter.circle_filled(egui::pos2(knob_x, rect.center().y), 6.5, pal.on_accent);
        } else {
            painter.rect_stroke(
                rect.shrink(0.5),
                pill,
                Stroke::new(1.0, pal.text_secondary),
                StrokeKind::Inside,
            );
            painter.circle_filled(egui::pos2(knob_x, rect.center().y), 5.0, pal.text_secondary);
        }
        if response.hovered() {
            painter.rect_stroke(
                rect.expand(2.0),
                CornerRadius::same(12),
                Stroke::new(1.0, pal.border),
                StrokeKind::Outside,
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Width of the segments of a [`segmented`] control, in order.
fn segment_widths(ui: &Ui, options: &[&str]) -> Vec<f32> {
    options
        .iter()
        .map(|text| {
            ui.painter()
                .layout_no_wrap(
                    (*text).to_owned(),
                    FontId::proportional(14.0),
                    Color32::PLACEHOLDER,
                )
                .size()
                .x
                + 32.0
        })
        .collect()
}

/// Total width of a [`segmented`] control with these option texts.
pub fn segmented_width(ui: &Ui, options: &[&str]) -> f32 {
    segment_widths(ui, options).iter().sum()
}

/// Joined buttons; the selected one is filled with the accent. Changing `value` marks the
/// response changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    pal: Palette,
    value: &mut T,
    options: &[(T, &str)],
) -> Response {
    let font = FontId::proportional(14.0);
    let texts: Vec<&str> = options.iter().map(|(_, text)| *text).collect();
    let widths = segment_widths(ui, &texts);
    let total = widths.iter().sum::<f32>();
    let (rect, mut response) = ui.allocate_exact_size(vec2(total, CONTROL_HEIGHT), Sense::hover());
    let mut x = rect.left();
    let last = options.len().saturating_sub(1);
    for (i, ((option, text), width)) in options.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(egui::pos2(x, rect.top()), vec2(*width, CONTROL_HEIGHT));
        x += width;
        let seg_response = ui.interact(seg, response.id.with(i), Sense::click());
        let selected = *value == *option;
        seg_response
            .widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, *text));
        if seg_response.clicked() && !selected {
            *value = *option;
            response.mark_changed();
        }
        if ui.is_rect_visible(seg) {
            let r = CONTROL_RADIUS;
            let radius = CornerRadius {
                nw: if i == 0 { r } else { 0 },
                sw: if i == 0 { r } else { 0 },
                ne: if i == last { r } else { 0 },
                se: if i == last { r } else { 0 },
            };
            let (fill, fg) = if *value == *option {
                (pal.accent, pal.on_accent)
            } else if seg_response.hovered() {
                (hover_fill(pal), pal.text)
            } else {
                (pal.card, pal.text)
            };
            let painter = ui.painter();
            painter.rect_filled(seg, radius, fill);
            painter.rect_stroke(seg, radius, border_stroke(pal), StrokeKind::Inside);
            let galley = ui
                .painter()
                .layout_no_wrap((*text).to_owned(), font.clone(), fg);
            let pos = Align2::CENTER_CENTER.anchor_size(seg.center(), galley.size());
            ui.painter().galley(pos.min, galley, fg);
        }
    }
    response
}

/// A full-width card with a coloured icon circle, a title, a secondary line and an action on the
/// right (pass `|_| {}` for none).
pub fn banner<R>(
    ui: &mut Ui,
    pal: Palette,
    tone: Tone,
    icon: &str,
    title: &str,
    subtitle: Option<&str>,
    add_action: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let (fill, stroke_color, circle) = match tone {
        Tone::Neutral => (pal.card, pal.border, pal.accent),
        Tone::Success => (
            pal.card.lerp_to_gamma(pal.success, 0.08),
            pal.success,
            pal.success,
        ),
        Tone::Warning => (pal.warning_container, pal.warning, pal.warning),
        Tone::Error => (
            pal.card.lerp_to_gamma(pal.error, 0.08),
            pal.error,
            pal.error,
        ),
    };
    surface(ui, fill, Stroke::new(1.0, stroke_color), |ui| {
        // The action takes what it needs on the right; the texts wrap in what is left.
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 40.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                let inner = add_action(ui);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), 18.0, circle);
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        icon,
                        FontId::proportional(ICON_SIZE),
                        on_color(pal, circle),
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(title).size(17.0).color(pal.text));
                        if let Some(subtitle) = subtitle {
                            ui.label(RichText::new(subtitle).size(13.0).color(pal.text_secondary));
                        }
                    });
                });
                inner
            },
        )
        .inner
    })
}

/// A small card of `width` points with a label and an icon on top and a large value with an
/// optional unit below. The value and the unit are one label ("119 fps").
pub fn stat_tile(
    ui: &mut Ui,
    pal: Palette,
    icon: &str,
    label: &str,
    value: &str,
    unit: Option<&str>,
    width: f32,
) -> Response {
    let mut job = LayoutJob::default();
    job.append(
        value,
        0.0,
        TextFormat::simple(FontId::proportional(24.0), pal.text),
    );
    if let Some(unit) = unit {
        job.append(
            &format!(" {unit}"),
            0.0,
            TextFormat::simple(FontId::proportional(12.0), pal.text_secondary),
        );
    }
    Frame::new()
        .fill(pal.card)
        .stroke(border_stroke(pal))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            // The frame adds its margin and its 1 px stroke on both sides.
            ui.set_width((width - 26.0).max(0.0));
            ui.set_min_height(52.0);
            // The tile may sit in a row: stack its own contents.
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).size(12.0).color(pal.text_secondary));
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        ui.label(RichText::new(icon).size(16.0).color(pal.accent_text));
                    });
                });
                ui.label(job);
            });
        })
        .response
}

/// Width of a [`primary_button`] or [`secondary_button`] with this text.
pub fn button_width(ui: &Ui, text: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::proportional(14.0),
        Color32::PLACEHOLDER,
    );
    galley.size().x + 2.0 * ui.spacing().button_padding.x + 2.0
}

/// An accent-filled button for the main action.
pub fn primary_button(ui: &mut Ui, pal: Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(14.0).color(pal.on_accent))
            .fill(pal.accent)
            .stroke(Stroke::new(1.0, pal.accent))
            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
            .min_size(vec2(0.0, CONTROL_HEIGHT)),
    )
}

/// An outlined button for the other actions.
pub fn secondary_button(ui: &mut Ui, pal: Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(14.0).color(pal.text))
            .fill(pal.card)
            .stroke(border_stroke(pal))
            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
            .min_size(vec2(0.0, CONTROL_HEIGHT)),
    )
}

/// A top divider and a row of controls aligned to the right, such as the "Apply now" bar. Put
/// the bar in a panel without a margin: it adds its own.
pub fn footer_bar<R>(
    ui: &mut Ui,
    pal: Palette,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    divider(ui, pal);
    Frame::new()
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), CONTROL_HEIGHT),
                Layout::right_to_left(Align::Center),
                add_contents,
            )
            .inner
        })
}

#[cfg(test)]
mod tests;
