//! Read-only views of Kimi Code's own config: model display names and
//! default efforts from `config.toml`, and the footer palette from `tui.toml`.

use crate::paths;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Models {
    /// alias / provider model / display name -> display name
    pub names: HashMap<String, String>,
    /// same keys -> default_effort
    pub efforts: HashMap<String, String>,
    /// same keys -> whether the model declares support_efforts
    pub has_efforts: HashMap<String, bool>,
}

impl Models {
    pub fn load() -> Self {
        let mut m = Models::default();
        let Ok(text) = std::fs::read_to_string(paths::kimi_home().join("config.toml")) else {
            return m;
        };
        let Ok(doc) = text.parse::<toml::Table>() else {
            paths::debug("config.toml did not parse");
            return m;
        };
        let Some(models) = doc.get("models").and_then(|v| v.as_table()) else {
            return m;
        };
        for (alias, entry) in models {
            let Some(entry) = entry.as_table() else {
                continue;
            };
            // [models."x".overrides] wins over the base section
            let overrides = entry.get("overrides").and_then(|v| v.as_table());
            let get = |k: &str| overrides.and_then(|o| o.get(k)).or_else(|| entry.get(k));
            let s = |k: &str| get(k).and_then(|v| v.as_str()).map(str::to_string);
            let display = s("display_name");
            let provider_model = s("model");
            let effort = s("default_effort");
            let supports = get("support_efforts")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty());
            let keys: Vec<String> = [Some(alias.clone()), provider_model, display.clone()]
                .into_iter()
                .flatten()
                .collect();
            for k in keys {
                if let Some(d) = &display {
                    m.names.entry(k.clone()).or_insert_with(|| d.clone());
                }
                if let Some(e) = &effort {
                    m.efforts.entry(k.clone()).or_insert_with(|| e.clone());
                }
                m.has_efforts.entry(k).or_insert(supports);
            }
        }
        m
    }

    /// Display name for any spelling, falling back to the last path segment
    /// (`kimi-code/k3-256k` -> `k3-256k`).
    pub fn display(&self, model: &str) -> String {
        self.names
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.rsplit('/').next().unwrap_or(model).to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(hex: &str) -> Option<Rgb> {
        let h = hex.strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Rgb(p(0)?, p(2)?, p(4)?))
    }
}

/// The ColorPalette tokens the footer uses (upstream src/tui/theme/colors.ts).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub text: Rgb,
    pub primary: Rgb,
    pub accent: Rgb,
    pub text_dim: Rgb,
    pub text_muted: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub error: Rgb,
}

pub const DARK: Palette = Palette {
    text: Rgb(0xE0, 0xE0, 0xE0),
    primary: Rgb(0x4F, 0xA8, 0xFF),
    accent: Rgb(0x5B, 0xC0, 0xBE),
    text_dim: Rgb(0x88, 0x88, 0x88),
    text_muted: Rgb(0x6B, 0x6B, 0x6B),
    success: Rgb(0x4E, 0xC8, 0x7E),
    warning: Rgb(0xE8, 0xA8, 0x38),
    error: Rgb(0xE8, 0x54, 0x54),
};

pub const LIGHT: Palette = Palette {
    text: Rgb(0x1A, 0x1A, 0x1A),
    primary: Rgb(0x15, 0x65, 0xC0),
    accent: Rgb(0x00, 0x83, 0x8F),
    text_dim: Rgb(0x45, 0x45, 0x45),
    text_muted: Rgb(0x5F, 0x5F, 0x5F),
    success: Rgb(0x0E, 0x7A, 0x38),
    warning: Rgb(0x92, 0x66, 0x0A),
    error: Rgb(0xB9, 0x1C, 0x1C),
};

impl Palette {
    /// Look a token up by name; both `text_dim` and `textDim` spellings work.
    pub fn token(&self, name: &str) -> Option<Rgb> {
        Some(match name {
            "text" => self.text,
            "primary" => self.primary,
            "accent" => self.accent,
            "text_dim" | "textDim" => self.text_dim,
            "text_muted" | "textMuted" => self.text_muted,
            "success" => self.success,
            "warning" => self.warning,
            "error" => self.error,
            _ => return None,
        })
    }

    pub fn is_light(&self) -> bool {
        self.text == LIGHT.text
    }
}

/// Resolve the palette like upstream's getColorPalette: "light"/"dark" are
/// built in, any other name is `themes/<name>.json` merged over its base, and
/// "auto" collapses to dark (the background probe can't run over a pipe).
/// `override_theme` comes from our own settings and wins over tui.toml.
pub fn palette(override_theme: Option<&str>) -> Palette {
    let theme = override_theme.map(str::to_string).or_else(tui_theme);
    match theme.as_deref() {
        None | Some("auto") | Some("dark") => DARK,
        Some("light") => LIGHT,
        Some(name) => custom_palette(name).unwrap_or(DARK),
    }
}

fn tui_theme() -> Option<String> {
    let text = std::fs::read_to_string(paths::kimi_home().join("tui.toml")).ok()?;
    let doc = text.parse::<toml::Table>().ok()?;
    doc.get("theme")?.as_str().map(str::to_string)
}

fn custom_palette(name: &str) -> Option<Palette> {
    let path = paths::kimi_home()
        .join("themes")
        .join(format!("{name}.json"));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let mut p = if v.get("base").and_then(|b| b.as_str()) == Some("light") {
        LIGHT
    } else {
        DARK
    };
    if let Some(colors) = v.get("colors").and_then(|c| c.as_object()) {
        let pick = |k: &str| colors.get(k).and_then(|c| c.as_str()).and_then(Rgb::parse);
        for (key, slot) in [
            ("text", &mut p.text),
            ("primary", &mut p.primary),
            ("accent", &mut p.accent),
            ("textDim", &mut p.text_dim),
            ("textMuted", &mut p.text_muted),
            ("success", &mut p.success),
            ("warning", &mut p.warning),
            ("error", &mut p.error),
        ] {
            if let Some(c) = pick(key) {
                *slot = c;
            }
        }
    }
    Some(p)
}
