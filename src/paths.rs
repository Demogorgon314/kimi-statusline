//! Locations under the Kimi Code home directory, plus the debug log.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Let a spawned child outlive us: the TUI SIGKILLs our whole process group
/// at 300ms, so background work needs a group (or console) of its own.
pub fn detach(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    cmd
}

/// Run this binary with `args` in the background (hidden subcommands such as
/// `fetch-quota`).
pub fn spawn_self(args: &[&str]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = detach(Command::new(exe).args(args)).spawn();
}

/// Caches for sessions, repos and directories nobody has looked at in this
/// long are dropped by the daily sweep.
const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 86_400);
const SWEEP_EVERY: Duration = Duration::from_secs(86_400);

/// At most once a day, clear out the cache dir: temp files orphaned by a run
/// killed between write and rename (each is named after its pid, so none is
/// ever reused), and per-session / per-repo caches gone cold.
pub fn sweep_cache() {
    let dir = cache_dir();
    let stamp = dir.join("sweep.stamp");
    let age = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
    };
    if age(&stamp).is_some_and(|a| a < SWEEP_EVERY) {
        return;
    }
    let _ = write_atomic(&stamp, b"");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(a) = age(&path) else { continue };
        let orphan = name.ends_with(".tmp") && a > Duration::from_secs(60);
        let cold = ["session-", "git-", "pr-", "tasks-", "dance-"]
            .iter()
            .any(|p| name.starts_with(p))
            && a > CACHE_MAX_AGE;
        if orphan || cold {
            debug(&format!("sweep: removing {name}"));
            let _ = std::fs::remove_file(&path);
        }
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
