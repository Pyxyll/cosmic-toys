//! Persisted pick history.
//!
//! Stored at `$XDG_CONFIG_HOME/cosmic/com.pyxyll.CosmicToys/v1/history`
//! (the same path cosmic-config uses for the GUI's `Config.history` field) so
//! the GUI's `watch_config` subscription picks up daemon writes automatically.
//!
//! The on-disk format is a RON list of double-quoted hex strings, newest
//! first. We don't pull in the `ron` crate to write it — the format is
//! trivial enough to emit by hand, and skipping the dep keeps the daemon
//! binary tiny.

use std::io;
use std::path::PathBuf;

const APP_ID: &str = "com.pyxyll.CosmicToys";
const VERSION: &str = "v1";
const FIELD: &str = "history";
const LIMIT: usize = 64;

fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        })
        .join("cosmic")
        .join(APP_ID)
        .join(VERSION)
}

fn history_path() -> PathBuf {
    config_dir().join(FIELD)
}

pub fn load() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    parse_ron_strings(&text)
}

pub fn push(hex: &str) -> io::Result<()> {
    let mut list = load();
    list.insert(0, hex.to_string());
    list.truncate(LIMIT);
    write(&list)
}

fn write(list: &[String]) -> io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    let mut body = String::from("[\n");
    for entry in list {
        body.push_str("    \"");
        body.push_str(entry);
        body.push_str("\",\n");
    }
    body.push(']');
    std::fs::write(history_path(), body)
}

/// Minimal parser: walks the file looking for double-quoted substrings.
/// Tolerates trailing commas, whitespace and the surrounding brackets.
/// Anything that isn't a literal string is silently ignored.
fn parse_ron_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        for ch in chars.by_ref() {
            if ch == '"' {
                break;
            }
            s.push(ch);
        }
        if !s.is_empty() {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_ron_strings;

    #[test]
    fn parses_typical_cosmic_config_format() {
        let input = "[\n    \"#FFEFCD\",\n    \"#160008\",\n]";
        let parsed = parse_ron_strings(input);
        assert_eq!(parsed, vec!["#FFEFCD", "#160008"]);
    }
}
