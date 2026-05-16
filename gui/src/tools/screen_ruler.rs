//! Screen Ruler tool — page + (future) settings UI.
//!
//! Page is a description + "Test the ruler" button that dispatches the
//! daemon's screen_ruler path via IPC. The real overlay lives in
//! `daemon/src/screen_ruler.rs`.

use crate::app::{AppModel, Message, ScreenRulerField, SnapGroup};
use crate::fl;
use cosmic::iced::Length;
use cosmic::prelude::*;
use cosmic::widget;

pub fn page(_app: &AppModel) -> Element<'_, Message> {
    widget::container(
        widget::Column::new()
            .spacing(16)
            .align_x(cosmic::iced::Alignment::Center)
            .push(widget::icon::from_name("preferences-desktop-display-symbolic").size(64))
            .push(widget::text::title3(fl!("screen-ruler-title")))
            .push(widget::text::body(fl!("screen-ruler-body")))
            .push(
                widget::button::standard(fl!("screen-ruler-test-button"))
                    .on_press(Message::ScreenRulerTriggered),
            ),
    )
    .center_x(Length::Fill)
    .padding(48)
    .into()
}

/// Screen Ruler settings: visual knobs for the line + crosshair +
/// magnifier loupe, plus reset. Daemon reads each field on the fly on
/// the next overlay invocation (no live update during an active drag).
pub fn settings_section<'a>(app: &'a AppModel) -> Element<'a, Message> {
    let cfg = &app.config;

    let slider_row = |label: String,
                      value: u32,
                      range: std::ops::RangeInclusive<u32>,
                      field: ScreenRulerField| {
        let value_label = widget::text::monotext(format!("{value}"));
        let slider = widget::slider(range, value, move |v| {
            Message::SetScreenRulerField(field, v)
        });
        widget::settings::item(
            label,
            widget::Row::new()
                .spacing(12)
                .align_y(cosmic::iced::Alignment::Center)
                .push(slider.width(Length::Fixed(180.0)))
                .push(value_label),
        )
    };

    let color_input = widget::text_input(
        "#FFFFFF",
        cfg.screen_ruler_line_color.clone(),
    )
    .on_input(Message::SetScreenRulerLineColor)
    .width(Length::Fixed(180.0));
    let color_row = widget::settings::item(
        fl!("screen-ruler-line-color"),
        widget::Row::new()
            .spacing(12)
            .align_y(cosmic::iced::Alignment::Center)
            .push(color_input),
    );

    let magnifier_toggle = widget::settings::item(
        fl!("screen-ruler-magnifier-default"),
        widget::toggler(cfg.screen_ruler_magnifier_default)
            .on_toggle(Message::SetScreenRulerMagnifierDefault),
    );

    // Three line styles. Mapped via a stable index so the dropdown stays
    // simple; persistence is by string for human-readable on-disk config.
    const STYLE_KEYS: [&str; 3] = ["solid", "dotted", "dashed"];
    let style_labels = vec![
        fl!("screen-ruler-style-solid"),
        fl!("screen-ruler-style-dotted"),
        fl!("screen-ruler-style-dashed"),
    ];
    let selected_idx = STYLE_KEYS
        .iter()
        .position(|s| *s == cfg.screen_ruler_line_style)
        .unwrap_or(0);
    let style_dropdown = widget::dropdown(style_labels, Some(selected_idx), |idx| {
        Message::SetScreenRulerLineStyle(STYLE_KEYS.get(idx).copied().unwrap_or("solid").into())
    });
    let style_row = widget::settings::item(fl!("screen-ruler-line-style"), style_dropdown);

    let reset_row = widget::Row::new()
        .padding(8)
        .align_y(cosmic::iced::Alignment::Center)
        .push(widget::Space::new().width(Length::Fill))
        .push(
            widget::button::standard(fl!("screen-ruler-reset"))
                .on_press(Message::ResetScreenRulerDefaults),
        );

    widget::settings::section()
        .title(fl!("settings-screen-ruler"))
        .add(slider_row(
            fl!("screen-ruler-line-thickness"),
            cfg.screen_ruler_line_thickness_px,
            1..=6,
            ScreenRulerField::LineThickness,
        ))
        .add(color_row)
        .add(slider_row(
            fl!("screen-ruler-crosshair-alpha"),
            cfg.screen_ruler_crosshair_alpha as u32,
            0..=255,
            ScreenRulerField::CrosshairAlpha,
        ))
        .add(slider_row(
            fl!("screen-ruler-magnifier-zoom"),
            cfg.screen_ruler_magnifier_zoom,
            4..=16,
            ScreenRulerField::MagnifierZoom,
        ))
        .add(style_row)
        .add(magnifier_toggle)
        .add(snap_toggle(
            fl!("screen-ruler-snap-cardinals"),
            cfg.screen_ruler_snap_cardinals,
            SnapGroup::Cardinals,
        ))
        .add(snap_toggle(
            fl!("screen-ruler-snap-diagonals"),
            cfg.screen_ruler_snap_diagonals,
            SnapGroup::Diagonals,
        ))
        .add(snap_toggle(
            fl!("screen-ruler-snap-thirds"),
            cfg.screen_ruler_snap_thirds,
            SnapGroup::Thirds,
        ))
        .add(snap_toggle(
            fl!("screen-ruler-snap-octants"),
            cfg.screen_ruler_snap_octants,
            SnapGroup::Octants,
        ))
        .add(reset_row)
        .into()
}

fn snap_toggle<'a>(label: String, on: bool, group: SnapGroup) -> cosmic::Element<'a, Message> {
    widget::settings::item(
        label,
        widget::toggler(on).on_toggle(move |v| Message::SetScreenRulerSnapGroup(group, v)),
    )
    .into()
}
