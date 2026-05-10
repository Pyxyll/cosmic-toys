//! Persistent configuration stored via cosmic-config.

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// Recent picks as hex strings (`#RRGGBB`), newest first. Capped at the
    /// limit defined in `app.rs`. Stored as strings rather than packed ints
    /// so the on-disk config file stays human-readable and editable.
    pub history: Vec<String>,
    /// Per-format toggles for the result view. Defaults match PowerToys
    /// (HEX/RGB/HSL/HSV on, OKLCH off). Order in the UI is fixed; users who
    /// want a different order can edit the file directly.
    pub format_hex: bool,
    pub format_rgb: bool,
    pub format_hsl: bool,
    pub format_hsv: bool,
    pub format_oklch: bool,
    /// Find Mouse: spotlight cutout radius in pixels. The bright ring sits
    /// just inside this radius; everything beyond fades to dim.
    pub mouse_find_radius_px: u32,
    /// Width of the bright ring at the cutout boundary, pixels.
    pub mouse_find_ring_thickness_px: u32,
    /// Alpha of the bright ring (0 = invisible, 255 = solid white).
    pub mouse_find_ring_alpha: u8,
    /// Alpha of the dim wash outside the cutout (0 = no dim, 255 = black).
    pub mouse_find_dim_alpha: u8,
    /// Soft-edge transition width between the cutout and the dim, pixels.
    pub mouse_find_feather_px: u32,
    /// Ring color as hex `#RRGGBB`. Defaults to white. Plan to add an
    /// "Use COSMIC accent color" option in a follow-up that snaps this
    /// to the current desktop accent.
    pub mouse_find_ring_color: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            format_hex: true,
            format_rgb: true,
            format_hsl: true,
            format_hsv: true,
            format_oklch: false,
            mouse_find_radius_px: 90,
            mouse_find_ring_thickness_px: 4,
            mouse_find_ring_alpha: 220,
            mouse_find_dim_alpha: 140,
            mouse_find_feather_px: 28,
            mouse_find_ring_color: "#FFFFFF".to_string(),
        }
    }
}
