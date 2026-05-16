//! Daemon-side IPC: listens on a Unix socket, dispatches incoming tool
//! requests, writes the tool's response back to the connecting client.
//!
//! Protocol (v0.3+, one request per connection):
//!   client writes  ->  `<tool-id>\n`
//!   server writes  <-  `<response>\n`
//!
//! Response shape is per-tool. `color_picker` returns the hex (or empty on
//! cancel). Tools without a return value (e.g. `find_mouse`) just write the
//! trailing newline. Unknown tool ids get an empty response.

use std::io;
use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::{find_mouse, history, ocr, overlay, screen_ruler};

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

async fn handle(stream: UnixStream) -> io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }
    let tool_id = line.trim();

    match tool_id {
        "color_picker" => {
            let result = tokio::task::spawn_blocking(overlay::pick_color)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?;
            match result {
                Ok(Some(hex)) => {
                    if let Err(e) = history::push(&hex) {
                        eprintln!("cosmic-toysd: history write failed: {e}");
                    }
                    write_half.write_all(hex.as_bytes()).await?;
                    write_half.write_all(b"\n").await?;
                }
                Ok(None) => {
                    write_half.write_all(b"\n").await?;
                }
                Err(e) => {
                    eprintln!("cosmic-toysd: pick failed: {e}");
                    write_half.write_all(b"\n").await?;
                }
            }
        }
        "find_mouse" => {
            let result = tokio::task::spawn_blocking(find_mouse::show)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?;
            if let Err(e) = result {
                eprintln!("cosmic-toysd: find_mouse failed: {e}");
            }
            write_half.write_all(b"\n").await?;
        }
        "screen_ruler" => {
            let result = tokio::task::spawn_blocking(screen_ruler::show)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?;
            if let Err(e) = result {
                eprintln!("cosmic-toysd: screen_ruler failed: {e}");
            }
            write_half.write_all(b"\n").await?;
        }
        "ocr" => {
            let result = tokio::task::spawn_blocking(ocr::show)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?;
            if let Err(e) = result {
                eprintln!("cosmic-toysd: ocr failed: {e}");
            }
            write_half.write_all(b"\n").await?;
        }
        other => {
            eprintln!("cosmic-toysd: unknown tool '{other}'");
            write_half.write_all(b"\n").await?;
        }
    }
    write_half.flush().await?;
    Ok(())
}

/// Cleanup helper for graceful shutdown.
pub fn remove_socket() {
    let _ = std::fs::remove_file(socket_path());
}
