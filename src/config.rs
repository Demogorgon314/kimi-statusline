//! Configuration, modeled on CCometixLine: a style section plus an ordered
//! list of segments, each with its own icon, colors, text style and options.
//!
//! Files live under `<KIMI_CODE_HOME>/kimi-statusline/`:
//! `config.toml` (the active config) and `themes/<name>.toml` (saved themes,
//! same format). A missing or broken config falls back to the `kimi` theme.

use crate::kimi_config::{Palette, Rgb};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub style: StyleConfig,
    #[serde(default)]
    pub segments: Vec<SegmentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StyleConfig {
    pub mode: StyleMode,
    /// Between segments; `"\u{e0b0}"` turns on powerline arrows.
    pub separator: String,
    /// Color of the separator (powerline arrows take the segment colors).
    pub separator_color: Option<AnsiColor>,
    pub lang: Lang,
    /// Palette for token colors: "" follows tui.toml's `theme`, or
    /// "dark" / "light" / a Kimi Code custom theme name.
    pub palette: String,
    /// Fixed render width; 0 detects the terminal.
    pub width: usize,
}

impl Default for StyleConfig {
    fn default() -> Self {
        StyleConfig {
            mode: StyleMode::Plain,
            separator: "  ".into(),
            separator_color: None,
            lang: Lang::En,
            palette: String::new(),
            width: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleMode {
    Plain,
    NerdFont,
    Powerline,
}

impl StyleMode {
    pub const ALL: [StyleMode; 3] = [StyleMode::Plain, StyleMode::NerdFont, StyleMode::Powerline];

    pub fn name(self) -> &'static str {
        match self {
            StyleMode::Plain => "plain",
            StyleMode::NerdFont => "nerd_font",
            StyleMode::Powerline => "powerline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    En,
    Zh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentId {
    /// permission / plan / swarm / tower badges
    Mode,
    /// live goal badge from state.json
    Goal,
    /// model display name + thinking effort (+ /dance rainbow)
    Model,
    /// background task / agent counts
    Tasks,
    Directory,
    /// branch, diff stats, ahead/behind, PR badge
    Git,
    /// context window fill
    Context,
    /// whole-session input / output / cache hit rate
    Usage,
    /// heaviest sub-agent model's usage
    Subagent,
    /// session wall-clock age
    Session,
    /// plan quota: 5h / 7d limits and reset times
    Quota,
    /// output tokens per second
    Tps,
}

impl SegmentId {
    pub const ALL: [SegmentId; 12] = [
        SegmentId::Mode,
        SegmentId::Goal,
        SegmentId::Model,
        SegmentId::Tasks,
        SegmentId::Directory,
        SegmentId::Git,
        SegmentId::Context,
        SegmentId::Usage,
        SegmentId::Subagent,
        SegmentId::Session,
        SegmentId::Quota,
        SegmentId::Tps,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SegmentId::Mode => "Mode",
            SegmentId::Goal => "Goal",
            SegmentId::Model => "Model",
            SegmentId::Tasks => "Tasks",
            SegmentId::Directory => "Directory",
            SegmentId::Git => "Git",
            SegmentId::Context => "Context",
            SegmentId::Usage => "Usage",
            SegmentId::Subagent => "Subagent",
            SegmentId::Session => "Session",
            SegmentId::Quota => "Quota",
            SegmentId::Tps => "TPS",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentConfig {
    pub id: SegmentId,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub icon: IconConfig,
    #[serde(default)]
    pub colors: ColorConfig,
    #[serde(default)]
    pub styles: TextStyleConfig,
    #[serde(default)]
    pub options: BTreeMap<String, toml::Value>,
}

fn yes() -> bool {
    true
}

impl SegmentConfig {
    pub fn opt_bool(&self, key: &str, default: bool) -> bool {
        self.options
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    pub fn opt_int(&self, key: &str, default: i64) -> i64 {
        self.options
            .get(key)
            .and_then(|v| v.as_integer())
            .unwrap_or(default)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct IconConfig {
    pub plain: String,
    pub nerd_font: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ColorConfig {
    pub icon: Option<AnsiColor>,
    pub text: Option<AnsiColor>,
    pub background: Option<AnsiColor>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TextStyleConfig {
    pub text_bold: bool,
}

/// A color in any of CCometixLine's spellings (`{ c16 = 14 }`,
/// `{ c256 = 208 }`, `{ r = 1, g = 2, b = 3 }`), a `"#rrggbb"` string, or the
/// name of a Kimi Code palette token (`"primary"`, `"text_dim"`, …) that
/// follows the TUI theme.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AnsiColor {
    Color16 { c16: u8 },
    Color256 { c256: u8 },
    Rgb { r: u8, g: u8, b: u8 },
    Named(String),
}

pub const TOKENS: [&str; 8] = [
    "text",
    "primary",
    "accent",
    "text_dim",
    "text_muted",
    "success",
    "warning",
    "error",
];

impl AnsiColor {
    /// SGR parameters for this color as foreground (`bg = false`) or
    /// background. Unknown names yield None.
    pub fn sgr(&self, palette: &Palette, bg: bool) -> Option<String> {
        let rgb = |Rgb(r, g, b): Rgb| format!("{};2;{r};{g};{b}", if bg { 48 } else { 38 });
        Some(match self {
            AnsiColor::Color16 { c16 } => {
                let base = if bg { 40 } else { 30 };
                let c = *c16 as u32;
                if c < 8 {
                    (base + c).to_string()
                } else {
                    (base + 60 + (c - 8).min(7)).to_string()
                }
            }
            AnsiColor::Color256 { c256 } => format!("{};5;{c256}", if bg { 48 } else { 38 }),
            AnsiColor::Rgb { r, g, b } => rgb(Rgb(*r, *g, *b)),
            AnsiColor::Named(name) => rgb(Rgb::parse(name).or_else(|| palette.token(name))?),
        })
    }

    /// Approximate RGB, for the configurator's preview swatches.
    pub fn to_rgb(&self, palette: &Palette) -> Option<Rgb> {
        Some(match self {
            AnsiColor::Color16 { c16 } => XTERM16[(*c16 as usize).min(15)],
            AnsiColor::Color256 { c256 } => xterm256(*c256),
            AnsiColor::Rgb { r, g, b } => Rgb(*r, *g, *b),
            AnsiColor::Named(name) => Rgb::parse(name).or_else(|| palette.token(name))?,
        })
    }

    pub fn describe(&self) -> String {
        match self {
            AnsiColor::Color16 { c16 } => format!("16-color {c16}"),
            AnsiColor::Color256 { c256 } => format!("256-color {c256}"),
            AnsiColor::Rgb { r, g, b } => format!("#{r:02X}{g:02X}{b:02X}"),
            AnsiColor::Named(n) => n.clone(),
        }
    }
}

const XTERM16: [Rgb; 16] = [
    Rgb(0, 0, 0),
    Rgb(205, 49, 49),
    Rgb(13, 188, 121),
    Rgb(229, 229, 16),
    Rgb(36, 114, 200),
    Rgb(188, 63, 188),
    Rgb(17, 168, 205),
    Rgb(229, 229, 229),
    Rgb(102, 102, 102),
    Rgb(241, 76, 76),
    Rgb(35, 209, 139),
    Rgb(245, 245, 67),
    Rgb(59, 142, 234),
    Rgb(214, 112, 214),
    Rgb(41, 184, 219),
    Rgb(255, 255, 255),
];

fn xterm256(c: u8) -> Rgb {
    match c {
        0..=15 => XTERM16[c as usize],
        16..=231 => {
            let i = c - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb(step(i / 36), step(i / 6 % 6), step(i % 6))
        }
        _ => {
            let v = 8 + (c - 232) * 10;
            Rgb(v, v, v)
        }
    }
}

// ---------------------------------------------------------------------------
// files
// ---------------------------------------------------------------------------

pub fn config_dir() -> PathBuf {
    paths::kimi_home().join("kimi-statusline")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

impl Config {
    pub fn load() -> Config {
        Config::try_load().unwrap_or_else(|e| {
            if !e.is_empty() {
                paths::debug(&format!("config: {e}"));
            }
            crate::themes::get("kimi")
        })
    }

    /// Err("") when there is simply no config file.
    pub fn try_load() -> Result<Config, String> {
        let text = match std::fs::read_to_string(config_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(String::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut cfg: Config = toml::from_str(&text).map_err(|e| e.to_string())?;
        cfg.add_missing_segments();
        Ok(cfg)
    }

    /// Segments added in a newer version than the one that wrote this
    /// config: append them, disabled, styled like the same theme's built-in
    /// preset, so they show up in the configurator without changing what
    /// the status line already renders.
    pub fn add_missing_segments(&mut self) {
        let missing: Vec<SegmentId> = SegmentId::ALL
            .into_iter()
            .filter(|id| self.segment(*id).is_none())
            .collect();
        if missing.is_empty() {
            return;
        }
        let preset = crate::themes::builtin(&self.theme)
            .or_else(|| crate::themes::builtin("kimi"))
            .expect("kimi theme");
        for id in missing {
            if let Some(mut seg) = preset.segment(id).cloned() {
                seg.enabled = false;
                self.segments.push(seg);
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.write_to(&config_path())
    }

    pub fn write_to(&self, path: &std::path::Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        paths::write_atomic(path, text.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn segment(&self, id: SegmentId) -> Option<&SegmentConfig> {
        self.segments.iter().find(|s| s.id == id)
    }
}
