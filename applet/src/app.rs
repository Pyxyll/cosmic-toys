//! Panel applet: small icon + popup with Pick button, recent chips,
//! and a link that launches the full GUI.
//!
//! State management is minimal: the daemon owns history + picks, and we
//! subscribe to the same cosmic-config file the GUI watches so the popup
//! reflects fresh picks without us doing any work.

use crate::color::PickedColor;
use crate::config::Config;
use crate::fl;
use cosmic::app::{Core, Task};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::window::Id;
use cosmic::iced::{Length, Limits, Subscription};
use std::time::Duration;
use cosmic::prelude::*;
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget;

const APP_ID: &str = "com.pyxyll.CosmicToysApplet";
/// We share the same config namespace as the daemon and GUI so a single
/// history list is the source of truth across all three components.
const HISTORY_APP_ID: &str = "com.pyxyll.CosmicToys";
const HISTORY_LIMIT_DISPLAYED: usize = 8;

#[derive(Default)]
pub struct AppModel {
    core: Core,
    popup: Option<Id>,
    history: Vec<PickedColor>,
    /// True between clicking "Pick" and PopupClosed firing. The subprocess
    /// is launched in the PopupClosed handler so the panel popup is gone
    /// before the picker overlay shows up — otherwise they'd overlap on
    /// the same Wayland top layer for a frame or two.
    pending_pick: bool,
}

// NOTE: auto-reopening the popup after a pick was attempted but doesn't
// work cleanly. Layer-shell popups need a "grab serial" from a recent
// pointer/keyboard input event; opening one from a delayed task (after
// the picker subprocess exits) has no such serial and the compositor
// silently drops the request. Clippy-land hits the same wall and works
// around it with a top-anchored layer surface — too much complexity for
// a nice-to-have. The daemon's notification + clipboard delivery already
// communicates "pick succeeded"; if the user wants to see the chip strip
// they click the panel icon. Revisit in D6 when format-config lands.

#[derive(Debug, Clone)]
pub enum Message {
    Surface(cosmic::surface::Action),
    PopupClosed(Id),
    UpdateHistory(Config),
    PickPressed,
    /// Fired ~150ms after PopupClosed when a pick was pending. The delay
    /// gives the compositor time to fully tear down the popup surface
    /// before the picker overlay grabs the top layer, otherwise the panel
    /// popup is briefly visible behind the overlay.
    SpawnPicker,
    SelectHistory(usize),
    OpenGui,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let history = cosmic_config::Config::new(HISTORY_APP_ID, Config::VERSION)
            .ok()
            .and_then(|ctx| Config::get_entry(&ctx).ok())
            .map(|c| parse_history(&c.history))
            .unwrap_or_default();

        let app = AppModel {
            core,
            popup: None,
            history,
            pending_pick: false,
        };
        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        // Watch the *daemon's* config (same APP_ID as the GUI) so when a
        // pick lands the chip strip refreshes without explicit polling.
        self.core()
            .watch_config::<Config>(HISTORY_APP_ID)
            .map(|update| Message::UpdateHistory(update.config))
    }

    fn view(&self) -> Element<'_, Message> {
        let popup_id = self.popup;
        self.core
            .applet
            .icon_button("color-select-symbolic")
            .on_press_with_rectangle(move |_offset, _bounds| {
                if let Some(id) = popup_id {
                    Message::Surface(destroy_popup(id))
                } else {
                    Message::Surface(app_popup::<AppModel>(
                        |state: &mut AppModel| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);
                            let mut popup_settings = state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
                                new_id,
                                None,
                                None,
                                None,
                            );
                            popup_settings.positioner.size_limits = Limits::NONE
                                .max_width(360.0)
                                .min_width(280.0)
                                .min_height(160.0)
                                .max_height(420.0);
                            popup_settings
                        },
                        None,
                    ))
                }
            })
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let pick = widget::button::suggested(fl!("pick-button"))
            .on_press(Message::PickPressed)
            .width(Length::Fill);

        let recent: Element<'_, Message> = if self.history.is_empty() {
            widget::text::caption(fl!("empty-hint"))
                .width(Length::Fill)
                .into()
        } else {
            let mut row = widget::Row::new().spacing(6);
            for (i, c) in self
                .history
                .iter()
                .take(HISTORY_LIMIT_DISPLAYED)
                .enumerate()
            {
                row = row.push(self.chip(i, c.rgb));
            }
            widget::Column::new()
                .spacing(6)
                .push(widget::text::heading(fl!("recent")))
                .push(row)
                .into()
        };

        let open = widget::button::link(fl!("open-app"))
            .on_press(Message::OpenGui);

        let content = widget::Column::new()
            .padding(12)
            .spacing(12)
            .push(pick)
            .push(recent)
            .push(open);

        self.core.applet.popup_container(content).into()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Surface(a) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(a),
                ));
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
                if self.pending_pick {
                    self.pending_pick = false;
                    return Task::perform(
                        async { tokio::time::sleep(Duration::from_millis(150)).await },
                        |_| cosmic::Action::App(Message::SpawnPicker),
                    );
                }
            }
            Message::SpawnPicker => {
                let _ = std::process::Command::new("cosmic-toysd")
                    .arg("--pick")
                    .spawn();
            }
            Message::UpdateHistory(c) => {
                self.history = parse_history(&c.history);
            }
            Message::PickPressed => {
                self.pending_pick = true;
                return self.close_popup();
            }
            Message::SelectHistory(i) => {
                if let Some(c) = self.history.get(i) {
                    let hex = c.hex();
                    let copy = cosmic::iced::clipboard::write::<cosmic::Action<Message>>(hex);
                    return copy.chain(self.close_popup());
                }
            }
            Message::OpenGui => {
                let _ = std::process::Command::new("cosmic-toys").spawn();
                return self.close_popup();
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl AppModel {
    fn close_popup(&mut self) -> Task<Message> {
        if let Some(id) = self.popup.take() {
            cosmic::task::message(cosmic::Action::Cosmic(
                cosmic::app::Action::Surface(destroy_popup(id)),
            ))
        } else {
            Task::none()
        }
    }

    fn chip(&self, idx: usize, rgb: (u8, u8, u8)) -> Element<'_, Message> {
        widget::button::custom(self.color_block(rgb, 32.0))
            .padding(0)
            .class(cosmic::theme::style::Button::Standard)
            .on_press(Message::SelectHistory(idx))
            .into()
    }

    fn color_block(&self, rgb: (u8, u8, u8), size: f32) -> Element<'_, Message> {
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
}

fn parse_history(raw: &[String]) -> Vec<PickedColor> {
    raw.iter().filter_map(|s| PickedColor::from_hex(s)).collect()
}
