//! Daemon-side IPC: listens on a Unix socket, runs the overlay on demand,
//! writes the picked hex back to the connecting client (and persists it).
//!
//! Protocol (one request per connection):
//!   client writes  ->  `b'p'`            request a pick
//!   server writes  <-  `<hex>\n` or ``  the picked colour, or empty on cancel
//!
//! No other bytes are defined yet. Future extensions (e.g. clear-history,
//! query-current) would add new request bytes; the daemon is forward-
//! compatible because unknown bytes get rejected silently.

use std::io;
use std::path::PathBuf;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::{history, overlay};

pub fn socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    runtime.join("cosmic-toysd.sock")
}

/// Returns `Ok(true)` if a daemon was already listening (we connected and
/// closed cleanly). Used by the daemon at startup to fail fast and exit.
pub async fn another_daemon_running() -> bool {
    UnixStream::connect(socket_path()).await.is_ok()
}

/// Bind and serve forever. Each accepted connection runs one overlay
/// session on a blocking thread, persists the result, and writes it back
/// to the client. Concurrent picks are intentionally serialised at the
/// overlay level (only one layer-shell session at a time makes sense).
pub async fn serve() -> io::Result<()> {
    let path = socket_path();
    // Cleanup of any stale file from a crashed previous instance.
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&path)?;
    eprintln!("cosmic-toysd: listening on {}", path.display());

    let pick_lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));

    loop {
        let (stream, _) = listener.accept().await?;
        let lock = pick_lock.clone();
        tokio::spawn(async move {
            let _guard = lock.lock().await;
            if let Err(e) = handle(stream).await {
                eprintln!("cosmic-toysd: client error: {e}");
            }
        });
    }
}

async fn handle(mut stream: UnixStream) -> io::Result<()> {
    let mut buf = [0u8; 1];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    if buf[0] != b'p' {
        return Ok(());
    }

    let result = tokio::task::spawn_blocking(overlay::pick_color)
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;

    match result {
        Ok(Some(hex)) => {
            // Persist before responding; if the client disappears the entry
            // still lives on disk.
            if let Err(e) = history::push(&hex) {
                eprintln!("cosmic-toysd: history write failed: {e}");
            }
            stream.write_all(hex.as_bytes()).await?;
            stream.write_all(b"\n").await?;
        }
        Ok(None) => {
            // User cancelled — empty response signals "no pick".
            stream.write_all(b"\n").await?;
        }
        Err(e) => {
            eprintln!("cosmic-toysd: pick failed: {e}");
            stream.write_all(b"\n").await?;
        }
    }
    stream.flush().await?;
    Ok(())
}

/// Cleanup helper for graceful shutdown.
pub fn remove_socket() {
    let _ = std::fs::remove_file(socket_path());
}
