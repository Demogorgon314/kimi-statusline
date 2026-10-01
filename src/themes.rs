//! Built-in theme presets (CCometixLine's set plus a `kimi` theme that
//! mirrors Kimi Code's own footer), and saved themes under
//! `<KIMI_CODE_HOME>/kimi-statusline/themes/<name>.toml`.

use crate::config::{
    themes_dir, AnsiColor, ColorConfig, Config, IconConfig, Lang, SegmentConfig, SegmentId,
    StyleConfig, StyleMode, TextStyleConfig,
};
use std::collections::BTreeMap;

pub const BUILTIN: [(&str, &str); 10] = [
    (
        "kimi",
        "Matches Kimi Code's built-in footer (follows the TUI theme)",
    ),
    ("cometix", "Cometix: bold 16-color with Nerd Font icons"),
    ("default", "Default: 16-color with emoji icons"),
    ("minimal", "Minimal: symbols instead of emoji"),
    ("gruvbox", "Gruvbox colors"),
    ("nord", "Nord colors on backgrounds"),
    ("powerline-dark", "Dark powerline"),
    ("powerline-light", "Light powerline"),
    ("powerline-rose-pine", "Rosé Pine powerline"),
    ("powerline-tokyo-night", "Tokyo Night powerline"),
];

/// (plain, nerd font) icons per segment, CCometixLine's where they overlap.
fn icons(id: SegmentId, minimal: bool) -> (&'static str, &'static str) {
    use SegmentId::*;
    match (id, minimal) {
        (Mode, false) => ("🛡️", "\u{f0483}"),
        (Mode, true) => ("◆", "\u{f0483}"),
        (Goal, false) => ("🎯", "\u{f0136}"),
        (Goal, true) => ("◎", "\u{f0136}"),
        (Model, false) => ("🤖", "\u{e26d}"),
        (Model, true) => ("✽", "\u{f2d0}"),
        (Tasks, false) => ("⚙️", "\u{f0493}"),
        (Tasks, true) => ("⚙", "\u{f0493}"),
        (Directory, false) => ("📁", "\u{f024b}"),
        (Directory, true) => ("◐", "\u{f024b}"),
        (Git, false) => ("🌿", "\u{f02a2}"),
        (Git, true) => ("※", "\u{f02a2}"),
        (Context, false) => ("⚡️", "\u{f49b}"),
        (Context, true) => ("◑", "\u{f49b}"),
        (Usage, false) => ("📊", "\u{f0a9e}"),
        (Usage, true) => ("Σ", "\u{f0a9e}"),
        (Subagent, false) => ("🧩", "\u{f0bc5}"),
        (Subagent, true) => ("⊕", "\u{f0bc5}"),
        (Session, false) => ("⏱️", "\u{f19bb}"),
        (Session, true) => ("◷", "\u{f19bb}"),
        (Quota, false) => ("⏳", "\u{f0a9e}"),
        (Quota, true) => ("◔", "\u{f0a9e}"),
        (Tps, false) => ("🚀", "\u{f04c5}"),
        (Tps, true) => ("≫", "\u{f04c5}"),
    }
}

fn default_options(id: SegmentId) -> BTreeMap<String, toml::Value> {
    let mut o = BTreeMap::new();
    let mut put = |k: &str, v: toml::Value| {
        o.insert(k.to_string(), v);
    };
    match id {
        SegmentId::Model => put("dance", true.into()),
        SegmentId::Directory => put("depth", 3.into()),
        SegmentId::Git => {
            put("status", true.into());
            put("pr", true.into());
            put("pr_link", true.into());
        }
        SegmentId::Context => {
            put("show_tokens", true.into());
            put("colorful", true.into());
        }
        SegmentId::Usage => {
            put("colorful", true.into());
            put("show_cache", true.into());
        }
        SegmentId::Subagent => put("colorful", true.into()),
        SegmentId::Quota => {
            put("show_5h", true.into());
            put("show_7d", true.into());
            put("show_month", false.into());
            put("show_reset", true.into());
            put("bar", false.into());
            put("colorful", true.into());
            put("refresh_secs", 120.into());
        }
        SegmentId::Tps => {
            put("show_avg", true.into());
            put("show_parallel", true.into());
            put("window_secs", 30.into());
            put("stale_secs", 300.into());
        }
        _ => {}
    }
    o
}

