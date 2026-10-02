//! Session discovery and incremental `wire.jsonl` parsing.
//!
//! Data source: `<home>/sessions/wd_*/session_*/agents/*/wire.jsonl`. Each
//! agent's wire is read from the byte offset the previous run stopped at, so
//! a refresh on a 100MB session costs the same as on a fresh one. The cursors
//! and the totals they produced live in a per-session cache file.

use crate::paths;
use crate::probe;
pub use crate::upstream::wire::Usage;
use crate::upstream::wire::{record_type, LoopEvent, RecordFields, StepEnd, WANTED};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CACHE_VERSION: u32 = 5;
/// Bound background catch-up memory and work per refresh. The foreground
/// never opens wires, so slow filesystem reads cannot consume its TUI budget.
const MAX_TAIL_BYTES: u64 = 16 * 1024 * 1024;
const PARSE_BUDGET: Duration = Duration::from_millis(120);
// Below the TUI's one-second cadence so a worker finishing just after one
// render does not suppress the next refresh for an additional second.
const SNAPSHOT_TTL: f64 = 0.5;

/// Small published view: foreground reads never enumerate sessions or wires.
/// Parser cursors stay in the separate per-directory cache.
#[derive(Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub dir: Option<PathBuf>,
    pub stats: Option<SessionStats>,
    pub created: Option<f64>,
}

fn snapshot_name(id: &str) -> String {
    format!(
        "session-view-v{CACHE_VERSION}-{}.json",
        paths::short_hash(id)
    )
}

pub fn snapshot(id: &str, live: bool) -> Snapshot {
    if id.is_empty() {
        return Snapshot::default();
    }
    if !live {
        return refresh(id);
    }
    let name = snapshot_name(id);
    let (value, stale) = probe::cached::<Snapshot>(&name, SNAPSHOT_TTL);
    let value = value.unwrap_or_default();
    if stale {
        probe::schedule_probe(
            &name,
            SNAPSHOT_TTL,
            &value,
            &["probe-session", "--session-id", id],
        );
    }
    value
}

pub fn refresh(id: &str) -> Snapshot {
    if id.is_empty() {
        return Snapshot::default();
    }
    let name = snapshot_name(id);
    let Some(_guard) = paths::lock(&paths::cache_dir().join(&name).with_extension("lock"), true)
    else {
        return Snapshot::default();
    };
    let mut view = Snapshot::default();
    if let Some(dir) = find_session_dir(id) {
        let key = dir.to_string_lossy();
        let Some(_parser_guard) = paths::lock(&cache_path(&key).with_extension("lock"), true)
        else {
            return view;
        };
        let mut stats = parse_session_until(&dir, load_cache(&key), Instant::now() + PARSE_BUDGET);
        let state = read_json(&dir.join("state.json"));
        view.created = state
            .as_ref()
            .and_then(|s| s.get("createdAt")?.as_f64())
            .map(|ms| ms / 1000.0);
        normalize_goal(&mut stats, state.as_ref());
        save_cache(&key, &stats);
        stats.files.clear();
        view.stats = Some(stats);
        view.dir = Some(dir);
    }
    probe::store(&name, &view);
    view
}

/// Both historical metadata and wire events publish one goal and clock anchor.
fn normalize_goal(stats: &mut SessionStats, state: Option<&serde_json::Value>) {
    if !stats.goal_events_seen {
        let legacy = state.and_then(|s| s.get("custom")?.get("goal"));
        let key = legacy.map(|g| g.to_string());
        stats.goal = legacy.and_then(|g| serde_json::from_value(g.clone()).ok());
        if key != stats.goal_key || (key.is_some() && stats.goal_seen_at.is_none()) {
            stats.goal_seen_at = key.as_ref().map(|_| paths::now_secs());
        }
        if let Some(goal) = stats.goal.as_mut().filter(|g| g.status == "active") {
            goal.wall_clock_resumed_at = goal
                .wall_clock_resumed_at
                .or_else(|| stats.goal_seen_at.map(|s| s * 1000.0));
        }
        stats.goal_key = key;
    }
    stats.goal_seen_at = stats
        .goal
        .as_ref()
        .and_then(|g| g.wall_clock_resumed_at)
        .map(|ms| ms / 1000.0);
}

