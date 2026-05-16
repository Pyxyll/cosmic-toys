//! libcosmic Application: the GUI window.
//!
//! Layout: a Cosmic-style sidebar nav (Picker / Settings / About) on the
//! left, page content on the right. The Picker page is the main view —
//! hero swatch + format readouts + history. Settings has the shortcut
//! binding and the autostart toggle. About is the standard libcosmic
//! about widget.

use crate::autostart;
use crate::color::PickedColor;
use crate::config::Config;
use crate::fl;
use crate::ipc;
use crate::shortcut;
use cosmic::Application;
use cosmic::app::{Core, Task};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::event;
use cosmic::iced::keyboard::{self, Key, key::Named};
use cosmic::iced::{Length, Subscription};
use std::time::{Duration, Instant};
use cosmic::prelude::*;
use cosmic::widget;
use cosmic::widget::nav_bar;

pub struct AppModel {
    pub(crate) core: Core,
    pub(crate) config: Config,
    /// Most recently picked color, displayed in the result view.
    pub(crate) picked: Option<PickedColor>,
    /// True while the overlay is running, used to debounce repeated clicks.
    pub(crate) picking: bool,
    /// Recent picks, newest first. Mirrored to `config.history` (persisted).
    pub(crate) history: Vec<PickedColor>,
    /// Sidebar navigation state.
    pub(crate) nav: nav_bar::Model,
    /// Cached "is autostart enabled?" so the toggle reflects on-disk truth.
    pub(crate) autostart_enabled: bool,
    /// Currently-bound shortcuts keyed by tool id. Missing entry = unbound.
    pub(crate) shortcut_current: std::collections::HashMap<String, String>,
    /// `Some(tool_id)` while the user is in "press a combo" mode for that
    /// tool; `None` when idle.
    pub(crate) capturing_shortcut: Option<String>,
    /// Feedback from the last shortcut save, scoped to a tool: `(tool_id,
    /// Ok(human))` on success, `(tool_id, Err(reason))` on parse/write
    /// failure, `None` while idle.
    pub(crate) shortcut_status: Option<(String, Result<String, String>)>,
    /// Most recently copied value + when. Used to flash the copy icon to a
    /// check mark for a brief window after a click. `None` once the
    /// feedback has been cleared.
    pub(crate) last_copied: Option<(String, Instant)>,
    /// Set when a new color landed in the hero card; the swatch fades in
    /// from `t = elapsed_since(start)`. `None` once the animation has run
    /// to completion (the Tick handler clears it so the subscription stops).
    pub(crate) hero_anim_start: Option<Instant>,
    /// Set when a new chip was prepended to Recents; chip[0] slides + fades
    /// in. Triggered only by PickResult, not SelectHistory.
    pub(crate) chip_anim_start: Option<Instant>,
    /// Which Settings sub-page is currently shown. Resets to `Hub` whenever
    /// the user navigates to the Settings sidebar entry.
    pub(crate) settings_subpage: SettingsSubPage,
}

const COPY_FEEDBACK_MS: u64 = 1500;
const PICK_ANIM_MS: u64 = 350;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    /// Color Picker tool.
    Picker,
    /// Find Mouse tool.
    MouseFind,
    /// Screen Ruler tool.
    ScreenRuler,
    Settings,
    About,
}

/// Drill-in state for the Settings sidebar entry. Each tool gets its own
/// sub-page; selecting Settings from the sidebar always returns to Hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSubPage {
    Hub,
    ColorPicker,
    MouseFind,
    ScreenRuler,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Oklch,
}

