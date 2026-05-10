//! cosmic-toysd: headless color-picker daemon.
//!
//! Three modes, dispatched on CLI flags:
//!
//!   cosmic-toysd            run forever; listen on the IPC socket
//!                                   and serve `pick` requests from clients
//!   cosmic-toysd --pick     one-shot pick that ALSO copies hex to
//!                                   the clipboard and fires a notification.
//!                                   Used by the hotkey when no daemon is
//!                                   running and as the v0.1-equivalent CLI.
//!   cosmic-toysd --quiet    one-shot pick that just prints the hex
//!                                   on stdout (no clipboard, no notify).
//!                                   Used by the GUI as a subprocess fallback.
//!
//! When the daemon is running, `--pick` invocations should ideally just
//! talk to it via the socket, but the standalone form keeps working for
//! cases where no daemon is reachable.

mod capture;
mod font;
mod history;
mod ipc;
mod overlay;

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

/// One-time copy of pre-rename history from
/// `com.pyxyll.CosmicColorPicker` (v0.2.x) to the new `com.pyxyll.CosmicToys`
/// namespace. Mirrors the same migration the GUI does — both run it because
/// either one could be the first invocation after upgrade. Idempotent.
fn migrate_legacy_history() {
    let xdg_config = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    let old_dir = xdg_config.join("cosmic/com.pyxyll.CosmicColorPicker/v1");
    let new_dir = xdg_config.join("cosmic/com.pyxyll.CosmicToys/v1");
    if new_dir.exists() || !old_dir.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&new_dir);
    if let Ok(entries) = std::fs::read_dir(&old_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::copy(entry.path(), new_dir.join(entry.file_name()));
        }
    }
}

#[derive(Debug, Default)]
struct CliFlags {
    pick: bool,
    quiet: bool,
}

fn parse_args() -> Result<CliFlags, ExitCode> {
    let mut flags = CliFlags::default();
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--pick" => flags.pick = true,
            "--quiet" | "-q" => flags.quiet = true,
            "-h" | "--help" => {
                print_help();
                return Err(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("cosmic-toysd {}", env!("CARGO_PKG_VERSION"));
                return Err(ExitCode::SUCCESS);
            }
            other => {
                eprintln!("cosmic-toysd: unknown argument: {other}");
                print_help();
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(flags)
}

fn print_help() {
    println!("Usage: cosmic-toysd [--pick | --quiet]");
    println!();
    println!("  (no flags)    Run forever as the IPC daemon.");
    println!("  --pick        One-shot pick: copy + notify, then exit.");
    println!("  --quiet       One-shot pick: print hex on stdout only.");
}

fn main() -> ExitCode {
    let flags = match parse_args() {
        Ok(f) => f,
        Err(code) => return code,
    };

    migrate_legacy_history();

    if flags.pick {
        // Hand off to a running daemon if one is reachable so the result
        // lands in shared history instead of a stale parallel state.
        if let Ok(rt) = tokio::runtime::Runtime::new()
            && rt.block_on(try_remote_pick(flags.quiet))
        {
            return ExitCode::SUCCESS;
        }
    }

    if flags.pick || flags.quiet {
        return run_oneshot(flags.quiet);
    }

    run_daemon()
}

/// Connect to the running daemon's socket, request a pick, await the hex,
/// optionally copy + notify on this side. Returns true on success.
async fn try_remote_pick(quiet: bool) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(mut stream) = tokio::net::UnixStream::connect(ipc::socket_path()).await else {
        return false;
    };
    if stream.write_all(b"p").await.is_err() {
        return false;
    }
    let mut buf = String::new();
    if stream.read_to_string(&mut buf).await.is_err() {
        return false;
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        // user cancelled — still a success from the IPC perspective
        return true;
    }
    println!("{trimmed}");
    if !quiet {
        deliver(trimmed);
    }
    true
}

fn run_oneshot(quiet: bool) -> ExitCode {
    match overlay::pick_color() {
        Ok(Some(hex)) => {
            // Persist regardless of mode so the GUI sees one-shot picks too.
            if let Err(e) = history::push(&hex) {
                eprintln!("cosmic-toysd: history write failed: {e}");
            }
            println!("{hex}");
            if !quiet {
                deliver(&hex);
            }
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cosmic-toysd: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_daemon() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cosmic-toysd: cannot start tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    runtime.block_on(async {
        if ipc::another_daemon_running().await {
            eprintln!("cosmic-toysd: another instance is already serving the socket");
            return ExitCode::SUCCESS;
        }

        let serve = ipc::serve();
        let mut sigterm = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("cosmic-toysd: cannot install SIGTERM handler: {e}");
                ipc::remove_socket();
                return ExitCode::from(1);
            }
        };

        let result = tokio::select! {
            r = serve => r,
            _ = sigterm.recv() => {
                eprintln!("cosmic-toysd: SIGTERM received, exiting");
                Ok(())
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("cosmic-toysd: SIGINT received, exiting");
                Ok(())
            }
        };

        ipc::remove_socket();

        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("cosmic-toysd: serve failed: {e}");
                ExitCode::from(1)
            }
        }
    })
}

fn deliver(hex: &str) {
    if let Ok(mut child) = Command::new("wl-copy").stdin(Stdio::piped()).spawn()
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(hex.as_bytes());
        drop(stdin);
        let _ = child.wait();
    }

    let _ = Command::new("notify-send")
        .args([
            "--app-name",
            "Color Picker",
            "--icon",
            "color-select-symbolic",
            "--expire-time",
            "3000",
            hex,
            "Copied to clipboard",
        ])
        .status();
}
