//! "Find Mouse" tool — brief overlay that draws the eye to the cursor.
//!
//! v0.3.0 ships with a stub: the IPC + CLI + GUI sidebar plumbing all work
//! end-to-end (so binding a hotkey to `cosmic-toys run find_mouse` reaches
//! `show()` here), but the actual layer-shell overlay rendering is TODO.
//! The function blocks for `DURATION_MS` so callers see a "real" timing,
//! but no pixels reach the screen yet.
//!
//! v0.3.x will replace the stub with the proper overlay: one fullscreen
//! layer-shell surface per output, dim background, circular cutout at the
//! cursor position, animated expanding ring, dismiss on motion / Esc /
//! timeout. See `daemon/src/overlay.rs` for the SCTK pattern to mirror.

use std::io;
use std::time::Duration;

const DURATION_MS: u64 = 600;

pub fn show() -> io::Result<()> {
    eprintln!("cosmic-toysd: find_mouse stub fired (overlay rendering TODO in v0.3.x)");
    std::thread::sleep(Duration::from_millis(DURATION_MS));
    Ok(())
}
