//! cosmic-toys: the GUI app.
//!
//! D0 architecture: the overlay code lives in the `cosmic-toysd`
//! daemon binary now. The GUI talks to the daemon when one is running
//! (via the IPC socket); when no daemon is reachable it falls back to
//! spawning `cosmic-toysd` as a one-shot subprocess. D1 extends
//! the daemon to be long-running with proper IPC; D2 wires the GUI's
//! Pick button through that IPC instead of subprocess spawn.

mod app;
mod autostart;
mod color;
mod config;
mod i18n;
mod ipc;
mod shortcut;

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

/// Best-effort one-time copy of pre-rename state from
/// `com.pyxyll.CosmicColorPicker` (v0.1 / v0.2.x) to the new
/// `com.pyxyll.CosmicToys` namespace. Idempotent — bails if the new
/// dir already exists, so subsequent launches are no-ops.
///
/// The old autostart entry (if any) is removed, not converted; users
/// re-toggle from Settings since the binary path inside the .desktop
/// also needs to change.
fn migrate_legacy_state() {
    let xdg_config = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });

    let old_dir = xdg_config.join("cosmic/com.pyxyll.CosmicColorPicker/v1");
    let new_dir = xdg_config.join("cosmic/com.pyxyll.CosmicToys/v1");
    if !new_dir.exists() && old_dir.exists() {
        let _ = std::fs::create_dir_all(&new_dir);
        if let Ok(entries) = std::fs::read_dir(&old_dir) {
            for entry in entries.flatten() {
                let _ = std::fs::copy(entry.path(), new_dir.join(entry.file_name()));
            }
        }
    }

    let old_autostart = xdg_config.join("autostart/com.pyxyll.CosmicColorPicker.desktop");
    if old_autostart.exists() {
        let _ = std::fs::remove_file(&old_autostart);
    }
}

#[derive(Debug, Default)]
struct CliFlags {
    pick: bool,
}

fn parse_args() -> Result<CliFlags, ExitCode> {
    let mut flags = CliFlags::default();
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--pick" => flags.pick = true,
            "-h" | "--help" => {
                print_help();
                return Err(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("cosmic-toys {}", env!("CARGO_PKG_VERSION"));
                return Err(ExitCode::SUCCESS);
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(flags)
}

fn print_help() {
    println!("Usage: cosmic-toys [--pick]");
    println!();
    println!("  (no flags)  Open the application window.");
    println!("  --pick      Trigger the picker overlay and copy the result.");
    println!();
    println!("Bindings configured in-app under Settings > Keyboard shortcut.");
}

fn main() -> ExitCode {
    let flags = match parse_args() {
        Ok(f) => f,
        Err(code) => return code,
    };

    migrate_legacy_state();

    if flags.pick {
        return run_pick();
    }

    run_app()
}

/// `--pick` path. Talk to the running daemon if reachable; otherwise spawn
/// `cosmic-toysd` directly so the hotkey still works without a
/// daemon. Either way the daemon owns clipboard + notification delivery.
fn run_pick() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(_) => return spawn_daemon_oneshot(),
    };
    if runtime.block_on(ipc::daemon_reachable()) {
        // Hand off via the socket. The daemon's `pick` handler responds with
        // the hex (which we ignore here — clipboard + notify is its job).
        let _ = runtime.block_on(ipc::request_pick());
        return ExitCode::SUCCESS;
    }
    spawn_daemon_oneshot()
}

fn spawn_daemon_oneshot() -> ExitCode {
    match Command::new("cosmic-toysd").status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => ExitCode::from(s.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("cosmic-toys: failed to launch cosmic-toysd: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_app() -> ExitCode {
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    let settings = cosmic::app::Settings::default()
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(420.0)
                .min_height(360.0),
        )
        .size(cosmic::iced::Size::new(560.0, 680.0));

    match cosmic::app::run::<app::AppModel>(settings, ()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cosmic-toys: application failed: {e}");
            ExitCode::from(1)
        }
    }
}
