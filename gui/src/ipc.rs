//! GUI-side IPC: a client that talks to the cosmic-toysd daemon.
//!
//! Protocol (v0.3+): client writes `<tool-id>\n`, daemon runs that tool and
//! writes back a tool-specific response (color picker returns the hex; tools
//! with no return value write a single `\n`). The GUI is purely a client;
//! the daemon is the only process that listens on the socket.

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

fn socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    runtime.join("cosmic-toysd.sock")
}

/// Generic tool dispatch. Used for any tool that has daemon-side work to do.
/// Returns the trimmed response string, or `None` if the daemon wasn't
/// reachable (the caller is expected to fall back to a subprocess).
pub async fn request_run(tool: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket_path()).await.ok()?;
    let mut payload = tool.as_bytes().to_vec();
    payload.push(b'\n');
    stream.write_all(&payload).await.ok()?;

    let mut buf = String::new();
    // Generous read timeout — picker users may take a while to click.
    let _ = tokio::time::timeout(Duration::from_secs(600), stream.read_to_string(&mut buf)).await;
    Some(buf.trim().to_string())
}

/// Convenience wrapper for the color picker — returns the picked hex, or
/// `None` on cancel / unreachable daemon. Kept around because the `Pick`
/// button in the GUI cares about the hex specifically.
pub async fn request_pick() -> Option<Option<String>> {
    let resp = request_run("color_picker").await?;
    if resp.is_empty() {
        Some(None)
    } else {
        Some(Some(resp))
    }
}

/// Quick reachability check used by the run-fallback path in main.
pub async fn daemon_reachable() -> bool {
    UnixStream::connect(socket_path()).await.is_ok()
}
