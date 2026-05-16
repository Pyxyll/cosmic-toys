//! Screen Ruler tool — on-screen pixel measurement.
//!
//! UX (matches PowerToys Screen Ruler's measure + bounds modes):
//! - Hotkey activates a fullscreen overlay with a faint crosshair
//!   following the cursor.
//! - Click + drag (no modifier) → line from press point to cursor; the
//!   pixel distance is shown live next to the cursor.
//! - Click + drag with Shift held → rectangle instead of a line; the
//!   label shows `W × H px`.
//! - Release → the measurement persists on screen until the next click
//!   or Esc.
//! - Esc → exit.
//!
//! v0.3.0 first cut is a stub — overlay rendering lands in the next
//! commit alongside the SCTK + font wiring (same shape as the
//! find_mouse overlay).

use std::io;
use std::time::Duration;

pub fn show() -> io::Result<()> {
    eprintln!("cosmic-toysd: screen_ruler stub fired (overlay rendering TODO)");
    std::thread::sleep(Duration::from_millis(400));
    Ok(())
}
