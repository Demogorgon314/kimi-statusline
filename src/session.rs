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

const CACHE_VERSION: u32 = 3;
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

/// `context.append_loop_event` wrapping a `step.end`: Kimi Code's own
/// measurement of the model call that just finished (see upstream
/// human/timing/plugin.ts).
#[derive(Deserialize)]
struct LoopEvent {
    #[serde(default)]
    time: Option<f64>,
    #[serde(default)]
    event: Option<StepEnd>,
}

#[derive(Deserialize)]
struct StepEnd {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    usage: Option<Usage>,
    /// first streamed delta → stream done: the decode phase only
    #[serde(rename = "llmStreamDurationMs", default)]
    stream_ms: Option<f64>,
    /// request sent → first delta (queueing + prefill)
    #[serde(rename = "llmFirstTokenLatencyMs", default)]
    ttft_ms: Option<f64>,
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

    fn record_step(&mut self, agent: &str, step: &StepEnd, line_time: Option<f64>) {
        let (Some(stream_ms), Some(usage), Some(end)) = (step.stream_ms, step.usage, line_time)
        else {
            return;
        };
        if stream_ms < Self::MIN_STREAM_MS || usage.output == 0 {
            return;
        }
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
        for c in self.recent.iter().filter(|c| c.end > from) {
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
}

// ---------------------------------------------------------------------------
// session resolution
// ---------------------------------------------------------------------------

/// Resolve only the requested session. Startup can have an empty ID or an
/// ID whose directory has not been created yet; neither means "resume latest".
pub fn find_session_dir(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    let home = paths::kimi_home();
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
    None
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

const WANTED: [&str; 9] = [
    "usage.record",
    "profile.bind",
    "llm.request",
    "config.update",
    "swarm_mode.enter",
    "swarm_mode.exit",
    "tower_mode.enter",
    "tower_mode.exit",
    "context.append_loop_event",
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
            if kind == "context.append_loop_event" {
                // the vast majority are tool calls/results; only step.end
                // (which carries the call's timing) is worth decoding
                const NEEDLE: &[u8] = br#"{"type":"step.end""#;
                if !line[..line.len().min(160)]
                    .windows(NEEDLE.len())
                    .any(|w| w == NEEDLE)
                {
                    continue;
                }
                if let Ok(ev) = serde_json::from_slice::<LoopEvent>(line) {
                    if let Some(step) = ev.event.filter(|e| e.kind == "step.end") {
                        st.speed.record_step(&agent, &step, ev.time);
                    }
                }
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
                    // request → usage wall time covers the whole call
                    // (time to first token included), so this is the speed
                    // you actually experience, not the decoder's peak
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

    /// A step.end loop event as Kimi Code writes it: stream finished at
    /// `end` ms after streaming for `stream` ms.
    fn step_end(end: u64, out: u64, stream: u64, ttft: u64) -> String {
        format!(
            r#"{{"type":"context.append_loop_event","agentId":"x","event":{{"type":"step.end","uuid":"u","step":1,"finishReason":"tool_use","usage":{{"inputOther":1,"output":{out},"inputCacheRead":0,"inputCacheCreation":0}},"llmFirstTokenLatencyMs":{ttft},"llmStreamDurationMs":{stream}}},"time":{end}}}"#
        )
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
