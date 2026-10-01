//! Session discovery and incremental `wire.jsonl` parsing.
//!
//! Data source: `<home>/sessions/wd_*/session_*/agents/*/wire.jsonl`. Each
//! agent's wire is read from the byte offset the previous run stopped at, so
//! a refresh on a 100MB session costs the same as on a fresh one. The cursors
//! and the totals they produced live in a per-session cache file.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CACHE_VERSION: u32 = 1;
/// Most bytes one wire may contribute per run; a huge first sighting is spread
/// over several refreshes instead of blowing the 300ms cap (a killed run saves
/// nothing, so it would re-read the same bytes forever).
const MAX_TAIL_BYTES: u64 = 16 * 1024 * 1024;
const PARSE_BUDGET: Duration = Duration::from_millis(120);

/// Reserved aliases standing in for a real model; mapped back through the same
/// agent's llm.request records.
const PLACEHOLDER_ALIASES: [&str; 3] = ["__secondary__", "secondary", "primary"];

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Usage {
    pub input_other: u64,
    pub output: u64,
    pub input_cache_read: u64,
    pub input_cache_creation: u64,
}

impl Usage {
    pub fn input(&self) -> u64 {
        self.input_other + self.input_cache_read + self.input_cache_creation
    }

    pub fn add(&mut self, o: &Usage) {
        self.input_other += o.input_other;
        self.output += o.output;
        self.input_cache_read += o.input_cache_read;
        self.input_cache_creation += o.input_cache_creation;
    }

    /// Cache hit rate in percent; None before any input.
    pub fn cache_rate(&self) -> Option<f64> {
        let total = self.input();
        (total > 0).then(|| self.input_cache_read as f64 / total as f64 * 100.0)
    }

