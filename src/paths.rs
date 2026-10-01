//! Locations under the Kimi Code home directory, plus the debug log.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `$KIMI_CODE_HOME`, else `~/.kimi-code`.
pub fn kimi_home() -> PathBuf {
    if let Some(p) = std::env::var_os("KIMI_CODE_HOME") {
        return PathBuf::from(p);
    }
    home_dir()
        .map(|h| h.join(".kimi-code"))
        .unwrap_or_else(|| PathBuf::from(".kimi-code"))
}

pub fn cache_dir() -> PathBuf {
    kimi_home().join("kimi-statusline-cache")
}

/// Stable short hash for cache file names (FNV-1a 64; std's hasher is not
/// stable across releases).
pub fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Append to `<home>/kimi-statusline-debug.log` when `KIMI_STATUSLINE_DEBUG`
/// is set or the flag file `<home>/kimi-statusline-debug` exists (the latter
/// needs no TUI restart).
pub fn debug(msg: &str) {
    let home = kimi_home();
    if std::env::var_os("KIMI_STATUSLINE_DEBUG").is_none()
        && !home.join("kimi-statusline-debug").exists()
    {
        return;
    }
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("kimi-statusline-debug.log"))
    {
        let _ = writeln!(f, "{:.3} {msg}", now_secs());
    }
}

/// Write via a temp file + rename, so a run killed at the 300ms cap can never
/// leave a half-written cache behind.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