#[derive(Debug, Clone)]
pub enum Message {
    PickPressed,
    PickResult(Option<String>),
    Copy(String),
    /// Fires ~1.5s after a Copy to revert the checkmark feedback.
    ClearCopyFeedback,
    SelectHistory(usize),
    ClearHistory,
    UpdateConfig(Config),
    ToggleAutostart(bool),
    ToggleFormat(Format, bool),
    /// Click on a shortcut button — start listening for the next combo
    /// to bind to the named tool.
    BeginCaptureShortcut(String),
    /// Either a real keypress while capturing, or Esc to cancel. The
    /// target tool is read from `self.capturing_shortcut`.
    CaptureShortcut(Key, keyboard::Modifiers),
    OpenUrl(String),
    /// Drives entry animations for hero swatch + new recents chip.
    /// Self-clears the start fields once the elapsed time exceeds duration.
    Tick,
    /// "Test the spotlight" button on the Mouse Find page — triggers the
    /// daemon's find_mouse path. Same effect as binding a hotkey to it.
    MouseFindTriggered,
    /// "Test the ruler" button on the Screen Ruler page — triggers the
    /// daemon's screen_ruler path.
    ScreenRulerTriggered,
    /// Adjust one of the Find Mouse spotlight visuals from a slider.
    SetMouseFindField(MouseFindField, u32),
    /// Update the Find Mouse ring color from the hex input. Stored verbatim
    /// so an in-progress invalid string round-trips; the daemon parses on
    /// render and falls back to white if unparseable.
    SetMouseFindRingColor(String),
    /// "Reset to defaults" button on the Find Mouse settings card —
    /// snaps every spotlight field back to `Config::default()`.
    ResetMouseFindDefaults,
    /// Drill into / back-out of a Settings sub-page.
    ShowSettingsSubPage(SettingsSubPage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseFindField {
    Radius,
    RingThickness,
    RingAlpha,
    DimAlpha,
    Feather,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "com.pyxyll.CosmicToys";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Message>) {
        let config = cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
            .map(|ctx| match Config::get_entry(&ctx) {
                Ok(c) => c,
                Err((_e, c)) => c,
            })
            .unwrap_or_default();

        let history = parse_history(&config.history);
        let picked = history.first().copied();

        let mut nav = nav_bar::Model::default();
        nav.insert()
            .text(fl!("nav-picker"))
            .icon(widget::icon::from_name("color-select-symbolic"))
            .data::<Page>(Page::Picker)
            .activate();
        nav.insert()
            .text(fl!("nav-mouse-find"))
            .icon(widget::icon::from_name("input-mouse-symbolic"))
            .data::<Page>(Page::MouseFind);
        nav.insert()
            .text(fl!("nav-screen-ruler"))
            .icon(widget::icon::from_name("preferences-desktop-display-symbolic"))
            .data::<Page>(Page::ScreenRuler);
        nav.insert()
            .text(fl!("nav-settings"))
            .icon(widget::icon::from_name("preferences-system-symbolic"))
            .data::<Page>(Page::Settings);
        nav.insert()
            .text(fl!("nav-about"))
            .icon(widget::icon::from_name("help-about-symbolic"))
            .data::<Page>(Page::About);

        let app = AppModel {
            core,
            config,
            picked,
            picking: false,
            history,
            nav,
            autostart_enabled: autostart::is_enabled(),
            shortcut_current: load_all_shortcuts(),
            capturing_shortcut: None,
            shortcut_status: None,
            last_copied: None,
            hero_anim_start: None,
            chip_anim_start: None,
            settings_subpage: SettingsSubPage::Hub,
        };

        (app, Task::none())
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        self.nav.activate(id);
        // Refresh page-specific cached state on entry — covers external edits
        // to the autostart file or shortcut config since the GUI was opened.
        self.autostart_enabled = autostart::is_enabled();
        self.shortcut_current = load_all_shortcuts();
        // Leaving Settings mid-capture should cancel cleanly. Also reset
        // the drill-in so re-entering Settings always lands on the hub.
        self.capturing_shortcut = None;
        self.settings_subpage = SettingsSubPage::Hub;
        Task::none()
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        vec![widget::text::heading(fl!("app-title")).into()]
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        // Window controls (close/min/max) are provided by the compositor —
        // we don't add anything else here. Hide-from-the-header was a
        // workaround from the single-binary era and is unneeded now that
        // the daemon owns its own lifecycle.
        Vec::new()
    }

    fn view(&self) -> Element<'_, Message> {
        let page = self.nav.active_data::<Page>().copied().unwrap_or(Page::Picker);
        let body = match page {
            Page::Picker => crate::tools::color_picker::page(self),
            Page::MouseFind => crate::tools::mouse_find::page(self),
            Page::ScreenRuler => crate::tools::screen_ruler::page(self),
            Page::Settings => self.settings_page(),
            Page::About => self.about_page(),
        };

        widget::container(widget::scrollable(
            widget::container(body).padding([16, 24, 24, 24]).max_width(640),
        ))
        .center_x(Length::Fill)
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let config_sub = self
            .core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config));

        let mut subs = vec![config_sub];

        // Tick at ~60fps only while a pick-entry animation is still running.
        // Both timestamps self-clear in the Tick handler once expired, so
        // this gate flips back to false and the timer subscription drops.
        if self.hero_anim_start.is_some() || self.chip_anim_start.is_some() {
            subs.push(
                cosmic::iced::time::every(Duration::from_millis(16))
                    .map(|_| Message::Tick),
            );
        }

        if self.capturing_shortcut.is_some() {
            subs.push(event::listen_with(|e, _status, _window| match e {
                event::Event::Keyboard(keyboard::Event::KeyPressed {
                    key, modifiers, ..
                }) => Some(Message::CaptureShortcut(key, modifiers)),
                _ => None,
            }));
        }

        Subscription::batch(subs)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PickPressed => {
                if self.picking {
                    return Task::none();
                }
                self.picking = true;
                return Task::perform(
                    async {
                        if let Some(result) = ipc::request_pick().await {
                            return result;
                        }
                        tokio::task::spawn_blocking(|| {
                            let out = std::process::Command::new("cosmic-toysd")
                                .arg("--quiet")
                                .output()
                                .ok()?;
                            if !out.status.success() {
                                return None;
                            }
                            let s = String::from_utf8(out.stdout).ok()?;
                            let trimmed = s.trim();
                            if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed.to_string())
                            }
                        })
                        .await
                        .ok()
                        .flatten()
                    },
                    |hex| cosmic::Action::App(Message::PickResult(hex)),
                );
            }
            Message::PickResult(hex) => {
                self.picking = false;
                // The daemon already persisted this pick to the on-disk
                // history (both for IPC and one-shot paths). Don't write
                // again here — `watch_config` will deliver the change via
                // `UpdateConfig`, which is also where chip-anim triggers.
                // We only set `picked` + hero anim here for instant feedback
                // ahead of the watch round-trip.
                if let Some(picked) = hex.as_deref().and_then(PickedColor::from_hex) {
                    self.picked = Some(picked);
                    self.hero_anim_start = Some(Instant::now());
                }
            }
            Message::Copy(text) => {
                self.last_copied = Some((text.clone(), Instant::now()));
                let copy = cosmic::iced::clipboard::write::<cosmic::Action<Message>>(text);
                let clear = Task::perform(
                    async {
                        tokio::time::sleep(Duration::from_millis(COPY_FEEDBACK_MS)).await
                    },
                    |_| cosmic::Action::App(Message::ClearCopyFeedback),
                );
                return copy.chain(clear);
            }
            Message::ClearCopyFeedback => {
                // Only clear if the most-recent copy is now stale; ignore
                // strays from earlier rapid clicks (each Copy schedules its
                // own clear, but a fresh click moves the goalposts).
                if let Some((_, t)) = self.last_copied
                    && t.elapsed() >= Duration::from_millis(COPY_FEEDBACK_MS)
                {
                    self.last_copied = None;
                }
            }
            Message::SelectHistory(i) => {
                if let Some(p) = self.history.get(i).copied() {
                    self.picked = Some(p);
                }
            }
            Message::ClearHistory => {
                self.history.clear();
                self.save_history();
            }
            Message::UpdateConfig(c) => {
                let new_history = parse_history(&c.history);
                // Detect a fresh entry at the head (a pick happened — either
                // ours via PickResult, or a hotkey pick while the GUI was
                // open). Compare against the current head before swapping in.
                let head_changed = new_history.first() != self.history.first();
                self.config = c;
                self.history = new_history;
                let now = Instant::now();
                if let Some(top) = self.history.first().copied()
                    && Some(top) != self.picked
                {
                    self.picked = Some(top);
                    self.hero_anim_start = Some(now);
                }
                if head_changed && self.history.first().is_some() {
                    self.chip_anim_start = Some(now);
                }
            }
            Message::ToggleFormat(format, on) => {
                let app_id = <Self as cosmic::Application>::APP_ID;
                if let Ok(ctx) = cosmic_config::Config::new(app_id, Config::VERSION) {
                    let mut new_config = self.config.clone();
                    match format {
                        Format::Hex => new_config.format_hex = on,
                        Format::Rgb => new_config.format_rgb = on,
                        Format::Hsl => new_config.format_hsl = on,
                        Format::Hsv => new_config.format_hsv = on,
                        Format::Oklch => new_config.format_oklch = on,
                    }
                    let _ = new_config.write_entry(&ctx);
                    self.config = new_config;
                }
            }
            Message::ToggleAutostart(on) => {
                let result = if on {
                    autostart::enable()
                } else {
                    autostart::disable()
                };
                if let Err(e) = result {
                    eprintln!("color picker: autostart toggle failed: {e}");
                }
                self.autostart_enabled = autostart::is_enabled();
            }
            Message::BeginCaptureShortcut(tool_id) => {
                self.capturing_shortcut = Some(tool_id.clone());
                self.shortcut_status = None;
                // Temp-unbind THIS tool so the user's current combo doesn't
                // fire mid-capture. Other tools' bindings are untouched.
                // Restored on cancel; overwritten on real save.
                if let Err(e) = shortcut::clear(&tool_id) {
                    eprintln!("cosmic-toys: temp-unbind {tool_id} failed: {e}");
                }
            }
            Message::CaptureShortcut(key, modifiers) => {
                let Some(tool_id) = self.capturing_shortcut.clone() else {
                    return Task::none();
                };
                // Modifier keys on their own don't complete a binding —
                // wait for an actual key while the user holds them.
                if is_modifier_key(&key) {
                    return Task::none();
                }
                // Esc with no modifiers cancels the capture and restores
                // whatever we cleared on entry.
                if matches!(&key, Key::Named(Named::Escape)) && modifiers.is_empty() {
                    self.capturing_shortcut = None;
                    if let Some(prev) = self.shortcut_current.get(&tool_id).cloned()
                        && let Err(e) = shortcut::set_binding(&tool_id, &prev)
                    {
                        eprintln!("cosmic-toys: restore previous {tool_id} binding failed: {e}");
                    }
                    return Task::none();
                }
                self.capturing_shortcut = None;
                let combo = format_combo(modifiers, &key);
                if combo.is_empty() {
                    self.shortcut_status =
                        Some((tool_id.clone(), Err("Unsupported key".to_string())));
                    // Restore the binding we cleared so we're not left in a
                    // half-applied state.
                    if let Some(prev) = self.shortcut_current.get(&tool_id).cloned() {
                        let _ = shortcut::set_binding(&tool_id, &prev);
                    }
                    return Task::none();
                }
                self.shortcut_status =
                    Some((tool_id.clone(), match shortcut::set_binding(&tool_id, &combo) {
                        Ok(()) => {
                            self.shortcut_current.insert(tool_id, combo.clone());
                            Ok(combo)
                        }
                        Err(e) => Err(e),
                    }));
            }
            Message::OpenUrl(url) => {
                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
            }
            Message::Tick => {
                let done = Duration::from_millis(PICK_ANIM_MS);
                if matches!(self.hero_anim_start, Some(t) if t.elapsed() >= done) {
                    self.hero_anim_start = None;
                }
                if matches!(self.chip_anim_start, Some(t) if t.elapsed() >= done) {
                    self.chip_anim_start = None;
                }
            }
            Message::MouseFindTriggered => {
                // Fire-and-forget: ask the daemon to run find_mouse. No
                // response handling — the daemon's job is just to flash the
                // overlay and exit; nothing to come back to the GUI for.
                return Task::perform(
                    async {
                        ipc::request_run("find_mouse").await;
                    },
                    |_| cosmic::Action::None,
                );
            }
            Message::ScreenRulerTriggered => {
                return Task::perform(
                    async {
                        ipc::request_run("screen_ruler").await;
                    },
                    |_| cosmic::Action::None,
                );
            }
            Message::SetMouseFindField(field, value) => {
                let app_id = <Self as cosmic::Application>::APP_ID;
                if let Ok(ctx) = cosmic_config::Config::new(app_id, Config::VERSION) {
                    let mut new_config = self.config.clone();
                    match field {
                        MouseFindField::Radius => {
                            new_config.mouse_find_radius_px = value;
                        }
                        MouseFindField::RingThickness => {
                            new_config.mouse_find_ring_thickness_px = value;
                        }
                        MouseFindField::RingAlpha => {
                            new_config.mouse_find_ring_alpha = value.min(255) as u8;
                        }
                        MouseFindField::DimAlpha => {
                            new_config.mouse_find_dim_alpha = value.min(255) as u8;
                        }
                        MouseFindField::Feather => {
                            new_config.mouse_find_feather_px = value;
                        }
                    }
                    let _ = new_config.write_entry(&ctx);
                    self.config = new_config;
                }
            }
            Message::SetMouseFindRingColor(hex) => {
                let app_id = <Self as cosmic::Application>::APP_ID;
                if let Ok(ctx) = cosmic_config::Config::new(app_id, Config::VERSION) {
                    let mut new_config = self.config.clone();
                    new_config.mouse_find_ring_color = hex;
                    let _ = new_config.write_entry(&ctx);
                    self.config = new_config;
                }
            }
            Message::ShowSettingsSubPage(sub) => {
                self.settings_subpage = sub;
                // Cancel any in-flight shortcut capture when leaving a sub
                // (the capture binding belongs to the sub-page we're leaving).
                self.capturing_shortcut = None;
            }
            Message::ResetMouseFindDefaults => {
                let app_id = <Self as cosmic::Application>::APP_ID;
                if let Ok(ctx) = cosmic_config::Config::new(app_id, Config::VERSION) {
                    let defaults = Config::default();
                    let mut new_config = self.config.clone();
                    new_config.mouse_find_radius_px = defaults.mouse_find_radius_px;
                    new_config.mouse_find_ring_thickness_px = defaults.mouse_find_ring_thickness_px;
                    new_config.mouse_find_ring_alpha = defaults.mouse_find_ring_alpha;
                    new_config.mouse_find_dim_alpha = defaults.mouse_find_dim_alpha;
                    new_config.mouse_find_feather_px = defaults.mouse_find_feather_px;
                    new_config.mouse_find_ring_color = defaults.mouse_find_ring_color;
                    let _ = new_config.write_entry(&ctx);
                    self.config = new_config;
                }
            }
        }
        Task::none()
    }
}

