//! Read and write Cosmic keyboard-shortcut entries that fire our tools.
//!
//! The file lives at
//! `~/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom`
//! and is a RON-ish map of `(modifiers: [...], key: "...") -> Action`. We
//! manage one entry per tool — each tool's command looks like
//! `Spawn("cosmic-toys run <tool>")`. Anything not matching that pattern
//! we leave untouched.
//!
//! Backwards compat: a `Spawn("cosmic-toysd --pick")` entry from v0.2.x
//! is treated as the color picker's binding (until set/cleared).

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

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

fn spawn_command(tool: &str) -> String {
    format!("cosmic-toys run {tool}")
}

/// Captures any of our entries, regardless of which tool. Group 1 =
/// modifiers list, group 2 = key, group 3 = full Spawn argument string.
/// Matches both the multi-line shape we write and the compact single-line
/// shape Cosmic Settings emits.
static OUR_ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\(\s*modifiers:\s*\[([^\]]*)\]\s*,\s*key:\s*"([^"]+)"\s*,?\s*\)\s*:\s*Spawn\(\s*"((?:[^"]*cosmic-toys(?:d)?[^"]*))"\s*\)\s*,?"#,
    )
    .expect("static regex compiles")
});

fn cmd_matches_tool(cmd: &str, tool: &str) -> bool {
    if cmd.contains(&spawn_command(tool)) {
        return true;
    }
    // Legacy alias: v0.2.x bound color_picker as `cosmic-toysd --pick`
    // (or `cosmic-toys --pick`). Treat both as the picker's binding.
    if tool == "color_picker" && (cmd.contains("--pick") || cmd.ends_with("--pick")) {
        return true;
    }
    false
}

/// Human-readable form of the binding for `tool`, or None if not bound.
pub fn current_binding(tool: &str) -> Option<String> {
    let text = std::fs::read_to_string(shortcuts_path()).ok()?;
    for cap in OUR_ENTRY_RE.captures_iter(&text) {
        let cmd = cap.get(3)?.as_str();
        if !cmd_matches_tool(cmd, tool) {
            continue;
        }
        let mods_raw = cap.get(1)?.as_str();
        let key = cap.get(2)?.as_str();
        let mut parts: Vec<String> = mods_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        parts.push(human_key(key));
        return Some(parts.join("+"));
    }
    None
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

/// Remove our entry for `tool` from the Cosmic shortcuts file (if present).
pub fn clear(tool: &str) -> Result<(), String> {
    let path = shortcuts_path();
    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Reading shortcuts file: {e}")),
    };
    let stripped = OUR_ENTRY_RE
        .replace_all(&original, |caps: &regex::Captures| -> String {
            let cmd = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            if cmd_matches_tool(cmd, tool) {
                String::new()
            } else {
                caps[0].to_string()
            }
        })
        .into_owned();
    if stripped == original {
        return Ok(());
    }
    write_atomically(&path, stripped.as_bytes())
        .map_err(|e| format!("Writing shortcuts file: {e}"))
}

/// Persist a new binding for `tool` to the Cosmic shortcuts file.
/// Replaces any existing entry that points at this tool's command.
pub fn set_binding(tool: &str, input: &str) -> Result<(), String> {
    let (modifiers, key) = parse_combo(input)?;
    clear(tool)?;

    let path = shortcuts_path();
    let original = std::fs::read_to_string(&path)
        .map_err(|e| format!("Reading shortcuts file: {e}"))?;

    let mut new_entry = String::new();
    write!(&mut new_entry, "    (\n        modifiers: [\n").unwrap();
    for m in &modifiers {
        writeln!(&mut new_entry, "            {m},").unwrap();
    }
    writeln!(&mut new_entry, "        ],").unwrap();
    writeln!(&mut new_entry, "        key: \"{key}\",").unwrap();
    writeln!(
        &mut new_entry,
        "    ): Spawn(\"{}\"),",
        spawn_command(tool)
    )
    .unwrap();

    let injected = inject_before_close(&original, &new_entry)
        .ok_or_else(|| "Shortcuts file is malformed: no closing `}` found".to_string())?;
    write_atomically(&path, injected.as_bytes())
        .map_err(|e| format!("Writing shortcuts file: {e}"))?;
    Ok(())
}

fn inject_before_close(haystack: &str, new_entry: &str) -> Option<String> {
    let close_pos = haystack.rfind('}')?;
    let (head, tail) = haystack.split_at(close_pos);
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
    fn matches_compact_format_per_tool() {
        let text = r#"{
    (modifiers: [Super, Shift], key: "c"): Spawn("cosmic-toys run color_picker"),
    (modifiers: [Super], key: "m"): Spawn("cosmic-toys run find_mouse"),
}"#;
        let mut hits = Vec::new();
        for cap in OUR_ENTRY_RE.captures_iter(text) {
            hits.push(cap.get(3).unwrap().as_str().to_string());
        }
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|c| c == "cosmic-toys run color_picker"));
        assert!(hits.iter().any(|c| c == "cosmic-toys run find_mouse"));
    }

    #[test]
    fn legacy_pick_form_maps_to_color_picker() {
        assert!(cmd_matches_tool("/usr/bin/cosmic-toysd --pick", "color_picker"));
        assert!(!cmd_matches_tool("/usr/bin/cosmic-toysd --pick", "find_mouse"));
    }
}