struct Spec {
    id: SegmentId,
    enabled: bool,
    icon: Option<AnsiColor>,
    text: Option<AnsiColor>,
    bg: Option<AnsiColor>,
}

fn c16(c: u8) -> Option<AnsiColor> {
    Some(AnsiColor::Color16 { c16: c })
}
fn c256(c: u8) -> Option<AnsiColor> {
    Some(AnsiColor::Color256 { c256: c })
}
fn rgb(r: u8, g: u8, b: u8) -> Option<AnsiColor> {
    Some(AnsiColor::Rgb { r, g, b })
}
fn tok(name: &str) -> Option<AnsiColor> {
    Some(AnsiColor::Named(name.into()))
}

/// Default enablement, shared by every preset.
fn enabled(id: SegmentId, kimi: bool) -> bool {
    match id {
        // the built-in footer's second line already shows context fill
        SegmentId::Context => !kimi,
        SegmentId::Session => false,
        _ => true,
    }
}

fn build(
    name: &str,
    mode: StyleMode,
    separator: &str,
    bold: bool,
    minimal_icons: bool,
    specs: Vec<Spec>,
) -> Config {
    let segments = specs
        .into_iter()
        .map(|s| {
            let (plain, nerd) = icons(s.id, minimal_icons);
            let mut options = default_options(s.id);
            if s.bg.is_some() {
                // fixed ↑/↓ and context colors fight a segment background;
                // let the segment's own text color carry them
                if options.contains_key("colorful") {
                    options.insert("colorful".into(), false.into());
                }
                if s.id == SegmentId::Context {
                    options.insert("colorful".into(), false.into());
                }
            }
            SegmentConfig {
                id: s.id,
                enabled: s.enabled,
                icon: IconConfig {
                    plain: plain.into(),
                    nerd_font: nerd.into(),
                },
                colors: ColorConfig {
                    icon: s.icon,
                    text: s.text,
                    background: s.bg,
                },
                styles: TextStyleConfig { text_bold: bold },
                options,
            }
        })
        .collect();
    Config {
        theme: name.into(),
        style: StyleConfig {
            mode,
            separator: separator.into(),
            separator_color: None,
            lang: Lang::En,
            palette: String::new(),
            width: 0,
        },
        segments,
    }
}

/// fg-only preset: one (icon, text) pair per segment in SegmentId::ALL order.
fn fg_theme(
    name: &str,
    mode: StyleMode,
    separator: &str,
    bold: bool,
    minimal: bool,
    colors: [(Option<AnsiColor>, Option<AnsiColor>); 12],
) -> Config {
    let specs = SegmentId::ALL
        .into_iter()
        .zip(colors)
        .map(|(id, (icon, text))| Spec {
            id,
            enabled: enabled(id, false),
            icon,
            text,
            bg: None,
        })
        .collect();
    build(name, mode, separator, bold, minimal, specs)
}

type Triple = (u8, u8, u8);

/// powerline preset: one (fg, bg) pair per segment.
fn pl_theme(name: &str, colors: [(Triple, Triple); 12]) -> Config {
    let specs = SegmentId::ALL
        .into_iter()
        .zip(colors)
        .map(|(id, ((fr, fg, fb), (br, bg, bb)))| Spec {
            id,
            enabled: enabled(id, false),
            icon: rgb(fr, fg, fb),
            text: rgb(fr, fg, fb),
            bg: rgb(br, bg, bb),
        })
        .collect();
    build(name, StyleMode::Powerline, "\u{e0b0}", false, false, specs)
}

fn kimi() -> Config {
    // no icons, palette-token colors: looks like the built-in footer, and
    // follows the TUI's dark/light/custom theme
    let mut cfg = build(
        "kimi",
        StyleMode::Plain,
        "  ",
        false,
        false,
        SegmentId::ALL
            .into_iter()
            .map(|id| Spec {
                id,
                enabled: enabled(id, true),
                icon: tok("text_muted"),
                text: match id {
                    SegmentId::Tasks => tok("primary"),
                    SegmentId::Directory | SegmentId::Git => tok("text_dim"),
                    SegmentId::Session => tok("text_muted"),
                    _ => None,
                },
                bg: None,
            })
            .collect(),
    );
    for s in &mut cfg.segments {
        s.icon.plain = match s.id {
            // a divider in front of the usage half of the line
            SegmentId::Usage => "│".into(),
            _ => String::new(),
        };
    }
    cfg
}

