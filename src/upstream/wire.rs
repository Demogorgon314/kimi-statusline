//! Wire contracts from agent-core-v2 at 21406fb4c. Source paths are relative
//! to packages/agent-core-v2/src; symbols are used instead of unstable line numbers.
use serde::{Deserialize, Serialize};

// agent/usage/usageOps.ts: UsageRecord and human/llm/usage.ts: TokenUsage.
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

/// `context.append_loop_event` wrapping a `step.end`: Kimi Code's own
/// measurement of the model call that just finished (see upstream
/// human/timing/plugin.ts).
#[derive(Deserialize)]
pub struct LoopEvent {
    #[serde(default)]
    pub time: Option<f64>,
    #[serde(default)]
    pub event: Option<StepEnd>,
}

#[derive(Deserialize)]
pub struct StepEnd {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub usage: Option<Usage>,
    /// first streamed delta → stream done: the decode phase only
    #[serde(rename = "llmStreamDurationMs", default)]
    pub stream_ms: Option<f64>,
    /// request sent → first delta (queueing + prefill)
    #[serde(rename = "llmFirstTokenLatencyMs", default)]
    pub ttft_ms: Option<f64>,
}

// app/event/event2.ts: Event2.serialize always writes type first.
/// The record's own `"type"` when the line starts with it. Matching the
/// leading key (not a substring search) keeps tool output that merely quotes
/// a record name from being taken for the record itself.
pub fn record_type(line: &[u8]) -> Option<&str> {
    let rest = line.strip_prefix(b"{\"type\":\"")?;
    let end = rest.iter().take(48).position(|&b| b == b'"')?;
    std::str::from_utf8(&rest[..end]).ok()
}

pub const WANTED: [&str; 13] = [
    "usage.record",
    "profile.bind",
    "llm.request",
    "config.update",
    "swarm_mode.enter",
    "swarm_mode.exit",
    "tower_mode.enter",
    "tower_mode.exit",
    "context.append_loop_event",
    "goal.create",
    "goal.update",
    "goal.clear",
    "forked",
];

#[derive(Deserialize)]
pub struct RecordFields {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, rename = "modelAlias")]
    pub model_alias: Option<String>,
    #[serde(default, rename = "thinkingEffort")]
    pub thinking_effort: Option<serde_json::Value>,
    #[serde(default, rename = "thinkingLevel")]
    pub thinking_level: Option<serde_json::Value>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub time: Option<f64>,
    #[serde(default, rename = "usageScope")]
    pub usage_scope: Option<String>,
}

/// wire/migration/v1.5.ts: active interval anchors in pre-1.5 records.
pub fn goal_resumed_at(kind: &str, record: &serde_json::Value) -> Option<f64> {
    let advances_interval = kind == "goal.create"
        || (kind == "goal.update"
            && (record.get("status").and_then(|v| v.as_str()) == Some("active")
                || (record.get("status").is_none()
                    && record.get("wallClockMs").is_some_and(|v| v.is_number()))));
    record
        .get("wallClockResumedAt")
        .or_else(|| advances_interval.then(|| record.get("time")).flatten())
        .and_then(|v| v.as_f64())
}