impl AppModel {
    fn save_history(&self) {
        if let Ok(ctx) = cosmic_config::Config::new(Self::APP_ID, Config::VERSION) {
            let mut new_config = self.config.clone();
            new_config.history = self.history.iter().map(|p| p.hex()).collect();
            let _ = new_config.write_entry(&ctx);
        }
    }

    fn about_page(&self) -> Element<'_, Message> {
        // Embed the SVG bytes so the about page renders correctly even when
        // the binary runs from `target/release` before `just install` has
        // dropped the icon into the hicolor theme path.
        let icon_handle = widget::icon::from_svg_bytes(
            include_bytes!("../resources/com.pyxyll.CosmicToys.svg").as_slice(),
        );
        let app_icon = widget::icon::icon(icon_handle).size(96);

        let hero = widget::Column::new()
            .spacing(6)
            .align_x(cosmic::iced::Alignment::Center)
            .push(app_icon)
            .push(widget::text::title1(fl!("app-title")))
            .push(widget::text::caption(format!(
                "v{}",
                env!("CARGO_PKG_VERSION")
            )))
            .push(widget::text::body(fl!("about-tagline")));

        const REPO: &str = "https://github.com/Pyxyll/cosmic-toys";
        let link = |label: String, url: String| -> Element<'_, Message> {
            widget::button::link(label)
                .on_press(Message::OpenUrl(url))
                .into()
        };

        let links = widget::Row::new()
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center)
            .push(link(fl!("about-source"), REPO.to_string()))
            .push(widget::text::body("·"))
            .push(link(fl!("about-issues"), format!("{REPO}/issues")))
            .push(widget::text::body("·"))
            .push(link(
                fl!("about-license"),
                format!("{REPO}/blob/main/LICENSE"),
            ));

        widget::Column::new()
            .spacing(24)
            .align_x(cosmic::iced::Alignment::Center)
            .push(widget::container(hero).padding([24, 0, 0, 0]))
            .push(links)
            .push(widget::text::caption(fl!("about-copyright")))
            .into()
    }

    fn settings_page(&self) -> Element<'_, Message> {
        match self.settings_subpage {
            SettingsSubPage::Hub => self.settings_hub(),
            SettingsSubPage::ColorPicker => self.settings_sub_color_picker(),
            SettingsSubPage::MouseFind => self.settings_sub_mouse_find(),
            SettingsSubPage::ScreenRuler => self.settings_sub_screen_ruler(),
            SettingsSubPage::App => self.settings_sub_app(),
        }
    }

    /// Settings hub: clickable rows that drill into per-tool sub-pages
    /// plus an "App" row for app-wide preferences.
    fn settings_hub(&self) -> Element<'_, Message> {
        let row = |label: String, icon: &'static str, target: SettingsSubPage| -> Element<'_, Message> {
            widget::button::custom(
                widget::Row::new()
                    .spacing(12)
                    .align_y(cosmic::iced::Alignment::Center)
                    .push(widget::icon::from_name(icon).size(24))
                    .push(widget::text::body(label).width(Length::Fill))
                    .push(widget::icon::from_name("go-next-symbolic").size(16)),
            )
            .width(Length::Fill)
            .padding([12, 16])
            .class(cosmic::theme::style::Button::MenuItem)
            .on_press(Message::ShowSettingsSubPage(target))
            .into()
        };

        widget::settings::section()
            .title(fl!("settings-hub-title"))
            .add(row(
                fl!("settings-hub-color-picker"),
                "color-select-symbolic",
                SettingsSubPage::ColorPicker,
            ))
            .add(row(
                fl!("settings-hub-mouse-find"),
                "input-mouse-symbolic",
                SettingsSubPage::MouseFind,
            ))
            .add(row(
                fl!("settings-hub-screen-ruler"),
                "preferences-desktop-display-symbolic",
                SettingsSubPage::ScreenRuler,
            ))
            .add(row(
                fl!("settings-hub-app"),
                "preferences-system-symbolic",
                SettingsSubPage::App,
            ))
            .into()
    }

    /// Screen Ruler sub-page: its hotkey binder. Tool-specific visual
    /// config (line color, label background, etc.) is a v0.3.x add.
    fn settings_sub_screen_ruler(&self) -> Element<'_, Message> {
        let header = self.settings_sub_header(fl!("settings-hub-screen-ruler"));
        let shortcut =
            self.shortcut_section_for("screen_ruler", fl!("settings-shortcut-screen-ruler"));
        widget::Column::new()
            .spacing(16)
            .push(header)
            .push(shortcut)
            .into()
    }

    /// Color Picker sub-page: its hotkey binder + format toggles.
    fn settings_sub_color_picker(&self) -> Element<'_, Message> {
        let header = self.settings_sub_header(fl!("settings-hub-color-picker"));
        let shortcut =
            self.shortcut_section_for("color_picker", fl!("settings-shortcut-picker"));
        let formats = crate::tools::color_picker::settings_section(self);
        widget::Column::new()
            .spacing(16)
            .push(header)
            .push(shortcut)
            .push(formats)
            .into()
    }

    /// Find Mouse sub-page: its hotkey binder + spotlight visuals.
    fn settings_sub_mouse_find(&self) -> Element<'_, Message> {
        let header = self.settings_sub_header(fl!("settings-hub-mouse-find"));
        let shortcut =
            self.shortcut_section_for("find_mouse", fl!("settings-shortcut-mouse-find"));
        let spotlight = crate::tools::mouse_find::settings_section(self);
        widget::Column::new()
            .spacing(16)
            .push(header)
            .push(shortcut)
            .push(spotlight)
            .into()
    }

    /// App-level sub-page: autostart (and anywhere else we land app-wide
    /// prefs going forward).
    fn settings_sub_app(&self) -> Element<'_, Message> {
        let header = self.settings_sub_header(fl!("settings-hub-app"));
        let autostart = widget::settings::section()
            .title(fl!("settings-startup"))
            .add(widget::settings::item(
                fl!("settings-autostart"),
                widget::toggler(self.autostart_enabled).on_toggle(Message::ToggleAutostart),
            ))
            .add(widget::text::caption(fl!("settings-autostart-hint")));
        widget::Column::new()
            .spacing(16)
            .push(header)
            .push(autostart)
            .into()
    }

    /// "← Back" button + sub-page title, rendered above each sub-page's
    /// content.
    fn settings_sub_header(&self, title: String) -> Element<'_, Message> {
        widget::Row::new()
            .spacing(12)
            .align_y(cosmic::iced::Alignment::Center)
            .push(
                widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .extra_small()
                    .on_press(Message::ShowSettingsSubPage(SettingsSubPage::Hub)),
            )
            .push(widget::text::title3(title))
            .into()
    }

    fn shortcut_section_for<'a>(
        &'a self,
        tool_id: &'a str,
        title: String,
    ) -> Element<'a, Message> {
        let is_capturing = self.capturing_shortcut.as_deref() == Some(tool_id);
        let trailing: Element<'_, Message> = if is_capturing {
            widget::container(widget::text::body(fl!("shortcut-listening")))
                .padding([4, 12])
                .into()
        } else {
            let label = self
                .shortcut_current
                .get(tool_id)
                .cloned()
                .unwrap_or_else(|| fl!("shortcut-unset"));
            widget::button::standard(label)
                .on_press(Message::BeginCaptureShortcut(tool_id.to_string()))
                .into()
        };

        let mut col = widget::Column::new()
            .spacing(6)
            .push(widget::settings::item(fl!("shortcut-label"), trailing))
            .push(widget::text::caption(fl!("shortcut-hint")).width(Length::Fill));

        if let Some((status_tool, status)) = &self.shortcut_status
            && status_tool == tool_id
        {
            let line = match status {
                Ok(combo) => widget::text::caption(format!("✓  {combo}")),
                Err(e) => widget::text::caption(format!("✗  {e}")),
            };
            col = col.push(line);
        }

        widget::settings::section()
            .title(title)
            .add(col)
            .into()
    }

    pub(crate) fn is_recently_copied(&self, value: &str) -> bool {
        match &self.last_copied {
            Some((s, t)) => {
                s == value && t.elapsed() < Duration::from_millis(COPY_FEEDBACK_MS)
            }
            None => false,
        }
    }
}

