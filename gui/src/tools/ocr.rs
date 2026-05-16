//! OCR (Live Text) tool — page only for now; the overlay lives in
//! `daemon/src/ocr.rs`. The page is the marketing surface + a test
//! button that triggers the same code path the hotkey does.

use crate::app::{AppModel, Message};
use crate::fl;
use cosmic::iced::Length;
use cosmic::prelude::*;
use cosmic::widget;

pub fn page(_app: &AppModel) -> Element<'_, Message> {
    // Small "ALPHA" chip rendered next to the title so users see at a
    // glance that this tool is rougher than the rest.
    let alpha_chip = widget::container(widget::text::caption(fl!("ocr-alpha-chip")))
        .padding([2, 10])
        .class(cosmic::theme::style::Container::Card);
    let title = widget::Row::new()
        .spacing(10)
        .align_y(cosmic::iced::Alignment::Center)
        .push(widget::text::title3(fl!("ocr-title")))
        .push(alpha_chip);

    // Inline caveat callout so the limitations aren't a surprise once
    // the user actually fires it.
    let caveat = widget::container(
        widget::Column::new()
            .spacing(4)
            .push(widget::text::body(fl!("ocr-alpha-headline")))
            .push(widget::text::caption(fl!("ocr-alpha-body"))),
    )
    .padding(12)
    .width(Length::Fixed(480.0))
    .class(cosmic::theme::style::Container::Card);

    widget::container(
        widget::Column::new()
            .spacing(16)
            .align_x(cosmic::iced::Alignment::Center)
            .push(widget::icon::from_name("accessories-text-editor-symbolic").size(64))
            .push(title)
            .push(widget::text::body(fl!("ocr-body")))
            .push(caveat)
            .push(
                widget::button::standard(fl!("ocr-test-button"))
                    .on_press(Message::OcrTriggered),
            ),
    )
    .center_x(Length::Fill)
    .padding(48)
    .into()
}
