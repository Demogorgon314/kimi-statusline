//! Gather everything a render needs for one payload.

use crate::config::{Config, SegmentId};
use crate::kimi_config::{self, Models};
use crate::payload::Payload;
use crate::render::Ctx;
use crate::{paths, probe, session};
use std::path::Path;
use std::time::{Duration, Instant};

/// Spawned probes (git) are skipped past this point, keeping their stale
/// cached values, so the run stays under the TUI's 300ms cap.
const PROBE_BUDGET: Duration = Duration::from_millis(120);

pub fn collect(payload: Payload, config: Config, started: Instant) -> Ctx {
    let models = Models::load();
    let palette_name = (!config.style.palette.is_empty()).then_some(config.style.palette.as_str());
    let palette = kimi_config::palette(palette_name);
    let now = paths::now_secs();
    let wants = |id: SegmentId| config.segment(id).is_some_and(|s| s.enabled);

    let session_dir = session::find_session_dir(&payload.session_id, &payload.cwd);
    paths::debug(&format!(
        "session={:?} dir={session_dir:?}",
        payload.session_id
    ));

    let mut stats = None;
    let mut goal = None;
    let mut session_created = None;
    if let Some(dir) = &session_dir {
        let cache_key = if payload.session_id.is_empty() {
            dir.to_string_lossy().into_owned()
        } else {
            payload.session_id.clone()
        };
        let prior = session::load_cache(&cache_key);
        let mut st = session::parse_session(dir, prior.clone());

        let state = session::read_json(&dir.join("state.json"));
        session_created = state
            .as_ref()
            .and_then(|s| s.get("createdAt")?.as_f64())
            .map(|ms| ms / 1000.0);
        goal = state
            .and_then(|s| s.get("custom")?.get("goal").cloned())
            .filter(|g| g.is_object());
        // anchor for a live goal's ticking clock: when this snapshot was
        // first seen (upstream's goalObservedAtMs)
        let key = goal.as_ref().map(|g| g.to_string());
        if key != st.goal_key || (key.is_some() && st.goal_seen_at.is_none()) {
            st.goal_seen_at = key.as_ref().map(|_| now);
            st.goal_key = key;
        }
        if prior.as_ref() != Some(&st) {
            session::save_cache(&cache_key, &st);
        }
        stats = Some(st);
    }

    let effort = stats.as_ref().and_then(|s| s.effort.clone()).or_else(|| {
        models
            .efforts
            .get(&payload.model)
            .cloned()
            .map(serde_json::Value::String)
    });
    let tasks = match (&session_dir, wants(SegmentId::Tasks)) {
        (Some(dir), true) => probe::running_tasks(dir),
        _ => (0, 0),
    };

    let git_seg = config.segment(SegmentId::Git).filter(|s| s.enabled);
    let in_repo = payload.git_branch.is_some() && !payload.cwd.is_empty();
    let git = git_seg
        .filter(|s| in_repo && s.opt_bool("status", true))
        .and_then(|_| probe::git_status(&payload.cwd, started.elapsed() > PROBE_BUDGET));
    let pr = match (git_seg, &payload.git_branch) {
        (Some(s), Some(branch)) if in_repo && s.opt_bool("pr", true) => {
            probe::pull_request(&payload.cwd, branch)
        }
        _ => None,
    };
    let dance = wants(SegmentId::Model)
        .then(|| probe::dance_state(&payload.cwd))
        .flatten();

    let quota = config
        .segment(SegmentId::Quota)
        .filter(|s| s.enabled)
        .and_then(|s| crate::quota::get(s.opt_int("refresh_secs", 120).max(30) as f64));

    let color = std::env::var_os("KIMI_STATUSLINE_NO_COLOR").is_none()
        && std::env::var("TERM").map_or(true, |t| t != "dumb");
    let goal_seen_at = stats.as_ref().and_then(|s| s.goal_seen_at);
    Ctx {
        payload,
        config,
        palette,
        models,
        stats,
        effort,
        goal,
        goal_seen_at,
        session_created,
        tasks,
        git,
        pr,
        dance,
        quota,
        now,
        color,
    }
}

/// A payload for the newest session in `cwd`, filled the way the TUI would:
/// for `preview` and the configurator.
pub fn sample_payload(cwd: &str, session_id: Option<String>) -> Payload {
    let mut p = Payload {
        cwd: cwd.to_string(),
        session_id: session_id.unwrap_or_default(),
        permission_mode: "manual".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        ..Default::default()
    };
    p.git_branch = probe::run_with_timeout(
        std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(cwd),
        Duration::from_secs(2),
    )
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());
    if let Some(dir) = session::find_session_dir(&p.session_id, cwd) {
        if let Some(model) = last_model(&dir) {
            p.model = model;
        }
        if p.session_id.is_empty() {
            p.session_id = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        if let Some((tokens, max)) = last_context(&dir) {
            p.context_tokens = tokens;
            p.max_context_tokens = max;
            p.context_usage = tokens as f64 / max.max(1) as f64;
        }
    }
    if p.model.is_empty() {
        p.model = "kimi-code/k3".into();
    }
    p
}

fn main_wire_tail(dir: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(dir.join("agents/main/wire.jsonl")).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(4 << 20))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn last_model(dir: &Path) -> Option<String> {
    main_wire_tail(dir)?.lines().rev().find_map(|l| {
        if !l.starts_with(r#"{"type":"llm.request""#) {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(l).ok()?;
        v.get("modelAlias")?.as_str().map(str::to_string)
    })
}

/// (context tokens, max) from the last request's usage, approximated as its
/// input; max comes from the model's max_context_size when the TUI isn't
/// there to tell us.
fn last_context(dir: &Path) -> Option<(u64, u64)> {
    let tail = main_wire_tail(dir)?;
    let input = tail.lines().rev().find_map(|l| {
        if !l.starts_with(r#"{"type":"usage.record""#) {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(l).ok()?;
        let u = v.get("usage")?;
        let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        Some(n("inputOther") + n("inputCacheRead") + n("inputCacheCreation"))
    })?;
    Some((input, 262_144))
}