    pub fn is_empty(&self) -> bool {
        self.input() == 0 && self.output == 0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cursor {
    offset: u64,
    /// The offset landed inside an over-long line; skip to the next newline.
    mid: bool,
    bound: Option<String>,
    real: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SessionStats {
    pub v: u32,
    pub total: Usage,
    pub main: Usage,
    pub sub_by_model: BTreeMap<String, Usage>,
    pub swarm: bool,
    pub tower: bool,
    /// thinkingEffort of the latest effort-bearing record in the main wire;
    /// a string ("high"), or a bool for legacy models.
    pub effort: Option<serde_json::Value>,
    pub files: BTreeMap<String, Cursor>,
    pub goal_key: Option<String>,
    pub goal_seen_at: Option<f64>,
}

// ---------------------------------------------------------------------------
// session resolution
// ---------------------------------------------------------------------------

pub fn find_session_dir(session_id: &str, cwd: &str) -> Option<PathBuf> {
    if session_id.is_empty() && cwd.is_empty() {
        return None;
    }
    let home = paths::kimi_home();
    if !session_id.is_empty() {
        if let Ok(f) = File::open(home.join("session_index.jsonl")) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if !line.contains(session_id) {
                    continue;
                }
                let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if rec.get("sessionId").and_then(|v| v.as_str()) == Some(session_id) {
                    if let Some(dir) = rec.get("sessionDir").and_then(|v| v.as_str()) {
                        let dir = PathBuf::from(dir);
                        if dir.is_dir() {
                            return Some(dir);
                        }
                    }
                }
            }
        }
        for name in [session_id.to_string(), format!("session_{session_id}")] {
            for wd in read_dirs(&home.join("sessions")) {
                let candidate = wd.join(&name);
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }
    // fallback: newest session whose recorded cwd matches
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for wd in read_dirs(&home.join("sessions")) {
        for session in read_dirs(&wd) {
            let state = session.join("state.json");
            let Ok(meta) = state.metadata() else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            if best.as_ref().is_some_and(|(t, _)| *t >= mtime) {
                continue;
            }
            if !cwd.is_empty() {
                let Some(v) = read_json(&state) else { continue };
                let rec_cwd = v
                    .get("cwd")
                    .or_else(|| v.get("workDir"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                if !same_path(rec_cwd, cwd) {
                    continue;
                }
            }
            best = Some((mtime, session));
        }
    }
    best.map(|(_, p)| p)
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim_end_matches(['/', '\\']).replace('\\', "/");
    if cfg!(windows) {
        norm(a).eq_ignore_ascii_case(&norm(b))
    } else {
        norm(a) == norm(b)
    }
}

pub fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

pub fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

// ---------------------------------------------------------------------------
// cache
// ---------------------------------------------------------------------------

fn cache_path(session_id: &str) -> PathBuf {
    paths::cache_dir().join(format!("session-{}.json", paths::short_hash(session_id)))
}

pub fn load_cache(session_id: &str) -> Option<SessionStats> {
    let bytes = std::fs::read(cache_path(session_id)).ok()?;
    let stats: SessionStats = serde_json::from_slice(&bytes).ok()?;
    (stats.v == CACHE_VERSION).then_some(stats)
}

pub fn save_cache(session_id: &str, stats: &SessionStats) {
    if let Ok(data) = serde_json::to_vec(stats) {
        let _ = paths::write_atomic(&cache_path(session_id), &data);
    }
}

// ---------------------------------------------------------------------------
// wire parsing
// ---------------------------------------------------------------------------

/// Read what was appended after `start`. Returns (complete lines, new offset,
/// mid). A trailing partial line (writer mid-append) is left for next time.
fn read_tail(path: &Path, start: u64, mid: bool, size: u64) -> (Vec<u8>, u64, bool) {
    let remaining = size.saturating_sub(start);
    if remaining == 0 {
        return (Vec::new(), start, mid);
    }
    let want = remaining.min(MAX_TAIL_BYTES);
    let mut buf = Vec::with_capacity(want as usize);
    let read = File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(start))?;
        f.take(want).read_to_end(&mut buf)
    });
    if read.is_err() {
        return (Vec::new(), start, mid);
    }
    let at_eof = buf.len() as u64 >= remaining;
    let mut start = start;
    let mut chunk = &buf[..];
    if mid {
        match chunk.iter().position(|&b| b == b'\n') {
            Some(nl) => {
                start += nl as u64 + 1;
                chunk = &chunk[nl + 1..];
            }
            None => return (Vec::new(), start + chunk.len() as u64, !at_eof),
        }
    }
    match chunk.iter().rposition(|&b| b == b'\n') {
        Some(cut) => (chunk[..=cut].to_vec(), start + cut as u64 + 1, false),
        // no complete line: wait for the writer, or step over a line longer
        // than the cap (a huge tool result, never a record we want)
        None if at_eof => (Vec::new(), start, false),
        None => (Vec::new(), start + chunk.len() as u64, true),
    }
}

/// The record's own `"type"` when the line starts with it. Matching the
/// leading key (not a substring search) keeps tool output that merely quotes
/// a record name from being taken for the record itself.
fn record_type(line: &[u8]) -> Option<&str> {
    let rest = line.strip_prefix(b"{\"type\":\"")?;
    let end = rest.iter().take(48).position(|&b| b == b'"')?;
    std::str::from_utf8(&rest[..end]).ok()
}

const WANTED: [&str; 8] = [
    "usage.record",
    "profile.bind",
    "llm.request",
    "config.update",
    "swarm_mode.enter",
    "swarm_mode.exit",
    "tower_mode.enter",
    "tower_mode.exit",
];

#[derive(Deserialize)]
struct Rec {
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "modelAlias")]
    model_alias: Option<String>,
    #[serde(default, rename = "thinkingEffort")]
    thinking_effort: Option<serde_json::Value>,
    #[serde(default, rename = "thinkingLevel")]
    thinking_level: Option<serde_json::Value>,
    #[serde(default)]
    usage: Option<Usage>,
}

