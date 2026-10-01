//! Segment rendering and width fitting.
//!
//! Each segment yields spans; a span may carry its own color (the cache-rate
//! ramp, mode badges, the /dance rainbow…), otherwise it takes the segment's
//! text color. Styles are closed with `ESC[22m ESC[39m` / `ESC[49m` rather
//! than a full reset: the TUI wraps the line in chalk.hex(colors.text), and
//! chalk re-opens its color at each of its own close codes, whereas `ESC[0m`
//! would leave the rest of the line in the terminal's default foreground.

use crate::config::{AnsiColor, Config, Lang, SegmentConfig, SegmentId, StyleMode};
use crate::kimi_config::{Models, Palette, Rgb};
use crate::payload::Payload;
use crate::probe::{Dance, GitStatus, PullRequest};
use crate::session::{SessionStats, Usage};
use unicode_width::UnicodeWidthChar;

const CLOSE_FG: &str = "\x1b[22m\x1b[39m";
const CLOSE_BG: &str = "\x1b[49m";
const POWERLINE_ARROW: &str = "\u{e0b0}";

/// Everything a render needs, gathered up front so fitting can re-render
/// cheaply at each degradation stage.
pub struct Ctx {
    pub payload: Payload,
    pub config: Config,
    pub palette: Palette,
    pub models: Models,
    pub stats: Option<SessionStats>,
    /// effort from the wire, else the model's default_effort
    pub effort: Option<serde_json::Value>,
    pub goal: Option<serde_json::Value>,
    pub goal_seen_at: Option<f64>,
    pub session_created: Option<f64>,
    pub tasks: (u32, u32),
    pub git: Option<GitStatus>,
    pub pr: Option<PullRequest>,
    pub dance: Option<Dance>,
    pub quota: Option<crate::quota::Quota>,
    pub now: f64,
    pub color: bool,
}

struct Span {
    text: String,
    color: Option<AnsiColor>,
    bold: bool,
    /// SGR faint: dims whatever color ends up applied, so it also works on
    /// powerline backgrounds where the segment's text color wins
    dim: bool,
}

fn plain(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        color: None,
        bold: false,
        dim: false,
    }
}

fn colored(text: impl Into<String>, color: AnsiColor) -> Span {
    Span {
        text: text.into(),
        color: Some(color),
        bold: false,
        dim: false,
    }
}

fn token(name: &str) -> AnsiColor {
    AnsiColor::Named(name.into())
}

fn rgb(Rgb(r, g, b): Rgb) -> AnsiColor {
    AnsiColor::Rgb { r, g, b }
}

impl Ctx {
    fn zh(&self) -> bool {
        self.config.style.lang == Lang::Zh
    }

    fn sgr(&self, color: &AnsiColor, bg: bool) -> Option<String> {
        self.color.then(|| color.sgr(&self.palette, bg)).flatten()
    }
}

// ---------------------------------------------------------------------------
// formatting helpers
// ---------------------------------------------------------------------------

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// Modern providers sit at 95%+ almost always, so up there one decimal is
/// kept; only a true 100% stays an integer.
fn fmt_rate(r: f64) -> String {
    if r >= 100.0 {
        "100%".into()
    } else if r >= 95.0 {
        format!("{:.1}%", r.min(99.9))
    } else {
        format!("{}%", r.round() as i64)
    }
}

fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb(to(r), to(g), to(b))
}

/// (rate%, hue, saturation, lightness): brick red through amber into jade,
/// with most of the resolution spent above 95% where real sessions live.
const CACHE_STOPS: [(f64, f64, f64, f64); 9] = [
    (0.0, 2.0, 0.55, 0.52),
    (50.0, 22.0, 0.58, 0.50),
    (75.0, 38.0, 0.60, 0.49),
    (90.0, 55.0, 0.58, 0.47),
    (95.0, 80.0, 0.52, 0.46),
    (97.0, 100.0, 0.48, 0.45),
    (98.0, 112.0, 0.46, 0.45),
    (99.0, 124.0, 0.44, 0.45),
    (100.0, 140.0, 0.42, 0.45),
];

