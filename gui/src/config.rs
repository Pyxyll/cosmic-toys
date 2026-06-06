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
    /// Screen Ruler: measurement line / rectangle stroke thickness, px.
    pub screen_ruler_line_thickness_px: u32,
    /// Screen Ruler line + endpoint + rect color as `#RRGGBB`.
    pub screen_ruler_line_color: String,
    /// Alpha (0..255) of the faint crosshair following the cursor when
    /// no drag is in progress.
    pub screen_ruler_crosshair_alpha: u8,
    /// Magnifier loupe zoom factor (pixels per source pixel).
    pub screen_ruler_magnifier_zoom: u32,
    /// If true, the magnifier loupe is on the moment the overlay opens;
    /// otherwise the user has to press M to enable it.
    pub screen_ruler_magnifier_default: bool,
    /// One of `"solid"`, `"dotted"`, `"dashed"`. Daemon falls back to
    /// solid for any other value.
    pub screen_ruler_line_style: String,
    /// Snap a Shift-held drag to the nearest of these angle groups. The
    /// four groups together cover the seven canonical angles
    /// 0/15/30/45/60/75/90; toggle a group to include / exclude all of
    /// its angles. With every group off, Shift has no effect on the line.
    pub screen_ruler_snap_cardinals: bool, // 0°, 90°
    pub screen_ruler_snap_diagonals: bool, // 45°
    pub screen_ruler_snap_thirds: bool,    // 30°, 60°
    pub screen_ruler_snap_octants: bool,   // 15°, 75°
    /// Panel applet: which tool launchers appear in the applet popup. The
    /// applet reads these from this shared namespace (the same place it
    /// already reads `history`), so a single config file drives all three
    /// components. Color Picker defaults on so the applet looks exactly like
    /// v0.2.x out of the box; the rest are opt-in. Adding these to the
    /// existing `#[version = 1]` struct is backward-compatible — cosmic-config
    /// stores one file per field, so an upgrader missing these files just
    /// falls back to the defaults below.
    pub applet_show_color_picker: bool,
    pub applet_show_find_mouse: bool,
    pub applet_show_screen_ruler: bool,
    pub applet_show_ocr: bool,
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
            screen_ruler_line_thickness_px: 2,
            screen_ruler_line_color: "#FFFFFF".to_string(),
            screen_ruler_crosshair_alpha: 90,
            screen_ruler_magnifier_zoom: 8,
            screen_ruler_magnifier_default: false,
            screen_ruler_line_style: "solid".to_string(),
            screen_ruler_snap_cardinals: true,
            screen_ruler_snap_diagonals: true,
            screen_ruler_snap_thirds: false,
            screen_ruler_snap_octants: false,
            applet_show_color_picker: true,
            applet_show_find_mouse: false,
            applet_show_screen_ruler: false,
            applet_show_ocr: false,
        }
    }
}
