//! cosmic-toys-applet: a panel applet for the color picker.
//!
//! Lives next to the cosmic-toysd daemon and the cosmic-toys
//! GUI. The applet is purely a UI affordance: it doesn't run the overlay
//! itself or own any state. Picking is delegated to the daemon (via the
//! `cosmic-toysd --pick` subprocess) and history is read from the
//! same cosmic-config file the GUI writes to.

mod app;
mod color;
mod config;
mod i18n;

fn main() -> cosmic::iced::Result {
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);
    cosmic::applet::run::<app::AppModel>(())
}
