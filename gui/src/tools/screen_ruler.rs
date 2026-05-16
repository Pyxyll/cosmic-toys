//! Screen Ruler tool — page + (future) settings UI.
//!
//! Page is a description + "Test the ruler" button that dispatches the
//! daemon's screen_ruler path via IPC. The real overlay lives in
//! `daemon/src/screen_ruler.rs`.

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