fn cache_color(rate: f64) -> Rgb {
    let rate = rate.clamp(0.0, 100.0);
    for w in CACHE_STOPS.windows(2) {
        let (r0, h0, s0, l0) = w[0];
        let (r1, h1, s1, l1) = w[1];
        if rate <= r1 {
            let t = (rate - r0) / (r1 - r0);
            return hsl_to_rgb(h0 + (h1 - h0) * t, s0 + (s1 - s0) * t, l0 + (l1 - l0) * t);
        }
    }
    let (_, h, s, l) = CACHE_STOPS[CACHE_STOPS.len() - 1];
    hsl_to_rgb(h, s, l)
}

/// Upstream shortenCwd: `~` for home, otherwise the last N segments.
pub fn shorten_cwd(path: &str, depth: usize) -> String {
    let mut path = path.replace('\\', "/");
    if let Some(home) = crate::paths::home_dir() {
        let home = home.to_string_lossy().replace('\\', "/");
        if path == home {
            return "~".into();
        }
        if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
            path = format!("~/{rest}");
        }
    }
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if depth == 0 || segs.len() <= depth {
        return path;
    }
    format!("…/{}", segs[segs.len() - depth..].join("/"))
}

fn version_tuple(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.split('.').map(|p| {
        p.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

/// OSC 8 hyperlink, like upstream toTerminalHyperlink (http(s) only).
fn hyperlink(text: &str, url: &str) -> String {
    if url.starts_with("https://") || url.starts_with("http://") {
        format!("\x1b]8;;{url}\x07{text}\x1b]8;;\x07")
    } else {
        text.to_string()
    }
}

// /dance palettes from upstream src/tui/easter-eggs/dance.ts
const DARK_RAINBOW: [Rgb; 8] = [
    Rgb(0x4F, 0xA8, 0xFF),
    Rgb(0x5B, 0xC0, 0xBE),
    Rgb(0x4E, 0xC8, 0x7E),
    Rgb(0xE8, 0xA8, 0x38),
    Rgb(0xFF, 0xCB, 0x6B),
    Rgb(0xC6, 0x78, 0xB8),
    Rgb(0xA2, 0x74, 0xD9),
    Rgb(0x7C, 0x8D, 0xFF),
];
const LIGHT_RAINBOW: [Rgb; 9] = [
    Rgb(0x15, 0x65, 0xC0),
    Rgb(0x00, 0x83, 0x8F),
    Rgb(0x0E, 0x7A, 0x38),
    Rgb(0x92, 0x66, 0x0A),
    Rgb(0x9A, 0x4A, 0x00),
    Rgb(0xB9, 0x1C, 0x1C),
    Rgb(0x8A, 0x3A, 0x75),
    Rgb(0x6B, 0x3A, 0x9A),
    Rgb(0x35, 0x4C, 0xB5),
];

/// Upstream rainbowText: one palette step per non-space character.
fn rainbow(text: &str, phase: usize, light: bool) -> Vec<Span> {
    let palette: &[Rgb] = if light { &LIGHT_RAINBOW } else { &DARK_RAINBOW };
    let mut i = phase;
    text.chars()
        .map(|ch| {
            if ch == ' ' {
                return plain(" ");
            }
            let c = palette[i % palette.len()];
            i += 1;
            colored(ch.to_string(), rgb(c))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// segments
// ---------------------------------------------------------------------------

/// The spans of one segment, or None when it has nothing to show. `compact`
/// trims labels and units for narrow terminals.
fn segment(ctx: &Ctx, seg: &SegmentConfig, compact: bool) -> Option<Vec<Span>> {
    let p = &ctx.payload;
    let zh = ctx.zh();
    match seg.id {
        SegmentId::Mode => {
            // pre-0.40.0 TUIs drew the raw mode names
            let legacy = version_tuple(&p.version).is_some_and(|v| v < (0, 40, 0));
            let mut names = Vec::new();
            match p.permission_mode.as_str() {
                "auto" => names.push((if legacy { "auto" } else { "Never Ask" }, "warning")),
                "yolo" => names.push((if legacy { "yolo" } else { "Ask When Needed" }, "warning")),
                _ => {}
            }
            if p.plan_mode {
                names.push(("plan", "primary"));
            }
            if let Some(st) = &ctx.stats {
                if st.swarm {
                    names.push(("swarm", "accent"));
                }
                if st.tower {
                    names.push(("tower", "accent"));
                }
            }
            let mut spans = Vec::new();
            for (i, (name, tok)) in names.into_iter().enumerate() {
                if i > 0 {
                    spans.push(plain(" "));
                }
                spans.push(Span {
                    text: name.into(),
                    color: Some(token(tok)),
                    bold: true,
                    dim: false,
                });
            }
            (!spans.is_empty()).then_some(spans)
        }
        SegmentId::Goal => goal_badge(ctx, compact),
        SegmentId::Model => {
            if p.model.is_empty() {
                return None;
            }
            let name = ctx.models.display(&p.model);
            let supports = ctx
                .models
                .has_efforts
                .get(&p.model)
                .copied()
                .unwrap_or(true);
            let thinking = match &ctx.effort {
                Some(serde_json::Value::Bool(true)) => " thinking".to_string(),
                Some(serde_json::Value::String(e)) if e == "on" || (!supports && e != "off") => {
                    " thinking".to_string()
                }
                Some(serde_json::Value::String(e)) if e != "off" => {
                    if compact {
                        format!(" {e}")
                    } else {
                        format!(" thinking: {e}")
                    }
                }
                _ => String::new(),
            };
            let label = format!("{name}{thinking}");
            match ctx
                .dance
                .filter(|_| seg.opt_bool("dance", true) && ctx.color)
            {
                // The TUI reruns us about once a second, so upstream's 110ms
                // frames can't be replayed; one palette step per run gives a
                // slow glide instead of a jumpy one. The hold freezes at the
                // phase upstream's 3s flow lands on (3000 / 110 ≈ 27).
                Some(Dance::Flow) => {
                    Some(rainbow(&label, ctx.now as usize, ctx.palette.is_light()))
                }
                Some(Dance::Hold) => Some(rainbow(&label, 27, ctx.palette.is_light())),
                None => Some(vec![plain(label)]),
            }
        }
        SegmentId::Tasks => {
            let (bash, agent) = ctx.tasks;
            let mut out = Vec::new();
            let mut push = |n: u32, one: &str, many: &str| {
                if n == 0 {
                    return;
                }
                if !out.is_empty() {
                    out.push(plain(" "));
                }
                let noun = if n == 1 { one } else { many };
                out.push(plain(if compact {
                    format!("{n} {noun}")
                } else {
                    format!("[{n} {noun} running]")
                }));
            };
            push(bash, "task", "tasks");
            push(agent, "agent", "agents");
            (!out.is_empty()).then_some(out)
        }
        SegmentId::Directory => {
            if p.cwd.is_empty() {
                return None;
            }
            let depth = if compact {
                1
            } else {
                seg.opt_int("depth", 3).max(0) as usize
            };
            Some(vec![plain(shorten_cwd(&p.cwd, depth))])
        }
        SegmentId::Git => {
            let branch = p.git_branch.as_deref().filter(|b| !b.is_empty())?;
            let mut text = branch.to_string();
            if let Some(g) = ctx.git {
                let mut parts = Vec::new();
                if g.added > 0 || g.deleted > 0 {
                    let mut diff = Vec::new();
                    if g.added > 0 {
                        diff.push(format!("+{}", g.added));
                    }
                    if g.deleted > 0 {
                        diff.push(format!("-{}", g.deleted));
                    }
                    parts.push(diff.join(" "));
                } else if g.dirty {
                    parts.push("±".into());
                }
                if g.conflicts {
                    parts.push("⚠".into());
                }
                let mut sync = String::new();
                if g.ahead > 0 {
                    sync += &format!("↑{}", g.ahead);
                }
                if g.behind > 0 {
                    sync += &format!("↓{}", g.behind);
                }
                if !sync.is_empty() {
                    parts.push(sync);
                }
                if !parts.is_empty() {
                    if compact {
                        text += "*";
                    } else {
                        text += &format!(" [{}]", parts.join(" "));
                    }
                }
            }
            let mut spans = vec![plain(text)];
            if let Some(pr) = &ctx.pr {
                let badge = format!("[PR#{}]", pr.number);
                let badge = if seg.opt_bool("pr_link", true) {
                    hyperlink(&badge, &pr.url)
                } else {
                    badge
                };
                spans.push(plain(" "));
                spans.push(colored(badge, token("primary")));
            }
            Some(spans)
        }
        SegmentId::Context => {
            if p.max_context_tokens == 0 {
                return None;
            }
            let ratio = p.context_usage.clamp(0.0, 1.0);
            let pct = (ratio * 100.0).round() as u32;
            let tok = if ratio >= 0.85 {
                "error"
            } else if ratio >= 0.6 {
                "warning"
            } else {
                "success"
            };
            let label = if zh { "上下文 " } else { "ctx " };
            let text = if compact || !seg.opt_bool("show_tokens", true) {
                format!("{pct}%")
            } else {
                format!(
                    "{label}{pct}% ({}/{})",
                    fmt_tokens(p.context_tokens),
                    fmt_tokens(p.max_context_tokens)
                )
            };
            Some(vec![Span {
                text,
                color: seg.opt_bool("colorful", true).then(|| token(tok)),
                bold: false,
                dim: false,
            }])
        }
        SegmentId::Usage => {
            let st = ctx.stats.as_ref()?;
            if st.total.is_empty() {
                return None;
            }
            let mut spans = Vec::new();
            if !compact {
                spans.push(plain(if zh { "总计 " } else { "total " }));
            }
            spans.extend(usage_triple(ctx, seg, &st.total, compact));
            Some(spans)
        }
        SegmentId::Subagent => {
            let st = ctx.stats.as_ref()?;
            // heaviest sub-agent model by input; the rest stay in the totals
            let (model, u) = st
                .sub_by_model
                .iter()
                .filter(|(_, u)| !u.is_empty())
                .max_by_key(|(_, u)| u.input())?;
            let mut spans = vec![plain(format!("{} ", ctx.models.display(model)))];
            spans.extend(usage_triple(ctx, seg, u, compact));
            Some(spans)
        }
        SegmentId::Quota => quota_segment(ctx, seg, compact),
        SegmentId::Tps => tps_segment(ctx, seg, compact),
        SegmentId::Session => {
            let created = ctx.session_created?;
            let secs = (ctx.now - created).max(0.0) as u64;
            Some(vec![plain(fmt_duration(secs))])
        }
    }
}

fn usage_triple(ctx: &Ctx, seg: &SegmentConfig, u: &Usage, compact: bool) -> Vec<Span> {
    let colorful = seg.opt_bool("colorful", true);
    let pick = |c: AnsiColor| colorful.then_some(c);
    let mut spans = vec![
        Span {
            text: format!("↑ {}", fmt_tokens(u.input())),
            color: pick(AnsiColor::Color16 { c16: 4 }),
            bold: false,
            dim: false,
        },
        plain(if compact { "·" } else { " · " }),
        Span {
            text: format!("↓ {}", fmt_tokens(u.output)),
            color: pick(AnsiColor::Color16 { c16: 5 }),
            bold: false,
            dim: false,
        },
    ];
    if seg.opt_bool("show_cache", true) {
        if let Some(r) = u.cache_rate() {
            let label = match (compact, ctx.zh()) {
                (true, _) => " ",
                (false, true) => " 缓存 ",
                (false, false) => " cache ",
            };
            spans.push(Span {
                text: format!("{label}{}", fmt_rate(r)),
                color: pick(rgb(cache_color(r))),
                bold: false,
                dim: false,
            });
        }
    }
    spans
}

/// `5h 42% ↻1h20m · 7d 13% ↻Mon 08:00`: used share of each plan window
/// plus when it resets. Within a day the reset reads as a countdown, further
/// out as a local weekday + time.
fn quota_segment(ctx: &Ctx, seg: &SegmentConfig, compact: bool) -> Option<Vec<Span>> {
    let q = ctx.quota.as_ref()?;
    let zh = ctx.zh();
    let windows = [
        ("5h", q.limit_5h.as_ref(), seg.opt_bool("show_5h", true)),
        ("7d", q.limit_7d.as_ref(), seg.opt_bool("show_7d", true)),
        (
            if zh { "月" } else { "mo" },
            q.month.as_ref(),
            seg.opt_bool("show_month", false),
        ),
    ];
    let colorful = seg.opt_bool("colorful", true);
    // reset times survive compaction: they are what the segment is for
    let show_reset = seg.opt_bool("show_reset", true);
    let bar = seg.opt_bool("bar", false) && !compact;
    let mut spans = Vec::new();
    for (label, entry, wanted) in windows {
        let Some(e) = entry.filter(|_| wanted) else {
            continue;
        };
        if !spans.is_empty() {
            spans.push(plain(if compact { " " } else { " · " }));
        }
        let ratio = e.used_ratio.clamp(0.0, 1.0);
        // ceil like upstream usagePercent: any use shows at least 1%
        let pct = (ratio * 100.0).ceil() as u32;
        let tok = if ratio >= 0.85 {
            "error"
        } else if ratio >= 0.5 {
            "warning"
        } else {
            "success"
        };
        spans.push(plain(format!("{label} ")));
        if bar {
            let filled = (ratio * 8.0).round() as usize;
            spans.push(Span {
                text: format!("{}{} ", "█".repeat(filled), "░".repeat(8 - filled)),
                color: colorful.then(|| token(tok)),
                bold: false,
                dim: false,
            });
        }
        spans.push(Span {
            text: format!("{pct}%"),
            color: colorful.then(|| token(tok)),
            bold: false,
            dim: false,
        });
        if show_reset {
            if let Some(reset) = e
                .reset_at
                .as_deref()
                .and_then(|r| fmt_reset(r, ctx.now, zh, compact))
            {
                spans.push(plain(format!(" ↻{reset}")));
            }
        }
    }
    (!spans.is_empty()).then_some(spans)
}

/// `33.1 tok/s · ×3 96 tok/s (avg 31.4)`: the latest call's decode speed
/// (any agent); while several agents are streaming at once, their combined
/// throughput and count; and the session's token-weighted average. An idle
/// session keeps the last measurement on screen, dimmed once it is older
/// than `stale_secs` so it doesn't read as live; `hide_when_stale` drops it
/// instead.
fn tps_segment(ctx: &Ctx, seg: &SegmentConfig, compact: bool) -> Option<Vec<Span>> {
    let speed = &ctx.stats.as_ref()?.speed;
    let last = speed.last()?;
    let stale_after = seg.opt_int("stale_secs", 300);
    let stale = stale_after > 0 && ctx.now - last.end / 1000.0 > stale_after as f64;
    if stale && seg.opt_bool("hide_when_stale", false) {
        return None;
    }
    let fmt = |v: f64| {
        if v >= 100.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.1}")
        }
    };
    let unit = if compact { "t/s" } else { "tok/s" };
    let mut spans = vec![plain(format!("{} {unit}", fmt(last.rate())))];
    if seg.opt_bool("show_parallel", true) {
        let window = seg.opt_int("window_secs", 30).max(1) as f64;
        if let Some(tp) = speed.throughput(window).filter(|t| t.agents > 1) {
            spans.push(plain(if compact { " " } else { " · " }));
            spans.push(colored(
                format!("×{} {} {unit}", tp.agents, fmt(tp.tokens_per_sec)),
                token("accent"),
            ));
        }
    }
    if seg.opt_bool("show_avg", true) && !compact {
        if let Some(avg) = speed.average() {
            let label = if ctx.zh() { "均" } else { "avg" };
            spans.push(colored(
                format!(" ({label} {})", fmt(avg)),
                token("text_muted"),
            ));
        }
    }
    if stale {
        for span in &mut spans {
            span.color = Some(token("text_muted"));
            span.dim = true;
        }
    }
    Some(spans)
}

fn fmt_reset(rfc3339: &str, now: f64, zh: bool, compact: bool) -> Option<String> {
    use chrono::{DateTime, Local};
    let at = DateTime::parse_from_rfc3339(rfc3339).ok()?;
    let secs = at.timestamp() as f64 - now;
    if secs <= 0.0 {
        return Some(if zh { "即将" } else { "now" }.into());
    }
    if secs < 86_400.0 {
        let m = (secs / 60.0).ceil() as u64;
        return Some(if m >= 60 {
            format!("{}h{:02}m", m / 60, m % 60)
        } else {
            format!("{m}m")
        });
    }
    if compact {
        // "3d", "6d": the day count is enough when space is short
        return Some(format!("{}d", (secs / 86_400.0).round() as u64));
    }
    let local = at.with_timezone(&Local);
    Some(if zh {
        const DAYS: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
        use chrono::Datelike;
        format!(
            "{} {}",
            DAYS[local.weekday().num_days_from_monday() as usize],
            local.format("%H:%M")
        )
    } else {
        local.format("%a %H:%M").to_string()
    })
}

/// `[goal ● active · 4m · 7 turns]`, a port of upstream formatGoalBadge.
/// Only live goals get a badge; an active goal's clock keeps ticking from
/// when this snapshot was first seen.
fn goal_badge(ctx: &Ctx, compact: bool) -> Option<Vec<Span>> {
    let g = ctx.goal.as_ref()?;
    let status = g.get("status")?.as_str()?;
    let dot = match status {
        "active" => "primary",
        "blocked" => "warning",
        "paused" => "text_muted",
        _ => return None,
    };
    let num = |v: Option<&serde_json::Value>| v.and_then(|v| v.as_f64()).unwrap_or(0.0);
    let turns_used = num(g.get("turnsUsed")) as u64;
    let turns = match g
        .get("budget")
        .and_then(|b| b.get("turnBudget"))
        .and_then(|t| t.as_u64())
    {
        Some(b) => format!("{turns_used}/{b} turns"),
        None if turns_used == 1 => "1 turn".into(),
        None => format!("{turns_used} turns"),
    };
    let mut wall_ms = num(g.get("wallClockMs"));
    if status == "active" {
        if let Some(seen) = ctx.goal_seen_at {
            wall_ms += ((ctx.now - seen) * 1000.0).max(0.0);
        }
    }
    let elapsed = fmt_duration((wall_ms / 1000.0).round() as u64);
    let tail = if compact {
        format!(" {elapsed}]")
    } else {
        format!(" {status} · {elapsed} · {turns}]")
    };
    Some(vec![plain("[goal "), colored("●", token(dot)), plain(tail)])
}

// ---------------------------------------------------------------------------
// painting
// ---------------------------------------------------------------------------

struct Rendered<'a> {
    seg: &'a SegmentConfig,
    text: String,
}

fn icon_of<'a>(ctx: &Ctx, seg: &'a SegmentConfig) -> &'a str {
    match ctx.config.style.mode {
        StyleMode::Plain => &seg.icon.plain,
        StyleMode::NerdFont | StyleMode::Powerline => &seg.icon.nerd_font,
    }
}

/// Paint one segment: icon, then spans in their own color or the segment's
/// text color. A background (powerline) segment gets one-space padding and
/// leaves its background open; the joiner closes it.
fn paint_segment(ctx: &Ctx, seg: &SegmentConfig, spans: Vec<Span>) -> String {
    let bg = seg
        .colors
        .background
        .as_ref()
        .and_then(|c| ctx.sgr(c, true));
    let bold = seg.styles.text_bold;
    let mut out = String::new();
    if let Some(bg) = &bg {
        out += &format!("\x1b[{bg}m ");
    }
    let icon = icon_of(ctx, seg);
    if !icon.is_empty() {
        out += &paint(ctx, icon, seg.colors.icon.as_ref(), false);
        out.push(' ');
    }
    for span in spans {
        // on a background, per-span accents (badges, ramps) would fight it:
        // the segment's own text color, chosen for that background, wins
        let color = if bg.is_some() {
            seg.colors.text.as_ref().or(span.color.as_ref())
        } else {
            span.color.as_ref().or(seg.colors.text.as_ref())
        };
        out += &paint_styled(ctx, &span.text, color, bold || span.bold, span.dim);
    }
    if bg.is_some() {
        out.push(' ');
    }
    out
}

fn paint(ctx: &Ctx, text: &str, color: Option<&AnsiColor>, bold: bool) -> String {
    paint_styled(ctx, text, color, bold, false)
}

fn paint_styled(ctx: &Ctx, text: &str, color: Option<&AnsiColor>, bold: bool, dim: bool) -> String {
    if text.is_empty() || !ctx.color {
        return text.to_string();
    }
    let mut codes = Vec::new();
    if let Some(c) = color.and_then(|c| ctx.sgr(c, false)) {
        codes.push(c);
    }
    // bold and faint share SGR 22 as their reset, which CLOSE_FG emits
    if bold {
        codes.push("1".into());
    } else if dim {
        codes.push("2".into());
    }
    if codes.is_empty() {
        text.to_string()
    } else {
        format!("\x1b[{}m{text}{CLOSE_FG}", codes.join(";"))
    }
}

fn join(ctx: &Ctx, parts: &[Rendered]) -> String {
    let style = &ctx.config.style;
    let powerline = style.separator == POWERLINE_ARROW;
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let bg = part.seg.colors.background.as_ref();
        if i > 0 {
            let prev_bg = parts[i - 1].seg.colors.background.as_ref();
            if powerline && (prev_bg.is_some() || bg.is_some()) {
                out += &arrow(ctx, prev_bg, bg);
            } else {
                if prev_bg.is_some() && ctx.color {
                    out += CLOSE_BG;
                }
                let sep_color = style.separator_color.clone().unwrap_or(token("text_muted"));
                out += &paint(ctx, &style.separator, Some(&sep_color), false);
            }
        }
        out += &part.text;
    }
    if let Some(last) = parts.last() {
        if let Some(bg) = last.seg.colors.background.as_ref() {
            if powerline {
                out += &arrow(ctx, Some(bg), None);
            } else if ctx.color {
                out += CLOSE_BG;
            }
        }
    }
    out
}