/// 0..1 progress along an active animation. Returns 1.0 when `start` is
/// `None` — i.e. the animation isn't running, so callers should render the
/// final, fully-on state. Shared by tools that have entry/exit animations.
pub(crate) fn anim_progress(start: Option<Instant>) -> f32 {
    match start {
        Some(t) => (t.elapsed().as_millis() as f32 / PICK_ANIM_MS as f32).clamp(0.0, 1.0),
        None => 1.0,
    }
}

pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Read the currently-bound shortcut for each known tool from the COSMIC
/// shortcuts config. Tools without a binding are simply absent from the
/// returned map.
fn load_all_shortcuts() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for tool in ["color_picker", "find_mouse", "screen_ruler"] {
        if let Some(combo) = shortcut::current_binding(tool) {
            out.insert(tool.to_string(), combo);
        }
    }
    out
}

fn parse_history(raw: &[String]) -> Vec<PickedColor> {
    raw.iter()
        .filter_map(|s| PickedColor::from_hex(s))
        .collect()
}

fn is_modifier_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Named(
            Named::Shift
                | Named::Control
                | Named::Alt
                | Named::Super
                | Named::Meta
                | Named::AltGraph
                | Named::CapsLock
                | Named::NumLock
                | Named::ScrollLock
                | Named::Symbol
        )
    )
}