/// Fold everything appended since `prior` into fresh stats.
pub fn parse_session(dir: &Path, prior: Option<SessionStats>) -> SessionStats {
    let mut st = prior.unwrap_or_default();
    st.v = CACHE_VERSION;
    let deadline = Instant::now() + PARSE_BUDGET;

    let mut agents = read_dirs(&dir.join("agents"));
    // main first: it carries the mode/effort state the prefix needs
    agents.sort_by_key(|p| p.file_name().map(|n| n != "main"));
    for agent_dir in agents {
        let wire = agent_dir.join("wire.jsonl");
        let agent = agent_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_main = agent == "main";
        let Ok(size) = wire.metadata().map(|m| m.len()) else {
            continue;
        };
        let mut cur = st.files.get(&agent).cloned().unwrap_or_default();
        if size < cur.offset {
            // truncated/replaced wire: per-file subtotals aren't kept, so
            // start the whole session over
            paths::debug(&format!("{agent}: wire shrank, full re-parse"));
            return parse_session(dir, None);
        }
        if size > cur.offset && Instant::now() > deadline {
            paths::debug(&format!("{agent}: parse budget spent, resuming next run"));
            continue;
        }
        let (chunk, offset, mid) = read_tail(&wire, cur.offset, cur.mid, size);
        for line in chunk.split(|&b| b == b'\n') {
            let Some(kind) = record_type(line) else {
                continue;
            };
            if !WANTED.contains(&kind) {
                continue;
            }
            match kind {
                "swarm_mode.enter" | "swarm_mode.exit" => {
                    if is_main {
                        st.swarm = kind.ends_with(".enter");
                    }
                    continue;
                }
                "tower_mode.enter" | "tower_mode.exit" => {
                    if is_main {
                        st.tower = kind.ends_with(".enter");
                    }
                    continue;
                }
                _ => {}
            }
            let Ok(rec) = serde_json::from_slice::<Rec>(line) else {
                continue;
            };
            match kind {
                "profile.bind" | "llm.request" | "config.update" => {
                    if rec.model_alias.is_some() {
                        cur.bound = rec.model_alias.clone();
                    }
                    if kind == "llm.request" && rec.model.is_some() {
                        cur.real = rec.model.clone();
                    }
                    if is_main {
                        if let Some(e) = rec.thinking_effort.or(rec.thinking_level) {
                            if !e.is_null() {
                                st.effort = Some(e);
                            }
                        }
                    }
                }
                "usage.record" => {
                    let Some(u) = rec.usage else { continue };
                    st.total.add(&u);
                    if is_main {
                        st.main.add(&u);
                        continue;
                    }
                    let mut model = rec.model.unwrap_or_default();
                    if model.is_empty() || PLACEHOLDER_ALIASES.contains(&model.as_str()) {
                        model = cur
                            .real
                            .clone()
                            .or_else(|| cur.bound.clone())
                            .unwrap_or(model);
                    }
                    if model.is_empty() {
                        model = "unknown".into();
                    }
                    st.sub_by_model.entry(model).or_default().add(&u);
                }
                _ => {}
            }
        }
        cur.offset = offset;
        cur.mid = mid;
        st.files.insert(agent, cur);
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_type_only_matches_leading_key() {
        assert_eq!(
            record_type(br#"{"type":"usage.record","x":1}"#),
            Some("usage.record")
        );
        assert_eq!(record_type(br#"{"x":{"type":"usage.record"}}"#), None);
    }

    #[test]
    fn parses_usage_and_subagents() {
        let dir = std::env::temp_dir().join(format!("kimi-sl-test-{}", std::process::id()));
        let main = dir.join("agents/main");
        let sub = dir.join("agents/agent-0");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            main.join("wire.jsonl"),
            concat!(
                r#"{"type":"llm.request","model":"k3","modelAlias":"kimi-code/k3","thinkingEffort":"max"}"#, "\n",
                r#"{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":100,"output":10,"inputCacheRead":900,"inputCacheCreation":0}}"#, "\n",
                r#"{"type":"swarm_mode.enter"}"#, "\n",
                r#"{"type":"usage.record","model":"partial"#,
            ),
        )
        .unwrap();
        std::fs::write(
            sub.join("wire.jsonl"),
            concat!(
                r#"{"type":"llm.request","model":"k3-real","modelAlias":"__secondary__"}"#, "\n",
                r#"{"type":"usage.record","model":"__secondary__","usage":{"inputOther":50,"output":5,"inputCacheRead":50,"inputCacheCreation":0}}"#, "\n",
            ),
        )
        .unwrap();

        let st = parse_session(&dir, None);
        assert_eq!(st.total.input(), 1100);
        assert_eq!(st.total.output, 15);
        assert_eq!(st.main.input(), 1000);
        assert_eq!(st.sub_by_model["k3-real"].input(), 100);
        assert!(st.swarm);
        assert_eq!(st.effort, Some(serde_json::json!("max")));

        // second run only reads the completed partial line
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(main.join("wire.jsonl"))
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            br#"","usage":{"inputOther":1,"output":1,"inputCacheRead":0,"inputCacheCreation":0}}
"#,
        )
        .unwrap();
        let st2 = parse_session(&dir, Some(st));
        assert_eq!(st2.total.input(), 1101);
        assert_eq!(st2.total.output, 16);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
