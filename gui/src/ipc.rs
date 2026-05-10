//! GUI-side IPC: a client that talks to the cosmic-toysd daemon.
//!
//! Protocol matches the daemon's `ipc.rs`: write `b'p'`, read back the
//! picked hex (or empty line on cancel). The GUI is purely a client now;
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

/// Ask the running daemon to pick a colour. Returns:
///   `Some(Some(hex))`  daemon picked a colour
///   `Some(None)`       daemon was reachable but the user cancelled
///   `None`             no daemon reachable; caller should fall back
pub async fn request_pick() -> Option<Option<String>> {
    let mut stream = UnixStream::connect(socket_path()).await.ok()?;
    stream.write_all(b"p").await.ok()?;

    // Generous read timeout: an idle user can sit on the picker for ages.
    let mut buf = String::new();
    let read = tokio::time::timeout(Duration::from_secs(600), stream.read_to_string(&mut buf))
        .await
        .ok()?;
    read.ok()?;

    let trimmed = buf.trim();
    if trimmed.is_empty() {
        Some(None)
    } else {
        Some(Some(trimmed.to_string()))
    }
}

/// Quick reachability check used by the `--pick` CLI fallback path.
pub async fn daemon_reachable() -> bool {
    UnixStream::connect(socket_path()).await.is_ok()
}