/// Reserved aliases standing in for a real model; mapped back through the same
/// agent's llm.request records.
const PLACEHOLDER_ALIASES: [&str; 3] = ["__secondary__", "secondary", "primary"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cursor {
    /// Hash of the open handle's dev/inode (Unix) or volume/file index (Windows).
    identity: Option<u64>,
    offset: u64,
    /// The offset landed inside an over-long line; skip to the next newline.
    mid: bool,
    bound: Option<String>,
    real: Option<String>,
    stream_end: Option<StreamEnd>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct StreamEnd {
    time: f64,
    usage: Usage,
}

/// One model call, timed by Kimi Code: when its stream started and ended
/// (unix ms), and the output tokens it produced.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Call {
    pub agent: String,
    pub start: f64,
    pub end: f64,
    pub output: u64,
    /// time to first token, ms
    pub ttft: f64,
    /// Only calls paired with a turn's usage record have a stream end anchor.
    #[serde(default)]
    pub anchored: bool,
}

impl Call {
    /// Decode speed: output tokens over the streaming time, the standard
    /// definition of TPS. Time to first token is excluded (it is queueing
    /// and prefill, not generation) and kept separately.
    pub fn rate(&self) -> f64 {
        self.output as f64 / ((self.end - self.start) / 1000.0)
    }
}

/// Generation speed across every agent. Sub-agents run in parallel with
/// each other while the main agent waits, so per-call speed alone would
/// understate how fast tokens arrive: `recent` keeps the latest calls of
/// all agents so overlapping ones can be summed into a combined rate.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Speed {
    /// latest calls of any agent, ordered by end time, capped at RECENT_CAP
    pub recent: Vec<Call>,
    /// session totals over all measured calls: output tokens and seconds
    /// of streaming, for a token-weighted average
    pub output: u64,
    pub secs: f64,
}

/// Throughput over a window of overlapping calls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Throughput {
    pub tokens_per_sec: f64,
    /// most calls streaming at the same moment within the window
    pub agents: usize,
}

impl Speed {
    const RECENT_CAP: usize = 64;
    /// Streams shorter than this are left out. Tokens arrive in network
    /// bursts, so a 150 ms tool-call stream of 60 tokens reads as ~400
    /// tok/s: the burst size, not the model's speed.
    const MIN_STREAM_MS: f64 = 1000.0;

    fn record_step(
        &mut self,
        agent: &str,
        step: &StepEnd,
        line_time: Option<f64>,
        stream_end: Option<StreamEnd>,
    ) {
        let (Some(stream_ms), Some(usage), Some(observed_at)) =
            (step.stream_ms, step.usage, line_time)
        else {
            return;
        };
        if stream_ms < Self::MIN_STREAM_MS || usage.output == 0 {
            return;
        }
        // step.end is emitted after tools complete. usage.record is emitted
        // when the LLM returns, before those tools run. Legacy records without
        // that anchor still provide a rate, but cannot establish overlap.
        let anchor = stream_end.filter(|s| s.usage == usage && s.time <= observed_at);
        let end = anchor.as_ref().map_or(observed_at, |s| s.time);
        self.output += usage.output;
        self.secs += stream_ms / 1000.0;
        // agents are parsed one file at a time, so calls arrive out of
        // time order: keep the list sorted by end
        let at = self.recent.partition_point(|c| c.end <= end);
        self.recent.insert(
            at,
            Call {
                agent: agent.to_string(),
                start: end - stream_ms,
                end,
                output: usage.output,
                ttft: step.ttft_ms.unwrap_or(0.0),
                anchored: anchor.is_some(),
            },
        );
        if self.recent.len() > Self::RECENT_CAP {
            self.recent.remove(0);
        }
    }