/// Format an iced (modifiers, key) pair into the human + Cosmic-config
/// form: `"Super+Shift+C"`. Returns empty string for keys we can't map
/// (e.g. dead keys, unidentified).
fn format_combo(mods: keyboard::Modifiers, key: &Key) -> String {
    let mut parts: Vec<String> = Vec::new();
    if mods.logo() {
        parts.push("Super".into());
    }
    if mods.control() {
        parts.push("Ctrl".into());
    }
    if mods.alt() {
        parts.push("Alt".into());
    }
    if mods.shift() {
        parts.push("Shift".into());
    }
    let key_str = match key {
        // iced delivers Space as Character(" "), not a Named variant.
        Key::Character(c) if c.as_str() == " " => "space".to_string(),
        Key::Character(c) => c.to_uppercase(),
        Key::Named(n) => match named_key_str(*n) {
            Some(s) => s.to_string(),
            None => return String::new(),
        },
        Key::Unidentified => return String::new(),
    };
    parts.push(key_str);
    parts.join("+")
}

/// Map iced's `Named` enum to the names Cosmic accepts in its shortcut
/// config. Anything not handled returns None which the caller treats as
/// "unsupported key".
fn named_key_str(n: Named) -> Option<&'static str> {
    Some(match n {
        Named::ArrowDown => "Down",
        Named::ArrowUp => "Up",
        Named::ArrowLeft => "Left",
        Named::ArrowRight => "Right",
        Named::Enter => "Return",
        Named::Escape => "Escape",
        Named::Tab => "Tab",
        Named::Backspace => "Backspace",
        Named::Delete => "Delete",
        Named::Insert => "Insert",
        Named::Home => "Home",
        Named::End => "End",
        Named::PageUp => "PageUp",
        Named::PageDown => "PageDown",
        Named::F1 => "F1",
        Named::F2 => "F2",
        Named::F3 => "F3",
        Named::F4 => "F4",
        Named::F5 => "F5",
        Named::F6 => "F6",
        Named::F7 => "F7",
        Named::F8 => "F8",
        Named::F9 => "F9",
        Named::F10 => "F10",
        Named::F11 => "F11",
        Named::F12 => "F12",
        Named::PrintScreen => "Print",
        _ => return None,
    })
}