/// Powerline transition: the arrow takes the previous background as its
/// foreground, drawn over the next segment's background.
fn arrow(ctx: &Ctx, prev: Option<&AnsiColor>, next: Option<&AnsiColor>) -> String {
    if !ctx.color {
        return POWERLINE_ARROW.into();
    }
    let mut out = String::new();
    match next.and_then(|c| ctx.sgr(c, true)) {
        Some(bg) => out += &format!("\x1b[{bg}m"),
        None => out += CLOSE_BG,
    }
    match prev.and_then(|c| ctx.sgr(c, false)) {
        Some(fg) => out += &format!("\x1b[{fg}m{POWERLINE_ARROW}{CLOSE_FG}"),
        None => out += POWERLINE_ARROW,
    }
    out
}

fn compose(ctx: &Ctx, compact: bool, dropped: &[SegmentId]) -> String {
    let parts: Vec<Rendered> = ctx
        .config
        .segments
        .iter()
        .filter(|s| s.enabled && !dropped.contains(&s.id))
        .filter_map(|seg| {
            let spans = segment(ctx, seg, compact)?;
            Some(Rendered {
                seg,
                text: paint_segment(ctx, seg, spans),
            })
        })
        .collect();
    join(ctx, &parts)
}

/// Render, then degrade until the line fits `width`: compact labels first,
/// then drop segments from least to most useful, and only as a last resort
/// cut with an ellipsis. `None` width returns the full line.
pub fn render(ctx: &Ctx, width: Option<usize>) -> String {
    let full = compose(ctx, false, &[]);
    let Some(width) = width else { return full };
    if visible_width(&full) <= width {
        return full;
    }
    use SegmentId::*;
    const DROP_ORDER: [SegmentId; 10] = [
        Session, Tps, Git, Directory, Subagent, Tasks, Goal, Context, Quota, Mode,
    ];
    let mut dropped = Vec::new();
    let mut line = compose(ctx, true, &dropped);
    for id in DROP_ORDER {
        if visible_width(&line) <= width {
            return line;
        }
        dropped.push(id);
        line = compose(ctx, true, &dropped);
    }
    if visible_width(&line) <= width {
        return line;
    }
    truncate(&line, width, ctx.color)
}

