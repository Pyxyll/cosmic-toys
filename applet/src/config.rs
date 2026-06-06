use cosmic::cosmic_config::{CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

/// Subset of the GUI's `com.pyxyll.CosmicToys` config that the applet cares
/// about. Only the fields declared here are read; the GUI owns writing them.
/// The `applet_show_*` flags decide which tool launchers the popup renders —
/// the GUI's "Panel Applet" settings section is the single writer.
///
/// `Default` is hand-written (not derived) because the derived default would
/// make `applet_show_color_picker` false, leaving a fresh install — one that
/// has never opened the GUI settings, so none of these files exist on disk —
/// with an empty popup. These defaults MUST match `gui/src/config.rs`.
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    pub history: Vec<String>,
    pub applet_show_color_picker: bool,
    pub applet_show_find_mouse: bool,
    pub applet_show_screen_ruler: bool,
    pub applet_show_ocr: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            applet_show_color_picker: true,
            applet_show_find_mouse: false,
            applet_show_screen_ruler: false,
            applet_show_ocr: false,
        }
    }
}