    /// The most recently finished call, from any agent.
    pub fn last(&self) -> Option<&Call> {
        self.recent.last()
    }

    /// Session average, token-weighted: all output tokens over all
    /// streaming seconds. (A mean of per-call rates would let a handful of
    /// short, bursty calls skew it.)
    pub fn average(&self) -> Option<f64> {
        (self.secs > 0.0).then(|| self.output as f64 / self.secs)
    }

    /// Combined rate over the `window_secs` before the latest call ended.
    /// Overlapping streams add up (three sub-agents at 30 tok/s each are
    /// 90 tok/s); a call reaching back before the window counts only the
    /// share of its tokens that falls inside it.
    pub fn throughput(&self, window_secs: f64) -> Option<Throughput> {
        let end = self.recent.last()?.end;
        let from = end - window_secs * 1000.0;
        let mut tokens = 0.0;
        let mut earliest = end;
        // (time, +1 start / -1 end) for a sweep: peak overlap is how many
        // agents were truly streaming together. Counting every agent that
        // touched the window would also count a main-agent call that ended
        // just before the sub-agents it spawned started.
        let mut edges: Vec<(f64, i32)> = Vec::new();
        for c in self.recent.iter().filter(|c| c.anchored && c.end > from) {
            let start = c.start.max(from);
            tokens += c.output as f64 * (c.end - start) / (c.end - c.start);
            earliest = earliest.min(start);
            edges.push((start, 1));
            edges.push((c.end, -1));
        }
        // ends sort before starts at the same instant: back-to-back calls
        // don't overlap
        edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (mut live, mut peak) = (0i32, 0i32);
        for (_, d) in edges {
            live += d;
            peak = peak.max(live);
        }
        let span = (end - earliest) / 1000.0;
        (span > 0.0).then(|| Throughput {
            tokens_per_sec: tokens / span,
            agents: peak as usize,
        })
    }
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
    pub speed: Speed,
    pub goal: Option<GoalState>,
    /// A cleared event-based goal must not fall back to stale legacy metadata.
    pub goal_events_seen: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GoalState {
    pub status: String,
    pub turns_used: u64,
    pub wall_clock_ms: f64,
    pub wall_clock_resumed_at: Option<f64>,
    #[serde(rename = "budget")]
    pub budget_limits: serde_json::Value,
}

impl SessionStats {
    fn apply_goal(&mut self, kind: &str, record: &serde_json::Value) {
        if kind == "forked" && !self.goal_events_seen {
            return;
        }
        self.goal_events_seen = true;
        let resumed_at = crate::upstream::wire::goal_resumed_at(kind, record);
        match kind {
            "goal.create" => {
                self.goal = Some(GoalState {
                    status: "active".into(),
                    wall_clock_resumed_at: resumed_at,
                    ..Default::default()
                })
            }
            "goal.clear" | "forked" => self.goal = None,
            "goal.update" => {
                let Some(goal) = self.goal.as_mut() else {
                    return;
                };
                if let Some(status) = record.get("status").and_then(|v| v.as_str()) {
                    if goal.status != status {
                        goal.status = status.to_string();
                        goal.wall_clock_resumed_at = None;
                    }
                }
                if goal.status == "active" {
                    if let Some(at) = resumed_at {
                        goal.wall_clock_resumed_at = Some(at);
                    }
                }
                if let Some(turns) = record.get("turnsUsed").and_then(|v| v.as_u64()) {
                    goal.turns_used = turns;
                }
                if let Some(ms) = record.get("wallClockMs").and_then(|v| v.as_f64()) {
                    goal.wall_clock_ms = ms;
                }
                if let Some(budget) = record.get("budgetLimits") {
                    goal.budget_limits = budget.clone();
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// session resolution
// ---------------------------------------------------------------------------

/// Resolve only the requested session. Startup can have an empty ID or an
/// ID whose directory has not been created yet; neither means "resume latest".
#[derive(Default, Serialize, Deserialize)]
struct Location {
    dir: Option<PathBuf>,
    offset: u64,
    mid: bool,
}

pub fn find_session_dir(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    let home = paths::kimi_home();
    let cache = paths::cache_dir().join(format!("location-{}.json", paths::short_hash(session_id)));
    let mut location: Location = std::fs::read(&cache)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if let Some(dir) = location.dir.as_ref().filter(|d| d.is_dir()) {
        return Some(dir.clone());
    }
    // A moved/deleted directory invalidates the old index cursor as well.
    if location.dir.take().is_some() {
        location = Location::default();
    }
    let index = home.join("session_index.jsonl");
    if let Ok(size) = index.metadata().map(|m| m.len()) {
        if size < location.offset {
            location = Location::default();
        }
        while location.offset < size {
            let (chunk, offset, mid) = read_tail(&index, location.offset, location.mid, size);
            let chunk_start = offset - chunk.len() as u64;
            location.offset = chunk_start;
            location.mid = false;
            for line in chunk.split_inclusive(|&b| b == b'\n') {
                location.offset += line.len() as u64;
                if !std::str::from_utf8(line).is_ok_and(|s| s.contains(session_id)) {
                    continue;
                }
                let Ok(rec) = serde_json::from_slice::<serde_json::Value>(line) else {
                    continue;
                };
                if rec.get("sessionId").and_then(|v| v.as_str()) == Some(session_id) {
                    if let Some(dir) = rec
                        .get("sessionDir")
                        .and_then(|v| v.as_str())
                        .map(PathBuf::from)
                    {
                        location.dir = Some(dir);
                        break;
                    }
                }
            }
            if location.dir.is_some() || location.offset < offset {
                break;
            }
            location.mid = mid;
            if chunk.is_empty() {
                location.offset = offset;
                break;
            }
        }
    }
    if location.dir.is_none() {
        'search: for name in [session_id.to_string(), format!("session_{session_id}")] {
            let Ok(entries) = std::fs::read_dir(home.join("sessions")) else {
                break;
            };
            for wd in entries.filter_map(Result::ok) {
                let candidate = wd.path().join(&name);
                if candidate.is_dir() {
                    location.dir = Some(candidate);
                    break 'search;
                }
            }
        }
    }
    if let Ok(bytes) = serde_json::to_vec(&location) {
        let _ = paths::write_atomic(&cache, &bytes);
    }
    location.dir.filter(|d| d.is_dir())
}

/// Select a recent session explicitly for the preview/configurator.
pub fn latest_session_dir(cwd: &str) -> Option<PathBuf> {
    if cwd.is_empty() {
        return None;
    }
    let home = paths::kimi_home();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for wd in read_dirs(&home.join("sessions")) {
        for session in read_dirs(&wd) {
            let state = session.join("state.json");
            let Ok(meta) = state.metadata() else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            if best.as_ref().is_some_and(|(t, _)| *t >= mtime) {
                continue;
            }
            let Some(v) = read_json(&state) else { continue };
            let rec_cwd = v
                .get("cwd")
                .or_else(|| v.get("workDir"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if !same_path(rec_cwd, cwd) {
                continue;
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

fn file_identity(file: &File) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let handle = same_file::Handle::from_file(file.try_clone().ok()?).ok()?;
    // A compiler change may change this hash; that only forces a safe replay.
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    handle.hash(&mut hash);
    Some(hash.finish())
}

/// Read what was appended after `start`. Returns (complete lines, new offset,
/// mid). A trailing partial line (writer mid-append) is left for next time.
fn read_tail(path: &Path, start: u64, mid: bool, size: u64) -> (Vec<u8>, u64, bool) {
    let Ok(file) = File::open(path) else {
        return (Vec::new(), start, mid);
    };
    read_tail_file(&file, start, mid, size)
}

fn read_tail_file(mut file: &File, start: u64, mid: bool, size: u64) -> (Vec<u8>, u64, bool) {
    let remaining = size.saturating_sub(start);
    if remaining == 0 {
        return (Vec::new(), start, mid);
    }
    let want = remaining.min(MAX_TAIL_BYTES);
    let mut buf = Vec::with_capacity(want as usize);
    let read = file
        .seek(SeekFrom::Start(start))
        .and_then(|_| file.take(want).read_to_end(&mut buf));
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
            None => return (Vec::new(), start + chunk.len() as u64, true),
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

/// Fold everything appended since `prior` into fresh stats.
#[cfg(test)]
pub fn parse_session(dir: &Path, prior: Option<SessionStats>) -> SessionStats {
    parse_session_until(dir, prior, Instant::now() + PARSE_BUDGET)
}

pub fn parse_session_until(
    dir: &Path,
    prior: Option<SessionStats>,
    deadline: Instant,
) -> SessionStats {
    let deadline = deadline.min(Instant::now() + PARSE_BUDGET);
    parse_session_with_budget(dir, prior, &mut || Instant::now() < deadline)
}

fn parse_session_with_budget(
    dir: &Path,
    prior: Option<SessionStats>,
    has_time: &mut dyn FnMut() -> bool,
) -> SessionStats {
    let mut st = prior.unwrap_or_default();
    st.v = CACHE_VERSION;

    let mut agents = read_dirs(&dir.join("agents"));
    // main first: it carries the mode/effort state the prefix needs
    agents.sort_by_key(|p| p.file_name().map(|n| n != "main"));
    for agent_dir in agents {
        if !has_time() {
            break;
        }
        let wire = agent_dir.join("wire.jsonl");
        let agent = agent_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_main = agent == "main";
        let Ok(file) = File::open(&wire) else {
            continue;
        };
        let Ok(size) = file.metadata().map(|m| m.len()) else {
            continue;
        };
        // Identity, length and bytes must come from the same open file: the
        // path can be atomically replaced by wireService.ts during migration.
        let identity = file_identity(&file);
        let mut cur = st.files.get(&agent).cloned().unwrap_or_default();
        if size < cur.offset || (cur.offset > 0 && cur.identity != identity) {
            // truncated/replaced wire: per-file subtotals aren't kept, so
            // start the whole session over
            paths::debug(&format!("{agent}: wire replaced or shrank, full re-parse"));
            // the goal clock is anchored in wall time, not in the wires:
            // carry it over so a live goal doesn't restart from zero
            let mut fresh = parse_session_with_budget(dir, None, has_time);
            fresh.goal_key = st.goal_key;
            fresh.goal_seen_at = st.goal_seen_at;
            return fresh;
        }
        cur.identity = identity;
        let (chunk, offset, mid) = read_tail_file(&file, cur.offset, cur.mid, size);
        cur.offset = offset - chunk.len() as u64;
        cur.mid = false;
        for line in chunk.split_inclusive(|&b| b == b'\n') {
            if !has_time() {
                break;
            }
            cur.offset += line.len() as u64;
            let Some(kind) = record_type(line) else {
                continue;
            };
            if !WANTED.contains(&kind) {
                continue;
            }
            if kind == "context.append_loop_event" {
                // the vast majority are tool calls/results; only step.end
                // (which carries the call's timing) is worth decoding
                const NEEDLE: &[u8] = br#"{"type":"step."#;
                if !line[..line.len().min(256)]
                    .windows(NEEDLE.len())
                    .any(|w| w == NEEDLE)
                {
                    continue;
                }
                if let Ok(ev) = serde_json::from_slice::<LoopEvent>(line) {
                    if let Some(step) = ev.event {
                        match step.kind.as_str() {
                            "step.begin" => cur.stream_end = None,
                            "step.end" => {
                                st.speed
                                    .record_step(&agent, &step, ev.time, cur.stream_end.take())
                            }
                            _ => {}
                        }
                    }
                }
                continue;
            }
            match kind {
                "goal.create" | "goal.update" | "goal.clear" | "forked" => {
                    if is_main {
                        if let Ok(record) = serde_json::from_slice(line) {
                            st.apply_goal(kind, &record);
                        }
                    }
                    continue;
                }
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
            let Ok(rec) = serde_json::from_slice::<RecordFields>(line) else {
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
                    if rec.usage_scope.as_deref() == Some("turn") {
                        cur.stream_end = rec.time.map(|time| StreamEnd { time, usage: u });
                    }
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
        if cur.offset == offset {
            cur.mid = mid;
        }
        st.files.insert(agent, cur);
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_generated_wires_agree_before_and_after_migration() {
        let dir = std::env::temp_dir().join(format!("ksl-golden-{}", std::process::id()));
        let agent = dir.join("agents/main");
        std::fs::create_dir_all(&agent).unwrap();
        let wire = agent.join("wire.jsonl");
        let mut prior = None;
        for fixture in [
            include_str!("../tests/fixtures/wire-v1.4.jsonl"),
            include_str!("../tests/fixtures/wire-v1.5.jsonl"),
        ] {
            let replacement = agent.join("replacement");
            std::fs::write(&replacement, fixture).unwrap();
            std::fs::rename(replacement, &wire).unwrap();
            let mut stats = parse_session(&dir, prior);
            normalize_goal(&mut stats, None);
            assert_eq!(stats.total.input(), 1000);
            assert_eq!(stats.total.output, 420);
            assert_eq!(stats.total.cache_rate(), Some(90.0));
            let call = stats.speed.last().unwrap();
            assert_eq!(
                (call.start, call.end, call.rate(), call.anchored),
                (5000.0, 15000.0, 42.0, true)
            );
            let goal = stats.goal.as_ref().unwrap();
            assert_eq!(goal.status, "active");
            assert_eq!(goal.turns_used, 1);
            assert_eq!(goal.budget_limits["turnBudget"], 8);
            assert_eq!(goal.wall_clock_ms, 30000.0);
            assert_eq!(stats.goal_seen_at, Some(41.0));
            prior = Some(stats);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_wire_rewrite_larger_than_cursor_replays_usage_once() {
        let dir = std::env::temp_dir().join(format!("ksl-rewrite-{}", std::process::id()));
        let agent = dir.join("agents/main");
        std::fs::create_dir_all(&agent).unwrap();
        let wire = agent.join("wire.jsonl");
        let usage = "{\"type\":\"usage.record\",\"usage\":{\"output\":7}}\n";
        std::fs::write(&wire, usage).unwrap();
        let first = parse_session(&dir, None);
        assert_eq!(first.total.output, 7);
        // Shift the old record beyond the previous offset: a size-only
        // cursor would count it twice, producing 21 instead of 14 tokens.
        let replacement = agent.join("replacement");
        let header = format!(
            "{{\"type\":\"metadata\",\"protocol_version\":\"1.5\",\"padding\":\"{}\"}}\n",
            "x".repeat(usage.len())
        );
        assert!(header.len() > first.files["main"].offset as usize);
        std::fs::write(&replacement, format!("{header}{usage}{usage}")).unwrap();
        std::fs::rename(replacement, &wire).unwrap();
        let replayed = parse_session(&dir, Some(first));
        assert_eq!(replayed.total.output, 14);
        assert_eq!(parse_session(&dir, Some(replayed)).total.output, 14);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_goal_accounting_advances_anchor_but_budget_updates_do_not() {
        let mut st = SessionStats::default();
        for (kind, record, anchor) in [
            (
                "goal.create",
                serde_json::json!({"time":1000}),
                Some(1000.0),
            ),
            (
                "goal.update",
                serde_json::json!({"wallClockMs":4000,"time":5000}),
                Some(5000.0),
            ),
            (
                "goal.update",
                serde_json::json!({"budgetLimits":{"turnBudget":10},"time":6000}),
                Some(5000.0),
            ),
            (
                "goal.update",
                serde_json::json!({"status":"paused","time":7000}),
                None,
            ),
            (
                "goal.update",
                serde_json::json!({"status":"active","time":8000}),
                Some(8000.0),
            ),
            (
                "goal.update",
                serde_json::json!({"status":"active","time":9000,"wallClockResumedAt":8500}),
                Some(8500.0),
            ),
        ] {
            st.apply_goal(kind, &record);
            assert_eq!(st.goal.as_ref().unwrap().wall_clock_resumed_at, anchor);
        }
    }

    /// Upstream records usage when the stream returns, then ends the step.
    fn step_end(end: u64, out: u64, stream: u64, ttft: u64) -> String {
        let usage = format!(
            r#"{{"type":"usage.record","usageScope":"turn","usage":{{"inputOther":1,"output":{out}}},"time":{end}}}"#
        );
        usage
            + "\n"
            + &format!(
                r#"{{"type":"context.append_loop_event","agentId":"x","event":{{"type":"step.end","uuid":"u","step":1,"finishReason":"tool_use","usage":{{"inputOther":1,"output":{out},"inputCacheRead":0,"inputCacheCreation":0}},"llmFirstTokenLatencyMs":{ttft},"llmStreamDurationMs":{stream}}},"time":{end}}}"#
            )
    }

    #[test]
    fn budget_stop_preserves_cursor_and_pending_stream_across_runs() {
        let dir = std::env::temp_dir().join(format!("ksl-budget-{}", std::process::id()));
        let agent = dir.join("agents/main");
        std::fs::create_dir_all(&agent).unwrap();
        let body = step_end(15000, 400, 10000, 5000) + "\n";
        std::fs::write(agent.join("wire.jsonl"), &body).unwrap();
        let mut checks = 0;
        let st = parse_session_with_budget(&dir, None, &mut || {
            checks += 1;
            checks <= 2
        });
        assert_eq!(st.total.output, 400);
        assert!(st.speed.last().is_none());
        assert!(st.files["main"].offset < body.len() as u64);
        let cached: SessionStats =
            serde_json::from_slice(&serde_json::to_vec(&st).unwrap()).unwrap();
        let resumed = parse_session_with_budget(&dir, Some(cached), &mut || true);
        assert_eq!(resumed.total.output, 400, "usage must not be counted twice");
        assert_eq!(resumed.speed.last().unwrap().rate(), 40.0);
        assert!(resumed.speed.last().unwrap().anchored);
        assert_eq!(resumed.files["main"].offset, body.len() as u64);
        let unchanged = parse_session_until(&dir, Some(resumed.clone()), Instant::now());
        assert_eq!(unchanged, resumed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_timing_keeps_rate_without_inventing_parallelism() {
        let mut speed = Speed::default();
        let ev: LoopEvent =
            serde_json::from_str(step_end(60000, 400, 10000, 0).lines().nth(1).unwrap()).unwrap();
        let step = ev.event.unwrap();
        speed.record_step("main", &step, ev.time, None);
        speed.record_step("child", &step, ev.time, None);
        assert_eq!(speed.last().unwrap().rate(), 40.0);
        assert_eq!(speed.average(), Some(40.0));
        assert!(speed.throughput(30.0).is_none());
    }

    #[test]
    fn skipped_long_record_waits_for_its_newline_before_resuming() {
        let path = std::env::temp_dir().join(format!("ksl-tail-{}", std::process::id()));
        std::fs::write(&path, "unfinished tool output").unwrap();
        let size = path.metadata().unwrap().len();
        let (chunk, offset, mid) = read_tail(&path, 0, true, size);
        assert!(chunk.is_empty() && mid);
        assert_eq!(offset, size);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        writeln!(file, " still the same record").unwrap();
        writeln!(file, r#"{{"type":"usage.record","usage":{{"output":7}}}}"#).unwrap();
        let (chunk, offset, mid) = read_tail(&path, offset, mid, path.metadata().unwrap().len());
        assert!(chunk.starts_with(b"{\"type\":\"usage.record\""));
        assert!(!mid);
        assert_eq!(offset, path.metadata().unwrap().len());
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn speed_uses_stream_time_not_wall_time() {
        let dir = std::env::temp_dir().join(format!("kimi-sl-speed-{}", std::process::id()));
        for a in ["main", "agent-0", "agent-1"] {
            std::fs::create_dir_all(dir.join("agents").join(a)).unwrap();
        }
        let wire = |a: &str, lines: &[String]| {
            let body = lines.join("\n") + "\n";
            std::fs::write(dir.join("agents").join(a).join("wire.jsonl"), body).unwrap()
        };
        // main: 400 tokens streamed in 10 s after a 5 s first-token wait:
        // 40 tok/s (wall clock would say 26.7). Then a 150 ms burst of 60
        // tokens (a tool-call stub) that must not count.
        // A tool result that merely quotes "step.end" must not count either.
        let quoted = r#"{"type":"context.append_loop_event","event":{"type":"tool.result","output":"{\"type\":\"step.end\""}}"#;
        wire(
            "main",
            &[
                step_end(15000, 400, 10000, 5000),
                step_end(16000, 60, 150, 800),
                quoted.into(),
            ],
        );
        // two sub-agents streaming in parallel, 10 s each at 30 tok/s
        wire("agent-0", &[step_end(25000, 300, 10000, 2000)]);
        wire("agent-1", &[step_end(24000, 300, 10000, 2000)]);

        let st = parse_session(&dir, None);
        let main_call = st.speed.recent.iter().find(|c| c.agent == "main").unwrap();
        assert_eq!(main_call.rate(), 40.0);
        assert_eq!(main_call.ttft, 5000.0);
        assert_eq!(st.speed.recent.len(), 3, "stub burst excluded");
        let last = st.speed.last().unwrap();
        assert_eq!((last.agent.as_str(), last.rate()), ("agent-0", 30.0));
        // token-weighted: 1000 tokens over 30 s of streaming
        assert_eq!(st.speed.average(), Some(1000.0 / 30.0));
        // last 12 s (13-25 s): both sub-agents fully inside (600 tokens),
        // plus the 2 s tail of main's 5-15 s stream (80 of its 400) → 680
        // tokens over 12 s
        let tp = st.speed.throughput(12.0).unwrap();
        assert_eq!(tp.agents, 2);
        assert!((tp.tokens_per_sec - 680.0 / 12.0).abs() < 1e-9, "{tp:?}");
        // a window reaching back over main's call: main streamed alone, so
        // the peak is still the two sub-agents
        assert_eq!(st.speed.throughput(60.0).unwrap().agents, 2);
        // last 2 s (23-25 s): both stream for 1 s at 60 tok/s, then only
        // agent-0 for 1 s at 30 tok/s → 90 tokens / 2 s
        let tp = st.speed.throughput(2.0).unwrap();
        assert!((tp.tokens_per_sec - 45.0).abs() < 1e-9, "{tp:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shrunk_wire_reparses_but_keeps_goal_clock() {
        let dir = std::env::temp_dir().join(format!("kimi-sl-shrink-{}", std::process::id()));
        let main = dir.join("agents/main");
        std::fs::create_dir_all(&main).unwrap();
        let rec = |n: u64| {
            format!("{{\"type\":\"usage.record\",\"usage\":{{\"inputOther\":{n},\"output\":1}}}}\n")
        };
        std::fs::write(main.join("wire.jsonl"), rec(100) + &rec(100)).unwrap();
        let mut st = parse_session(&dir, None);
        st.goal_key = Some("g".into());
        st.goal_seen_at = Some(42.0);
        std::fs::write(main.join("wire.jsonl"), rec(7)).unwrap();
        let st = parse_session(&dir, Some(st));
        assert_eq!(st.total.input(), 7);
        assert_eq!(st.goal_seen_at, Some(42.0));
        assert_eq!(st.goal_key.as_deref(), Some("g"));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
