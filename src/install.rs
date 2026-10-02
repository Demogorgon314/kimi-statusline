//! Wire ourselves into `<KIMI_CODE_HOME>/tui.toml` as `[status_line].command`.
//! Edited with toml_edit so the user's comments and layout survive, and the
//! previous file is kept as `tui.toml.bak`.
//!
//! Plugin mode: the plugin's SessionStart hook runs `install --plugin`, which
//! writes the command with a `--plugin` flag. A status line run carrying that
//! flag checks the plugin record on every refresh and stands down by itself
//! once the plugin is disabled or removed (hooks stop running at that point,
//! so nothing else could clean up).

use crate::paths;
use std::path::PathBuf;
use toml_edit::{value, DocumentMut, Item, Table};

pub const PLUGIN_ID: &str = "kimi-statusline";

fn tui_toml() -> PathBuf {
    paths::kimi_home().join("tui.toml")
}

fn load() -> Result<(DocumentMut, String), String> {
    let path = tui_toml();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?;
    Ok((doc, text))
}

fn save(doc: &DocumentMut, old: &str) -> Result<(), String> {
    let path = tui_toml();
    let new = doc.to_string();
    if new == old {
        return Ok(());
    }
    // never leave the TUI with an unparseable file: it would drop every
    // preference back to defaults
    new.parse::<toml::Table>()
        .map_err(|e| format!("refusing to write invalid TOML: {e}"))?;
    if !old.is_empty() {
        std::fs::write(path.with_extension("toml.bak"), old).map_err(|e| e.to_string())?;
    }
    paths::write_atomic(&path, new.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn quote_command(exe: &str) -> Result<String, String> {
    Ok(format!("'{}'", exe.replace('\'', r"'\''")))
}

#[cfg(windows)]
fn quote_command(exe: &str) -> Result<String, String> {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let normalize = |s: &str| {
        if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            s.strip_prefix(r"\\?\").unwrap_or(s).to_string()
        }
    };
    let safe = |s: &str| {
        !s.chars()
            // cmd also splits the command token at , ; =
            .any(|c| c.is_whitespace() || "\"&|<>^()%!,;=".contains(c))
    };
    let path = normalize(exe);
    if safe(&path) {
        return Ok(path);
    }
    // status-line-command.ts passes the command to Node spawn(cmd, /d /s /c)
    // without windowsVerbatimArguments. Embedded quotes are escaped by libuv,
    // so use an unquoted 8.3 path, not Rust Command's different quoting rules.
    let wide: Vec<u16> = exe.encode_utf16().chain(Some(0)).collect();
    // SAFETY: input is NUL-terminated; the first call only requests the size.
    let size = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if size > 0 {
        let mut buffer = vec![0u16; size as usize];
        // SAFETY: buffer holds the number of UTF-16 elements passed to Win32.
        let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), size) };
        if written > 0 && written < size {
            let mut short = normalize(&String::from_utf16_lossy(&buffer[..written as usize]));
            // Keep our recognizable executable name for uninstall/plugin
            // ownership checks; only its parent directories need short names.
            if let Some(name) = std::path::Path::new(&path).file_name() {
                if safe(&name.to_string_lossy()) {
                    short = std::path::Path::new(&short)
                        .with_file_name(name)
                        .to_string_lossy()
                        .into_owned();
                }
            }
            if safe(&short) {
                return Ok(short);
            }
        }
    }
    Err("Cannot install this executable path through Kimi Code's Windows shell. No shell-safe 8.3 path is available; move kimi-statusline.exe to a directory without spaces or shell metacharacters and retry.".into())
}

fn is_ours(cmd: &str) -> bool {
    cmd.contains("kimi-statusline")
}

pub struct Outcome {
    pub changed: bool,
    pub command: String,
}

pub fn install(command: Option<String>, force: bool, plugin: bool) -> Result<Outcome, String> {
    let command = match command {
        Some(c) => c,
        None => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let exe = exe.canonicalize().unwrap_or(exe);
            let mut c = quote_command(&exe.to_string_lossy())?;
            if plugin {
                c += " --plugin";
            }
            c
        }
    };
    let (mut doc, old) = load()?;
    let section = doc
        .entry("status_line")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or("[status_line] in tui.toml is not a table")?;
    if let Some(existing) = section.get("command").and_then(|c| c.as_str()) {
        if !existing.is_empty() && !is_ours(existing) && !force {
            return Err(format!(
                "tui.toml already has status_line.command = {existing:?}; rerun with --force to replace it"
            ));
        }
    }
    section.insert("command", value(&command));
    let changed = doc.to_string() != old;
    save(&doc, &old)?;
    Ok(Outcome { changed, command })
}

/// Remove our command (and the section if nothing else is left in it).
/// Returns whether anything changed.
pub fn uninstall() -> Result<bool, String> {
    let (mut doc, old) = load()?;
    let Some(section) = doc.get_mut("status_line").and_then(|s| s.as_table_mut()) else {
        return Ok(false);
    };
    match section.get("command").and_then(|c| c.as_str()) {
        Some(cmd) if is_ours(cmd) => {}
        _ => return Ok(false),
    }
    section.remove("command");
    if section.is_empty() {
        doc.remove("status_line");
    }
    save(&doc, &old)?;
    Ok(true)
}

/// Whether the plugin has been disabled or removed. `/plugins disable` only
/// stops hooks and `/plugins remove` only drops the record; the tui.toml
/// command stays, so the status line has to notice by itself. An unreadable
/// installed.json fails open.
pub fn plugin_inactive() -> bool {
    let path = paths::kimi_home().join("plugins").join("installed.json");
    let Some(v) = crate::session::read_json(&path) else {
        return false;
    };
    let Some(records) = v.get("plugins").and_then(|p| p.as_array()) else {
        return false;
    };
    match records
        .iter()
        .find(|r| r.get("id").and_then(|i| i.as_str()) == Some(PLUGIN_ID))
    {
        Some(r) => !crate::upstream::plugin::record_active(r),
        None => true,
    }
}
