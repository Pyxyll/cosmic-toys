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
mod tools;

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
    /// If set, run the named tool one-shot via the daemon (or daemon
    /// subprocess fallback) and exit. `None` means "open the GUI window".
    run_tool: Option<String>,
}

fn parse_args() -> Result<CliFlags, ExitCode> {
    let mut flags = CliFlags::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // `cosmic-toys run <tool>` — canonical form for v0.3+.
            "run" => match args.next() {
                Some(tool) => flags.run_tool = Some(tool),
                None => {
                    eprintln!("'run' requires a tool id (e.g. 'cosmic-toys run color_picker')");
                    return Err(ExitCode::from(2));
                }
            },
            // Legacy alias kept so v0.2.x shortcut bindings keep working.
            "--pick" => flags.run_tool = Some("color_picker".into()),
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
    println!("Usage: cosmic-toys [run <tool>]");
    println!();
    println!("  (no args)         Open the application window.");
    println!("  run <tool>        Trigger a tool one-shot (e.g. `run color_picker`).");
    println!("  --pick            Alias for `run color_picker` (legacy).");
    println!();
    println!("Hotkeys: bind in-app under Settings, or point your shortcut config at");
    println!("`cosmic-toys run <tool>` directly.");
}

fn main() -> ExitCode {
    let flags = match parse_args() {
        Ok(f) => f,
        Err(code) => return code,
    };

    migrate_legacy_state();

    if let Some(tool) = flags.run_tool {
        return run_tool(&tool);
    }

    run_app()
}

/// One-shot tool invocation. Talk to the running daemon if reachable;
/// otherwise spawn `cosmic-toysd run <tool>` so a hotkey still works without
/// a daemon. Either way the daemon owns side-effects (clipboard, notification,
/// overlay rendering).
fn run_tool(tool: &str) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(_) => return spawn_daemon_oneshot(tool),
    };
    if runtime.block_on(ipc::daemon_reachable()) {
        let _ = runtime.block_on(ipc::request_run(tool));
        return ExitCode::SUCCESS;
    }
    spawn_daemon_oneshot(tool)
}

fn spawn_daemon_oneshot(tool: &str) -> ExitCode {
    match Command::new("cosmic-toysd").args(["run", tool]).status() {
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