pub fn builtin(name: &str) -> Option<Config> {
    // order: mode goal model tasks directory git context usage subagent session
    Some(match name {
        "kimi" => kimi(),
        "cometix" | "default" => {
            let pair = |c| (c16(c), c16(c));
            let mut cfg = fg_theme(
                name,
                if name == "cometix" {
                    StyleMode::NerdFont
                } else {
                    StyleMode::Plain
                },
                " | ",
                name == "cometix",
                false,
                [
                    pair(11),
                    pair(5),
                    pair(14),
                    pair(12),
                    (c16(11), c16(10)),
                    pair(12),
                    pair(13),
                    pair(14),
                    pair(6),
                    pair(2),
                    pair(3),
                    pair(10),
                ],
            );
            if name == "default" {
                cfg.style.mode = StyleMode::Plain;
            }
            cfg
        }
        "minimal" => {
            let pair = |c| (c16(c), c16(c));
            fg_theme(
                name,
                StyleMode::Plain,
                " │ ",
                false,
                true,
                [
                    pair(11),
                    pair(5),
                    pair(14),
                    pair(12),
                    (c16(11), c16(10)),
                    pair(12),
                    pair(13),
                    pair(14),
                    pair(6),
                    pair(2),
                    pair(3),
                    pair(10),
                ],
            )
        }
        "gruvbox" => {
            let pair = |c| (c256(c), c256(c));
            fg_theme(
                name,
                StyleMode::NerdFont,
                " | ",
                true,
                false,
                [
                    pair(167),
                    pair(175),
                    pair(208),
                    pair(214),
                    pair(142),
                    pair(109),
                    (c16(5), c16(5)),
                    pair(214),
                    pair(108),
                    pair(142),
                    pair(214),
                    pair(108),
                ],
            )
        }
        "nord" => {
            let fg = (46, 52, 64);
            let specs = SegmentId::ALL
                .into_iter()
                .zip([
                    (191, 97, 106),
                    (208, 135, 112),
                    (136, 192, 208),
                    (94, 129, 172),
                    (163, 190, 140),
                    (129, 161, 193),
                    (180, 142, 173),
                    (235, 203, 139),
                    (143, 188, 187),
                    (163, 190, 140),
                    (235, 203, 139),
                    (136, 192, 208),
                ])
                .map(|(id, (r, g, b))| Spec {
                    id,
                    enabled: enabled(id, false),
                    icon: rgb(fg.0, fg.1, fg.2),
                    text: rgb(fg.0, fg.1, fg.2),
                    bg: rgb(r, g, b),
                })
                .collect();
            build(name, StyleMode::Powerline, "\u{e0b0}", false, false, specs)
        }
        "powerline-dark" => pl_theme(
            name,
            [
                ((255, 255, 255), (120, 40, 40)),
                ((255, 255, 255), (90, 60, 110)),
                ((255, 255, 255), (45, 45, 45)),
                ((255, 255, 255), (30, 80, 120)),
                ((255, 255, 255), (139, 69, 19)),
                ((255, 255, 255), (64, 64, 64)),
                ((209, 213, 219), (55, 65, 81)),
                ((209, 213, 219), (45, 50, 59)),
                ((229, 192, 123), (40, 44, 52)),
                ((163, 190, 140), (45, 50, 59)),
                ((224, 175, 104), (40, 44, 52)),
                ((136, 192, 208), (40, 50, 60)),
            ],
        ),
        "powerline-light" => pl_theme(
            name,
            [
                ((255, 255, 255), (220, 53, 69)),
                ((255, 255, 255), (111, 66, 193)),
                ((0, 0, 0), (135, 206, 235)),
                ((255, 255, 255), (32, 201, 151)),
                ((255, 255, 255), (255, 107, 71)),
                ((255, 255, 255), (79, 179, 217)),
                ((255, 255, 255), (107, 114, 128)),
                ((255, 255, 255), (40, 167, 69)),
                ((0, 0, 0), (255, 193, 7)),
                ((255, 255, 255), (40, 167, 69)),
                ((0, 0, 0), (255, 193, 7)),
                ((255, 255, 255), (0, 123, 255)),
            ],
        ),
        "powerline-rose-pine" => pl_theme(
            name,
            [
                ((235, 111, 146), (35, 33, 54)),
                ((246, 193, 119), (31, 29, 46)),
                ((235, 188, 186), (25, 23, 36)),
                ((49, 116, 143), (38, 35, 58)),
                ((196, 167, 231), (38, 35, 58)),
                ((156, 207, 216), (31, 29, 46)),
                ((224, 222, 244), (82, 79, 103)),
                ((246, 193, 119), (35, 33, 54)),
                ((235, 188, 186), (42, 39, 63)),
                ((156, 207, 216), (42, 39, 63)),
                ((246, 193, 119), (35, 33, 54)),
                ((156, 207, 216), (25, 23, 36)),
            ],
        ),
        "powerline-tokyo-night" => pl_theme(
            name,
            [
                ((247, 118, 142), (36, 40, 59)),
                ((255, 158, 100), (41, 46, 66)),
                ((252, 167, 234), (25, 27, 41)),
                ((125, 207, 255), (32, 35, 52)),
                ((130, 170, 255), (47, 51, 77)),
                ((195, 232, 141), (30, 32, 48)),
                ((192, 202, 245), (61, 89, 161)),
                ((224, 175, 104), (36, 40, 59)),
                ((187, 154, 247), (32, 35, 52)),
                ((158, 206, 106), (41, 46, 66)),
                ((224, 175, 104), (36, 40, 59)),
                ((125, 207, 255), (25, 27, 41)),
            ],
        ),
        _ => return None,
    })
}

