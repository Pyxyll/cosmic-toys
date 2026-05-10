//! Find Mouse tool.
//!
//! v0.3.0 ships with a stub overlay (see `daemon/src/find_mouse.rs`) — the
//! page here is the marketing surface: title, description, a "Test the
//! spotlight" button that hits the daemon's find_mouse path so users can
//! verify their hotkey before relying on it.

use crate::app::{AppModel, Message, MouseFindField};
use crate::fl;
use cosmic::iced::Length;
use cosmic::prelude::*;
use cosmic::widget;

pub fn page(_app: &AppModel) -> Element<'_, Message> {
    widget::container(
        widget::Column::new()
            .spacing(16)
            .align_x(cosmic::iced::Alignment::Center)
            .push(widget::icon::from_name("input-mouse-symbolic").size(64))
            .push(widget::text::title3(fl!("mouse-find-title")))
            .push(widget::text::body(fl!("mouse-find-body")))
            .push(
                widget::button::standard(fl!("mouse-find-pick-button"))
                    .on_press(Message::MouseFindTriggered),
            ),
    )
    .center_x(Length::Fill)
    .padding(48)
    .into()
}

/// Spotlight visuals: five sliders that write to cosmic-config; the
/// daemon reads each field at the start of every find_mouse run.
pub fn settings_section<'a>(app: &'a AppModel) -> Element<'a, Message> {
    let cfg = &app.config;

    let row = |label: String, value: u32, range: std::ops::RangeInclusive<u32>, field: MouseFindField| {
        let value_label = widget::text::monotext(format!("{value}"));
        let slider = widget::slider(range, value, move |v| {
            Message::SetMouseFindField(field, v)
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

    widget::settings::section()
        .title(fl!("settings-mouse-find"))
        .add(row(
            fl!("mouse-find-radius"),
            cfg.mouse_find_radius_px,
            40..=200,
            MouseFindField::Radius,
        ))
        .add(row(
            fl!("mouse-find-ring-thickness"),
            cfg.mouse_find_ring_thickness_px,
            0..=12,
            MouseFindField::RingThickness,
        ))
        .add(row(
            fl!("mouse-find-ring-alpha"),
            cfg.mouse_find_ring_alpha as u32,
            0..=255,
            MouseFindField::RingAlpha,
        ))
        .add(row(
            fl!("mouse-find-dim-alpha"),
            cfg.mouse_find_dim_alpha as u32,
            0..=255,
            MouseFindField::DimAlpha,
        ))
        .add(row(
            fl!("mouse-find-feather"),
            cfg.mouse_find_feather_px,
            0..=64,
            MouseFindField::Feather,
        ))
        .into()
}

