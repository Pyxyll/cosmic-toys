//! Color Picker tool.
//!
//! Renders the picker tool's main page (welcome state when no pick exists,
//! result view with hero swatch + format readouts + recents history when one
//! does) and the picker's section on the Settings page (per-format display
//! toggles).
//!
//! State (picked color, picking-in-flight flag, history, last-copy feedback,
//! hero/chip entry-animation timestamps) lives on `AppModel`. Free fns here
//! read it via `&AppModel`. When more tools land we may revisit and isolate
//! per-tool state into substructs; for now the direct-access shape keeps the
//! refactor scoped.

use crate::app::{AppModel, Format, Message, anim_progress, ease_out_cubic};
use crate::color::PickedColor;
use crate::fl;
use cosmic::iced::Length;
use cosmic::prelude::*;
use cosmic::widget;

/// The Color Picker tool's main page.
pub fn page(app: &AppModel) -> Element<'_, Message> {
    // Pick button lives inside the hero card / welcome view — the floating
    // header-row above the first card looked lonely and pushed the cards
    // too far down. The body owns its own action.
    match &app.picked {
        None => welcome_view(app),
        Some(p) => result_view(app, p),
    }
}

/// The Color Picker's section on the Settings page: per-format display
/// toggles. Shortcut binding + autostart stay app-level.
pub fn settings_section<'a>(app: &'a AppModel) -> Element<'a, Message> {
    widget::settings::section()
        .title(fl!("settings-formats"))
        .add(format_toggle_row("HEX", Format::Hex, app.config.format_hex))
        .add(format_toggle_row("RGB", Format::Rgb, app.config.format_rgb))
        .add(format_toggle_row("HSL", Format::Hsl, app.config.format_hsl))
        .add(format_toggle_row("HSV", Format::Hsv, app.config.format_hsv))
        .add(format_toggle_row("OKLCH", Format::Oklch, app.config.format_oklch))
        .into()
}

fn pick_icon_button(app: &AppModel) -> Element<'_, Message> {
    widget::button::icon(widget::icon::from_name("color-select-symbolic"))
        .large()
        .on_press_maybe((!app.picking).then_some(Message::PickPressed))
        .into()
}

fn welcome_view(app: &AppModel) -> Element<'_, Message> {
    widget::container(
        widget::Column::new()
            .spacing(16)
            .align_x(cosmic::iced::Alignment::Center)
            .push(widget::icon::from_name("color-select-symbolic").size(64))
            .push(widget::text::title3(fl!("welcome-title")))
            .push(widget::text::body(fl!("welcome-body")))
            .push(pick_icon_button(app)),
    )
    .center_x(Length::Fill)
    .padding(48)
    .into()
}

fn result_view<'a>(app: &'a AppModel, p: &PickedColor) -> Element<'a, Message> {
    let mut col = widget::Column::new()
        .spacing(16)
        .push(hero_card(app, p))
        .push(formats_card(app, p));
    if !app.history.is_empty() {
        col = col.push(history_card(app));
    }
    col.into()
}

fn hero_card<'a>(app: &'a AppModel, p: &PickedColor) -> Element<'a, Message> {
    // Hero swatch: fade-in via alpha when a fresh pick just landed.
    // No size animation here so the headline next to it doesn't reflow.
    let alpha = ease_out_cubic(anim_progress(app.hero_anim_start));
    let swatch = animated_color_block(p.rgb, 80.0, alpha);

    let icon_name = if app.is_recently_copied(&p.hex()) {
        "object-select-symbolic"
    } else {
        "edit-copy-symbolic"
    };
    let copy_hex = widget::button::icon(widget::icon::from_name(icon_name))
        .extra_small()
        .on_press(Message::Copy(p.hex()));

    let headline = widget::Row::new()
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center)
        .push(widget::text::title2(p.hex()))
        .push(copy_hex);

    // [swatch | hex + copy | (filler) | pick]
    let row = widget::Row::new()
        .spacing(16)
        .align_y(cosmic::iced::Alignment::Center)
        .push(swatch)
        .push(headline)
        .push(widget::Space::new().width(Length::Fill))
        .push(pick_icon_button(app));

    widget::container(row)
        .padding(14)
        .width(Length::Fill)
        .class(cosmic::theme::style::Container::Card)
        .into()
}

fn formats_card<'a>(app: &'a AppModel, p: &PickedColor) -> Element<'a, Message> {
    let mut section = widget::settings::section();
    if app.config.format_hex {
        let v = p.hex();
        let copied = app.is_recently_copied(&v);
        section = section.add(format_item(&fl!("format-hex"), v, copied));
    }
    if app.config.format_rgb {
        let v = p.rgb_str();
        let copied = app.is_recently_copied(&v);
        section = section.add(format_item(&fl!("format-rgb"), v, copied));
    }
    if app.config.format_hsl {
        let v = p.hsl_str();
        let copied = app.is_recently_copied(&v);
        section = section.add(format_item(&fl!("format-hsl"), v, copied));
    }
    if app.config.format_hsv {
        let v = p.hsv_str();
        let copied = app.is_recently_copied(&v);
        section = section.add(format_item(&fl!("format-hsv"), v, copied));
    }
    if app.config.format_oklch {
        let v = p.oklch_str();
        let copied = app.is_recently_copied(&v);
        section = section.add(format_item(&fl!("format-oklch"), v, copied));
    }
    section.into()
}