fn ansi_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.first() != Some(&0x1b) {
        return None;
    }
    match b.get(1) {
        // CSI: ESC [ params final-byte
        Some(b'[') => b[2..]
            .iter()
            .position(|c| (0x40..=0x7e).contains(c))
            .map(|i| i + 3),
        // OSC: ESC ] ... BEL | ESC \
        Some(b']') => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return Some(i + 1);
                }
                if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                    return Some(i + 2);
                }
                i += 1;
            }
            Some(b.len())
        }
        _ => Some(1),
    }
}

/// Terminal columns of a rendered line: escapes are zero-width, East Asian
/// wide characters count 2.
pub fn visible_width(s: &str) -> usize {
    let mut w = 0;
    let mut i = 0;
    while i < s.len() {
        if let Some(n) = ansi_len(&s[i..]) {
            i += n;
            continue;
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        w += ch.width().unwrap_or(0);
        i += ch.len_utf8();
    }
    w
}

fn truncate(s: &str, width: usize, color: bool) -> String {
    let mut out = String::new();
    let mut used = 0;
    let mut i = 0;
    while i < s.len() {
        if let Some(n) = ansi_len(&s[i..]) {
            out.push_str(&s[i..i + n]);
            i += n;
            continue;
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
        i += ch.len_utf8();
    }
    out.push('…');
    if color {
        out.push_str(CLOSE_FG);
        out.push_str(CLOSE_BG);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tps_ctx(age_secs: f64, hide_when_stale: bool) -> (Ctx, SegmentConfig) {
        let mut config = crate::themes::get("kimi");
        let seg = config
            .segments
            .iter_mut()
            .find(|s| s.id == SegmentId::Tps)
            .unwrap();
        seg.enabled = true;
        seg.options
            .insert("hide_when_stale".into(), hide_when_stale.into());
        let seg = seg.clone();
        let now = 1_000_000.0;
        let mut stats = SessionStats::default();
        stats.speed.recent.push(crate::session::Call {
            agent: "main".into(),
            start: (now - age_secs - 10.0) * 1000.0,
            end: (now - age_secs) * 1000.0,
            output: 420,
            ttft: 0.0,
        });
        let ctx = Ctx {
            payload: Payload::default(),
            config,
            palette: crate::kimi_config::DARK,
            models: Models::default(),
            stats: Some(stats),
            effort: None,
            goal: None,
            goal_seen_at: None,
            session_created: None,
            tasks: (0, 0),
            git: None,
            pr: None,
            dance: None,
            quota: None,
            now,
            color: true,
        };
        (ctx, seg)
    }

    #[test]
    fn idle_tps_stays_visible_but_dimmed() {
        let muted = Some(token("text_muted"));
        // fresh: the rate is not dimmed
        let (ctx, seg) = tps_ctx(10.0, false);
        let spans = tps_segment(&ctx, &seg, false).unwrap();
        assert_eq!(spans[0].text, "42.0 tok/s");
        assert_ne!(spans[0].color, muted);
        // idle past stale_secs (300): still shown, every span dimmed
        let (ctx, seg) = tps_ctx(3600.0, false);
        let spans = tps_segment(&ctx, &seg, false).unwrap();
        assert_eq!(spans[0].text, "42.0 tok/s");
        assert!(spans.iter().all(|s| s.color == muted && s.dim));
        assert!(!tps_segment(&tps_ctx(10.0, false).0, &seg, false).unwrap()[0].dim);
        // hide_when_stale restores the old behavior
        let (ctx, seg) = tps_ctx(3600.0, true);
        assert!(tps_segment(&ctx, &seg, false).is_none());
    }

    #[test]
    fn tokens_and_rates() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(12_345), "12.3k");
        assert_eq!(fmt_tokens(1_234_567), "1.23M");
        assert_eq!(fmt_rate(97.84), "97.8%");
        assert_eq!(fmt_rate(99.99), "99.9%");
        assert_eq!(fmt_rate(100.0), "100%");
        assert_eq!(fmt_rate(42.4), "42%");
    }

    #[test]
    fn width_ignores_escapes_and_counts_cjk() {
        assert_eq!(visible_width("\x1b[38;2;1;2;3mab\x1b[22m\x1b[39m"), 2);
        assert_eq!(visible_width("缓存"), 4);
        assert_eq!(visible_width("\x1b]8;;https://x\x07PR\x1b]8;;\x07"), 2);
        assert_eq!(
            visible_width(&truncate("\x1b[34mabcdef\x1b[39m", 4, false)),
            4
        );
    }

    #[test]
    fn cwd_shortening() {
        assert_eq!(shorten_cwd("/a/b/c/d/e", 3), "…/c/d/e");
        assert_eq!(shorten_cwd("/a/b", 3), "/a/b");
    }

    #[test]
    fn cache_ramp_endpoints() {
        let Rgb(r, g, _) = cache_color(0.0);
        assert!(r > g);
        let Rgb(r, g, _) = cache_color(100.0);
        assert!(g > r);
    }

    #[test]
    fn reset_formatting() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-11T16:00:00Z")
            .unwrap()
            .timestamp() as f64;
        assert_eq!(
            fmt_reset("2026-09-11T18:00:00Z", now, false, false).unwrap(),
            "2h00m"
        );
        assert_eq!(
            fmt_reset("2026-09-11T16:30:00Z", now, false, false).unwrap(),
            "30m"
        );
        assert_eq!(
            fmt_reset("2026-09-11T15:00:00Z", now, false, false).unwrap(),
            "now"
        );
        assert!(fmt_reset("2026-09-15T00:00:00Z", now, false, false)
            .unwrap()
            .contains(':'));
        assert_eq!(
            fmt_reset("2026-09-15T00:00:00Z", now, false, true).unwrap(),
            "3d"
        );
    }

    #[test]
    fn rainbow_skips_spaces() {
        let spans = rainbow("a b", 0, false);
        assert_eq!(spans.len(), 3);
        assert!(spans[1].color.is_none());
        assert_eq!(spans[2].color, Some(rgb(DARK_RAINBOW[1])));
    }
}
