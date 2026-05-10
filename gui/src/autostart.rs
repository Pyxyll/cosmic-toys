//! XDG autostart entry: a small `.desktop` file dropped into
//! `~/.config/autostart/` that the desktop session honours at login.
//!
//! We use this rather than a systemd user service because the XDG path is
//! the cross-DE convention and Cosmic respects it natively. The `.desktop`
//! launches the binary with `--background`, so the window stays hidden
//! while the IPC daemon lives.

use std::io;
use std::path::PathBuf;

const FILENAME: &str = "com.pyxyll.CosmicToys.desktop";

fn autostart_dir() -> PathBuf {
    dirs_path().join("autostart")
}

fn dirs_path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        })
}

pub fn entry_path() -> PathBuf {
    autostart_dir().join(FILENAME)
}

pub fn is_enabled() -> bool {
    entry_path().exists()
}

pub fn enable() -> io::Result<()> {
    std::fs::create_dir_all(autostart_dir())?;
    // `Exec=cosmic-toys` resolves via PATH; ~/.local/bin is in the
    // standard user PATH on Cosmic. If a user installs system-wide, the
    // binary is still picked up the same way.
    let body = "[Desktop Entry]\n\
Type=Application\n\
Name=Cosmic Color Picker\n\
Comment=Native Wayland color picker for COSMIC\n\
Exec=cosmic-toys --background\n\
Icon=color-select-symbolic\n\
NoDisplay=true\n\
X-GNOME-Autostart-enabled=true\n";
    std::fs::write(entry_path(), body)
}

pub fn disable() -> io::Result<()> {
    match std::fs::remove_file(entry_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
