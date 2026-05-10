//! Read and write the Cosmic keyboard-shortcut entry that fires our picker.
//!
//! The file lives at
//! `~/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom`
//! and is a RON-ish map of `(modifiers: [...], key: "...") -> Action`. We
//! only ever touch the single entry whose `Spawn(...)` argument references
//! the `cosmic-toysd` binary, leaving every other entry untouched.

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

const SPAWN_COMMAND: &str = "cosmic-toysd --pick";

fn shortcuts_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    base.join("cosmic")
        .join("com.system76.CosmicSettings.Shortcuts")
        .join("v1")
        .join("custom")
}

/// Matches our entry in either the multi-line format we write or the
/// compact single-line shape Cosmic Settings tends to emit.
static OUR_ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\(\s*modifiers:\s*\[([^\]]*)\]\s*,\s*key:\s*"([^"]+)"\s*,?\s*\)\s*:\s*Spawn\(\s*"([^"]*cosmic-toysd[^"]*)"\s*\)\s*,?"#,
    )
    .expect("static regex compiles")
});

/// What the user sees / types: e.g. `Super+Shift+C`. Returns `Some(string)`
/// if our entry is currently bound, `None` otherwise.
pub fn current_binding() -> Option<String> {
    let text = std::fs::read_to_string(shortcuts_path()).ok()?;
    let cap = OUR_ENTRY_RE.captures(&text)?;
    let mods_raw = cap.get(1)?.as_str();
    let key = cap.get(2)?.as_str();

    let mut parts: Vec<String> = mods_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    parts.push(human_key(key));
    Some(parts.join("+"))
}

/// Validate user input. Returns Ok(parsed) where `parsed.0` is the list of
/// modifier names (capitalised: Super, Shift, Ctrl, Alt) and `.1` is the
/// key string in the form Cosmic expects ("c", "F1", "Down", ...).
pub fn parse_combo(input: &str) -> Result<(Vec<String>, String), String> {
    let parts: Vec<&str> = input.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("Empty shortcut".to_string());
    }
    let key = parts.last().unwrap();
    let mut modifiers = Vec::new();
    for p in &parts[..parts.len() - 1] {
        let canon = match p.to_ascii_lowercase().as_str() {
            "super" | "meta" | "win" | "logo" => "Super",
            "shift" => "Shift",
            "ctrl" | "control" => "Ctrl",
            "alt" => "Alt",
            other => return Err(format!("Unknown modifier: {other}")),
        };
        if !modifiers.iter().any(|m: &String| m == canon) {
            modifiers.push(canon.to_string());
        }
    }
    Ok((modifiers, normalise_key(key)))
}

/// Remove our entry from the Cosmic shortcuts file (if present). Used
/// while capturing a new binding so the user's *current* combo doesn't
/// fire the picker mid-capture, and on user-initiated unbind.
pub fn clear() -> Result<(), String> {
    let path = shortcuts_path();
    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Reading shortcuts file: {e}")),
    };
    let stripped = OUR_ENTRY_RE.replace_all(&original, "").into_owned();
    if stripped == original {
        return Ok(());
    }
    write_atomically(&path, stripped.as_bytes())
        .map_err(|e| format!("Writing shortcuts file: {e}"))
}

/// Persist the new binding to the Cosmic shortcuts file. Removes any
/// previous entry that points at our daemon and inserts the new one.
pub fn set_binding(input: &str) -> Result<(), String> {
    let (modifiers, key) = parse_combo(input)?;

    let path = shortcuts_path();
    let original = std::fs::read_to_string(&path)
        .map_err(|e| format!("Reading shortcuts file: {e}"))?;
    let stripped = OUR_ENTRY_RE.replace_all(&original, "");

    let mut new_entry = String::new();
    write!(
        &mut new_entry,
        "    (\n        modifiers: [\n"
    )
    .unwrap();
    for m in &modifiers {
        writeln!(&mut new_entry, "            {m},").unwrap();
    }
    writeln!(&mut new_entry, "        ],").unwrap();
    writeln!(&mut new_entry, "        key: \"{key}\",").unwrap();
    writeln!(&mut new_entry, "    ): Spawn(\"{SPAWN_COMMAND}\"),").unwrap();

    let injected = inject_before_close(&stripped, &new_entry)
        .ok_or_else(|| "Shortcuts file is malformed: no closing `}` found".to_string())?;
    write_atomically(&path, injected.as_bytes())
        .map_err(|e| format!("Writing shortcuts file: {e}"))?;
    Ok(())
}

fn inject_before_close(haystack: &str, new_entry: &str) -> Option<String> {
    // Find the last `}` at the file's trailing edge; ignore anything after it.
    let close_pos = haystack.rfind('}')?;
    let (head, tail) = haystack.split_at(close_pos);

    // Trim any trailing whitespace + stray comma/blank lines from the cleaned
    // section so the inserted block sits cleanly on its own.
    let head_trimmed = head.trim_end();
    let mut buf = String::with_capacity(haystack.len() + new_entry.len() + 4);
    buf.push_str(head_trimmed);
    if !head_trimmed.ends_with(',') && !head_trimmed.ends_with('{') {
        buf.push(',');
    }
    buf.push('\n');
    buf.push_str(new_entry);
    buf.push_str(tail);
    Some(buf)
}

fn write_atomically(path: &PathBuf, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn normalise_key(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.len() == 1 {
        // Single ASCII letters/digits live as lowercase in Cosmic's config.
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_string()
    }
}

fn human_key(key: &str) -> String {
    if key.len() == 1 {
        key.to_ascii_uppercase()
    } else {
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_super_shift_letter() {
        let (m, k) = parse_combo("Super+Shift+C").unwrap();
        assert_eq!(m, vec!["Super".to_string(), "Shift".to_string()]);
        assert_eq!(k, "c");
    }

    #[test]
    fn rejects_unknown_modifier() {
        assert!(parse_combo("Hyper+C").is_err());
    }

    #[test]
    fn deduplicates_modifiers() {
        let (m, _) = parse_combo("Ctrl+Ctrl+X").unwrap();
        assert_eq!(m, vec!["Ctrl".to_string()]);
    }

    #[test]
    fn matches_compact_format() {
        let text = r#"{
    (modifiers: [Super, Shift], key: "c"): Spawn("/bin/cosmic-toysd --pick"),
}"#;
        let cap = OUR_ENTRY_RE.captures(text).unwrap();
        assert_eq!(cap.get(2).unwrap().as_str(), "c");
    }
}
