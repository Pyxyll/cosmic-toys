//! Find Mouse tool.
//!
//! v0.3.0 ships with a stub overlay (see `daemon/src/find_mouse.rs`) — the
//! page here is the marketing surface: title, description, a "Test the
//! spotlight" button that hits the daemon's find_mouse path so users can
//! verify their hotkey before relying on it.

use crate::app::{AppModel, Message};
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