/// A saved theme file first (so users can override presets), then built-ins,
/// then `kimi`.
pub fn get(name: &str) -> Config {
    load_file(name)
        .or_else(|| builtin(name))
        .unwrap_or_else(|| builtin("kimi").expect("kimi theme"))
}

fn load_file(name: &str) -> Option<Config> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let text = std::fs::read_to_string(themes_dir().join(format!("{name}.toml"))).ok()?;
    let mut cfg: Config = toml::from_str(&text).ok()?;
    cfg.theme = name.into();
    cfg.add_missing_segments();
    Some(cfg)
}

pub fn save(name: &str, cfg: &Config) -> Result<std::path::PathBuf, String> {
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return Err(format!("invalid theme name {name:?}"));
    }
    let mut cfg = cfg.clone();
    cfg.theme = name.into();
    let path = themes_dir().join(format!("{name}.toml"));
    cfg.write_to(&path)?;
    Ok(path)
}

/// Built-ins followed by saved themes.
pub fn list() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(themes_dir()) {
        let mut extra: Vec<String> = rd
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".toml")
                    .map(str::to_string)
            })
            .filter(|n| !names.contains(n))
            .collect();
        extra.sort();
        names.extend(extra);
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_configs_gain_new_segments_disabled() {
        let mut cfg = builtin("nord").unwrap();
        cfg.segments.retain(|s| s.id != SegmentId::Tps);
        let enabled_before: Vec<_> = cfg
            .segments
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.id)
            .collect();
        cfg.add_missing_segments();
        let tps = cfg.segment(SegmentId::Tps).unwrap();
        assert!(!tps.enabled);
        // styled like the nord preset (powerline background)
        assert!(tps.colors.background.is_some());
        assert_eq!(cfg.segments.last().unwrap().id, SegmentId::Tps);
        let enabled_after: Vec<_> = cfg
            .segments
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.id)
            .collect();
        assert_eq!(enabled_before, enabled_after);
    }

    #[test]
    fn every_builtin_round_trips_through_toml() {
        for (name, _) in BUILTIN {
            let cfg = builtin(name).unwrap();
            assert_eq!(cfg.segments.len(), SegmentId::ALL.len(), "{name}");
            let text = toml::to_string_pretty(&cfg).unwrap();
            let back: Config = toml::from_str(&text).unwrap();
            assert_eq!(back, cfg, "{name}");
        }
    }
}