fn history_card(app: &AppModel) -> Element<'_, Message> {
    let mut strip = widget::Row::new().spacing(8);
    for (i, c) in app.history.iter().enumerate() {
        strip = strip.push(history_chip(app, i, c.rgb));
    }
    let strip_padded = widget::container(strip).padding([0, 0, 12, 0]);
    let scrollable_strip = widget::scrollable(strip_padded).direction(
        cosmic::iced::widget::scrollable::Direction::Horizontal(
            cosmic::iced::widget::scrollable::Scrollbar::new(),
        ),
    );

    let header = widget::Row::new()
        .align_y(cosmic::iced::Alignment::Center)
        .push(widget::text::heading(fl!("history-title")).width(Length::Fill))
        .push(
            widget::button::link(fl!("history-clear"))
                .on_press(Message::ClearHistory),
        );
    widget::container(
        widget::Column::new()
            .spacing(12)
            .push(header)
            .push(scrollable_strip),
    )
    .padding(20)
    .width(Length::Fill)
    .class(cosmic::theme::style::Container::Card)
    .into()
}

fn history_chip(app: &AppModel, idx: usize, rgb: (u8, u8, u8)) -> Element<'_, Message> {
    // Freshest entry slides + fades in: width grows from 0 to 36 (which
    // pushes older chips rightward, reading like a real insertion) and the
    // swatch alpha ramps in lockstep. Older chips render statically.
    let inner = if idx == 0 && app.chip_anim_start.is_some() {
        let p = ease_out_cubic(anim_progress(app.chip_anim_start));
        animated_color_block(rgb, 36.0 * p, p)
    } else {
        color_block(rgb, 36.0)
    };
    widget::button::custom(inner)
        .padding(0)
        .class(cosmic::theme::style::Button::Standard)
        .on_press(Message::SelectHistory(idx))
        .into()
}

fn color_block<'a>(rgb: (u8, u8, u8), size: f32) -> Element<'a, Message> {
    let color = cosmic::iced::Color::from_rgb8(rgb.0, rgb.1, rgb.2);
    widget::container(widget::Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(cosmic::theme::style::Container::custom(
            move |theme: &cosmic::Theme| {
                let cosmic = theme.cosmic();
                cosmic::iced::widget::container::Style {
                    background: Some(color.into()),
                    border: cosmic::iced::Border {
                        radius: cosmic.corner_radii.radius_s.into(),
                        width: 1.0,
                        color: cosmic.background.divider.into(),
                    },
                    ..Default::default()
                }
            },
        ))
        .into()
}

/// Variant of `color_block` whose fill + border alpha are scaled by `alpha`.
/// Used by the hero swatch and the freshest recents chip during entry anim.
fn animated_color_block<'a>(rgb: (u8, u8, u8), size: f32, alpha: f32) -> Element<'a, Message> {
    let mut color = cosmic::iced::Color::from_rgb8(rgb.0, rgb.1, rgb.2);
    color.a = alpha;
    widget::container(widget::Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(cosmic::theme::style::Container::custom(
            move |theme: &cosmic::Theme| {
                let cosmic = theme.cosmic();
                let mut border_color: cosmic::iced::Color = cosmic.background.divider.into();
                border_color.a *= alpha;
                cosmic::iced::widget::container::Style {
                    background: Some(color.into()),
                    border: cosmic::iced::Border {
                        radius: cosmic.corner_radii.radius_s.into(),
                        width: 1.0,
                        color: border_color,
                    },
                    ..Default::default()
                }
            },
        ))
        .into()
}

fn format_toggle_row<'a>(label: &str, kind: Format, on: bool) -> Element<'a, Message> {
    widget::settings::item(
        label.to_string(),
        widget::toggler(on).on_toggle(move |v| Message::ToggleFormat(kind, v)),
    )
    .into()
}

/// A settings-list row: label on the left, monospace value, copy icon button.
/// `copied=true` swaps the copy icon for a checkmark to confirm the click.
fn format_item<'a>(label: &str, value: String, copied: bool) -> Element<'a, Message> {
    let value_for_copy = value.clone();
    let icon_name = if copied {
        "object-select-symbolic"
    } else {
        "edit-copy-symbolic"
    };
    let trailing = widget::Row::new()
        .spacing(12)
        .align_y(cosmic::iced::Alignment::Center)
        .push(widget::text::monotext(value))
        .push(
            widget::button::icon(widget::icon::from_name(icon_name))
                .extra_small()
                .on_press(Message::Copy(value_for_copy)),
        );
    widget::settings::item(label.to_string(), trailing).into()
}
