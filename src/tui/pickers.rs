//! Popup pickers: color (16 / 256 / RGB / Kimi palette tokens), icon, and
//! separator, after CCometixLine's components.

use crate::config::{AnsiColor, TOKENS};
use crate::kimi_config::{Palette, Rgb};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

pub fn to_ratatui(c: &AnsiColor, palette: &Palette) -> Option<Color> {
    Some(match c {
        AnsiColor::Color16 { c16 } => Color::Indexed(*c16),
        AnsiColor::Color256 { c256 } => Color::Indexed(*c256),
        _ => {
            let Rgb(r, g, b) = c.to_rgb(palette)?;
            Color::Rgb(r, g, b)
        }
    })
}

pub enum Outcome<T> {
    Pending,
    Cancel,
    Done(T),
}

/// What a mouse click inside a popup means. Clicking an option selects it;
/// clicking the already-selected option chooses it, like Enter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    /// select option `i` (choose it if already selected)
    Option(usize),
    /// a tab/mode switch, by index
    Tab(usize),
    /// a footer button that acts like this key
    Key(KeyCode),
}

/// Clickable rectangles a popup drew this frame.
pub type ClickMap = Vec<(Rect, Click)>;

fn inner(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn footer_button(line: &str, label: &str, key: KeyCode, at: Rect, hits: &mut ClickMap) {
    if let Some(x) = line.find(label) {
        let x = line[..x].chars().count() as u16;
        let w = label.chars().count() as u16;
        if x < at.width {
            hits.push((
                Rect::new(at.x + x, at.y, w.min(at.width - x), 1),
                Click::Key(key),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// color picker
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    Palette,
    Basic16,
    Extended256,
    Rgb,
}

const NAMES16: [&str; 16] = [
    "Black",
    "Red",
    "Green",
    "Yellow",
    "Blue",
    "Magenta",
    "Cyan",
    "White",
    "Dark Gray",
    "Light Red",
    "Light Green",
    "Light Yellow",
    "Light Blue",
    "Light Magenta",
    "Light Cyan",
    "Bright White",
];

pub struct ColorPicker {
    pub title: String,
    mode: ColorMode,
    sel: usize,
    hex: String,
}

impl ColorPicker {
    pub fn new(title: &str, current: &Option<AnsiColor>) -> ColorPicker {
        let (mode, sel, hex) = match current {
            Some(AnsiColor::Color16 { c16 }) => (ColorMode::Basic16, *c16 as usize, String::new()),
            Some(AnsiColor::Color256 { c256 }) => {
                (ColorMode::Extended256, *c256 as usize, String::new())
            }
            Some(AnsiColor::Rgb { r, g, b }) => {
                (ColorMode::Rgb, 0, format!("#{r:02x}{g:02x}{b:02x}"))
            }
            Some(AnsiColor::Named(n)) => (
                ColorMode::Palette,
                TOKENS.iter().position(|t| t == n).map_or(0, |i| i + 1),
                String::new(),
            ),
            None => (ColorMode::Palette, 0, String::new()),
        };
        ColorPicker {
            title: title.into(),
            mode,
            sel,
            hex,
        }
    }

    fn len(&self) -> usize {
        match self.mode {
            ColorMode::Palette => TOKENS.len() + 1,
            ColorMode::Basic16 => 16,
            ColorMode::Extended256 => 256,
            ColorMode::Rgb => 0,
        }
    }

    fn cols(&self) -> usize {
        match self.mode {
            ColorMode::Extended256 => 16,
            ColorMode::Basic16 => 4,
            _ => 1,
        }
    }

    fn selected(&self) -> Result<Option<AnsiColor>, ()> {
        Ok(match self.mode {
            ColorMode::Palette if self.sel == 0 => None,
            ColorMode::Palette => Some(AnsiColor::Named(TOKENS[self.sel - 1].into())),
            ColorMode::Basic16 => Some(AnsiColor::Color16 {
                c16: self.sel as u8,
            }),
            ColorMode::Extended256 => Some(AnsiColor::Color256 {
                c256: self.sel as u8,
            }),
            ColorMode::Rgb => {
                let Rgb(r, g, b) =
                    Rgb::parse(&format!("#{}", self.hex.trim_start_matches('#'))).ok_or(())?;
                Some(AnsiColor::Rgb { r, g, b })
            }
        })
    }

    pub fn click(&mut self, c: Click) -> Outcome<Option<AnsiColor>> {
        match c {
            Click::Tab(i) => {
                let mode = [
                    ColorMode::Palette,
                    ColorMode::Basic16,
                    ColorMode::Extended256,
                    ColorMode::Rgb,
                ][i.min(3)];
                if mode != self.mode {
                    self.mode = mode;
                    self.sel = 0;
                }
                Outcome::Pending
            }
            Click::Option(i) if i == self.sel => self.key(KeyEvent::from(KeyCode::Enter)),
            Click::Option(i) => {
                self.sel = i.min(self.len().saturating_sub(1));
                Outcome::Pending
            }
            Click::Key(k) => self.key(KeyEvent::from(k)),
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome<Option<AnsiColor>> {
        let (len, cols) = (self.len(), self.cols());
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => {
                if let Ok(c) = self.selected() {
                    return Outcome::Done(c);
                }
            }
            KeyCode::Tab => {
                self.mode = match self.mode {
                    ColorMode::Palette => ColorMode::Basic16,
                    ColorMode::Basic16 => ColorMode::Extended256,
                    ColorMode::Extended256 => ColorMode::Rgb,
                    ColorMode::Rgb => ColorMode::Palette,
                };
                self.sel = 0;
            }
            KeyCode::Char(c) if self.mode == ColorMode::Rgb => {
                if c.is_ascii_hexdigit() && self.hex.trim_start_matches('#').len() < 6 {
                    self.hex.push(c);
                }
            }
            KeyCode::Backspace if self.mode == ColorMode::Rgb => {
                self.hex.pop();
            }
            KeyCode::Char('r') => {
                self.mode = ColorMode::Rgb;
            }
            KeyCode::Up if len > 0 => self.sel = (self.sel + len - cols % len.max(1)) % len,
            KeyCode::Down if len > 0 => self.sel = (self.sel + cols) % len,
            KeyCode::Left if len > 0 => self.sel = (self.sel + len - 1) % len,
            KeyCode::Right if len > 0 => self.sel = (self.sel + 1) % len,
            _ => {}
        }
        Outcome::Pending
    }

    pub fn draw(&self, f: &mut Frame, palette: &Palette) -> ClickMap {
        let area = centered(f.area(), 74, 24);
        f.render_widget(Clear, area);
        let body = inner(area);
        let mut hits: ClickMap = Vec::new();
        // tabs on the first line: " Kimi palette " " 16 colors " …
        let mut x = body.x;
        for (i, name) in ["Kimi palette", "16 colors", "256 colors", "RGB"]
            .iter()
            .enumerate()
        {
            let w = name.chars().count() as u16 + 2;
            hits.push((Rect::new(x, body.y, w, 1), Click::Tab(i)));
            x += w + 1;
        }
        // options start on the third line
        let top = body.y + 2;
        let tab = |m: ColorMode, name: &str| {
            let style = if self.mode == m {
                Style::new().bg(Color::Cyan).fg(Color::Black)
            } else {
                Style::new().fg(Color::Gray)
            };
            Span::styled(format!(" {name} "), style)
        };
        let mut lines = vec![
            Line::from(vec![
                tab(ColorMode::Palette, "Kimi palette"),
                Span::raw(" "),
                tab(ColorMode::Basic16, "16 colors"),
                Span::raw(" "),
                tab(ColorMode::Extended256, "256 colors"),
                Span::raw(" "),
                tab(ColorMode::Rgb, "RGB"),
            ]),
            Line::from(""),
        ];
        let cell = |i: usize, c: Color, label: String| {
            let mut style = Style::new().fg(c);
            if i == self.sel {
                style = style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
            }
            Span::styled(label, style)
        };
        match self.mode {
            ColorMode::Palette => {
                for i in 0..=TOKENS.len() {
                    hits.push((Rect::new(body.x, top + i as u16, 24, 1), Click::Option(i)));
                }
                lines.push(Line::from(cell(0, Color::Gray, " ∅ none (inherit)".into())));
                for (i, t) in TOKENS.iter().enumerate() {
                    let c = palette
                        .token(t)
                        .map_or(Color::Gray, |Rgb(r, g, b)| Color::Rgb(r, g, b));
                    lines.push(Line::from(cell(i + 1, c, format!(" ██ {t}"))));
                }
                lines.push(Line::styled(
                    "Follows the Kimi Code theme (dark/light/custom)",
                    Style::new().fg(Color::DarkGray),
                ));
            }
            ColorMode::Basic16 => {
                for i in 0..16u16 {
                    hits.push((
                        Rect::new(body.x + (i % 4) * 19, top + i / 4, 19, 1),
                        Click::Option(i as usize),
                    ));
                }
                for row in 0..4 {
                    let spans = (0..4)
                        .map(|col| {
                            let i = row * 4 + col;
                            cell(
                                i,
                                Color::Indexed(i as u8),
                                format!(" ██ {:<15}", NAMES16[i]),
                            )
                        })
                        .collect::<Vec<_>>();
                    lines.push(Line::from(spans));
                }
            }
            ColorMode::Extended256 => {
                for i in 0..256u16 {
                    hits.push((
                        Rect::new(body.x + (i % 16) * 3, top + i / 16, 3, 1),
                        Click::Option(i as usize),
                    ));
                }
                for row in 0..16 {
                    let spans = (0..16)
                        .map(|col| {
                            let i = row * 16 + col;
                            let label = if i == self.sel { "[█]" } else { " █ " };
                            cell(i, Color::Indexed(i as u8), label.into())
                        })
                        .collect::<Vec<_>>();
                    lines.push(Line::from(spans));
                }
                lines.push(Line::from(format!("selected: c256:{}", self.sel)));
            }
            ColorMode::Rgb => {
                let hex = self.hex.trim_start_matches('#');
                let swatch = Rgb::parse(&format!("#{hex}"))
                    .map(|Rgb(r, g, b)| {
                        Span::styled("  ████████", Style::new().fg(Color::Rgb(r, g, b)))
                    })
                    .unwrap_or(Span::raw(""));
                lines.push(Line::from(vec![Span::raw(format!("Hex: #{hex}█")), swatch]));
                lines.push(Line::styled(
                    "Type 6 hex digits, Enter to apply",
                    Style::new().fg(Color::DarkGray),
                ));
            }
        }
        lines.push(Line::from(""));
        let help = "[Tab] Mode  [←↑↓→] Navigate  [Enter] Select  [Esc] Cancel";
        let help_row = body.y + lines.len() as u16;
        lines.push(Line::styled(help, Style::new().fg(Color::Gray)));
        if help_row < body.bottom() {
            let at = Rect::new(body.x, help_row, body.width, 1);
            footer_button(help, "[Enter] Select", KeyCode::Enter, at, &mut hits);
            footer_button(help, "[Esc] Cancel", KeyCode::Esc, at, &mut hits);
        }
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(self.title.as_str()),
            ),
            area,
        );
        hits.retain(|(r, _)| r.y < body.bottom());
        hits
    }
}

// ---------------------------------------------------------------------------
// icon picker
// ---------------------------------------------------------------------------

const PLAIN_ICONS: [(&str, &str); 32] = [
    ("🤖", "Robot"),
    ("🧠", "Brain"),
    ("✨", "Sparkles"),
    ("⚡️", "Lightning"),
    ("📁", "Folder"),
    ("📂", "Open folder"),
    ("🌿", "Branch"),
    ("🔀", "Merge"),
    ("📊", "Chart"),
    ("📈", "Trending"),
    ("⏳", "Hourglass"),
    ("⏱️", "Stopwatch"),
    ("🛡️", "Shield"),
    ("🎯", "Target"),
    ("⚙️", "Gear"),
    ("🧩", "Puzzle"),
    ("💰", "Money"),
    ("🔋", "Battery"),
    ("🔥", "Fire"),
    ("🚀", "Rocket"),
    ("✽", "Star"),
    ("◐", "Half circle"),
    ("◑", "Half circle 2"),
    ("◔", "Quarter circle"),
    ("※", "Reference"),
    ("Σ", "Sigma"),
    ("⊕", "Circled plus"),
    ("◷", "Clock"),
    ("◆", "Diamond"),
    ("◎", "Bullseye"),
    ("•", "Bullet"),
    ("│", "Bar"),
];

const NERD_ICONS: [(&str, &str); 28] = [
    ("\u{e26d}", "nf-seti-config"),
    ("\u{f06a9}", "nf-md-robot"),
    ("\u{f2d0}", "nf-fa-window"),
    ("\u{f024b}", "nf-md-folder"),
    ("\u{f07b}", "nf-fa-folder"),
    ("\u{f02a2}", "nf-md-git"),
    ("\u{e725}", "nf-dev-git_branch"),
    ("\u{f407}", "nf-oct-git_pull_request"),
    ("\u{f49b}", "nf-oct-zap"),
    ("\u{f0a9e}", "nf-md-circle_slice_1"),
    ("\u{f0aa1}", "nf-md-circle_slice_4"),
    ("\u{f0aa5}", "nf-md-circle_slice_8"),
    ("\u{f0e7}", "nf-fa-bolt"),
    ("\u{f19bb}", "nf-md-timer"),
    ("\u{f0483}", "nf-md-shield"),
    ("\u{f0136}", "nf-md-bullseye"),
    ("\u{f0493}", "nf-md-cog"),
    ("\u{f0bc5}", "nf-md-puzzle"),
    ("\u{eec1}", "nf-fa-money"),
    ("\u{f0c7}", "nf-fa-save"),
    ("\u{f017}", "nf-fa-clock"),
    ("\u{f0e4}", "nf-fa-tachometer"),
    ("\u{f080}", "nf-fa-bar_chart"),
    ("\u{f135}", "nf-fa-rocket"),
    ("\u{f06d}", "nf-fa-fire"),
    ("\u{f0eb}", "nf-fa-lightbulb"),
    ("\u{f1b2}", "nf-fa-cube"),
    ("\u{f121}", "nf-fa-code"),
];

pub struct IconPicker {
    pub nerd: bool,
    sel: usize,
    custom: Option<String>,
    /// list scroll position from the last draw, for mapping clicks
    offset: std::cell::Cell<usize>,
}

impl IconPicker {
    pub fn new(nerd: bool) -> IconPicker {
        IconPicker {
            nerd,
            sel: 0,
            custom: None,
            offset: std::cell::Cell::new(0),
        }
    }

    fn icons(&self) -> &'static [(&'static str, &'static str)] {
        if self.nerd {
            &NERD_ICONS
        } else {
            &PLAIN_ICONS
        }
    }

    pub fn click(&mut self, c: Click) -> Outcome<(bool, String)> {
        match c {
            Click::Option(i) if i == self.sel && self.custom.is_none() => {
                self.key(KeyEvent::from(KeyCode::Enter))
            }
            Click::Option(i) => {
                self.custom = None;
                self.sel = i.min(self.icons().len());
                Outcome::Pending
            }
            Click::Tab(_) => self.key(KeyEvent::from(KeyCode::Tab)),
            Click::Key(k) => self.key(KeyEvent::from(k)),
        }
    }

    pub fn scroll(&mut self, down: bool) {
        if self.custom.is_none() {
            let _ = self.key(KeyEvent::from(if down {
                KeyCode::Down
            } else {
                KeyCode::Up
            }));
        }
    }

    /// Done((nerd?, icon)) — the style is returned so the caller writes the
    /// right slot even if the user switched tabs.
    pub fn key(&mut self, k: KeyEvent) -> Outcome<(bool, String)> {
        if let Some(input) = self.custom.as_mut() {
            match k.code {
                KeyCode::Esc => self.custom = None,
                KeyCode::Enter => return Outcome::Done((self.nerd, input.clone())),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
            return Outcome::Pending;
        }
        let len = self.icons().len() + 1; // + "no icon"
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Tab => {
                self.nerd = !self.nerd;
                self.sel = 0;
            }
            KeyCode::Char('c') => self.custom = Some(String::new()),
            KeyCode::Up => self.sel = (self.sel + len - 1) % len,
            KeyCode::Down => self.sel = (self.sel + 1) % len,
            KeyCode::Enter => {
                let icon = if self.sel == 0 {
                    ""
                } else {
                    self.icons()[self.sel - 1].0
                };
                return Outcome::Done((self.nerd, icon.into()));
            }
            _ => {}
        }
        Outcome::Pending
    }

    pub fn draw(&self, f: &mut Frame) -> ClickMap {
        let area = centered(f.area(), 52, 24);
        f.render_widget(Clear, area);
        let mut hits: ClickMap = Vec::new();
        let title = if self.nerd {
            "Icon — Nerd Font"
        } else {
            "Icon — Plain"
        };
        let [list_area, help] =
            Layout::vertical([Constraint::Min(5), Constraint::Length(3)]).areas(area);
        let mut items = vec![ListItem::new("  (no icon)")];
        items.extend(
            self.icons()
                .iter()
                .map(|(i, name)| ListItem::new(format!("  {i}   {name}"))),
        );
        let n = items.len();
        let mut state = ListState::default()
            .with_selected(Some(self.sel))
            .with_offset(self.offset.get());
        f.render_stateful_widget(
            List::new(items)
                .highlight_style(Style::new().bg(Color::Cyan).fg(Color::Black))
                .block(Block::default().borders(Borders::ALL).title(title)),
            list_area,
            &mut state,
        );
        self.offset.set(state.offset());
        let list_inner = inner(list_area);
        for (i, row) in (state.offset()..n).zip(list_inner.y..list_inner.bottom()) {
            hits.push((
                Rect::new(list_inner.x, row, list_inner.width, 1),
                Click::Option(i),
            ));
        }
        // the title doubles as the Plain / Nerd Font toggle
        hits.push((
            Rect::new(list_area.x, list_area.y, list_area.width, 1),
            Click::Tab(0),
        ));
        let text = match &self.custom {
            Some(input) => format!("Custom: {input}█   [Enter] Apply [Esc] Back"),
            None => "[↑↓] Navigate  [Tab] Plain/Nerd  [C] Custom  [Enter] Select".into(),
        };
        let at = Rect::new(help.x + 1, help.y + 1, help.width.saturating_sub(2), 1);
        if self.custom.is_some() {
            footer_button(&text, "[Enter] Apply", KeyCode::Enter, at, &mut hits);
            footer_button(&text, "[Esc] Back", KeyCode::Esc, at, &mut hits);
        } else {
            footer_button(&text, "[Tab] Plain/Nerd", KeyCode::Tab, at, &mut hits);
            footer_button(&text, "[C] Custom", KeyCode::Char('c'), at, &mut hits);
            footer_button(&text, "[Enter] Select", KeyCode::Enter, at, &mut hits);
        }
        f.render_widget(
            Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
            help,
        );
        hits
    }
}

// ---------------------------------------------------------------------------
// separator editor
// ---------------------------------------------------------------------------

const SEPARATORS: [(&str, &str, &str); 7] = [
    ("Space", "  ", "Two spaces, like Kimi Code's own footer"),
    ("Pipe", " | ", "Classic pipe separator"),
    ("Thin", " │ ", "Thin vertical line"),
    ("Dot", " • ", "Middle dot"),
    ("Slash", " / ", "Forward slash"),
    ("Chevron", " › ", "Single chevron"),
    (
        "Arrow",
        "\u{e0b0}",
        "Powerline arrow (needs segment backgrounds)",
    ),
];

pub struct SeparatorEditor {
    sel: Option<usize>,
    input: String,
}

impl SeparatorEditor {
    pub fn new(current: &str) -> SeparatorEditor {
        let sel = SEPARATORS.iter().position(|(_, v, _)| *v == current);
        SeparatorEditor {
            sel,
            input: if sel.is_some() {
                String::new()
            } else {
                current.into()
            },
        }
    }

    pub fn click(&mut self, c: Click) -> Outcome<String> {
        match c {
            Click::Option(i) if self.sel == Some(i) => self.key(KeyEvent::from(KeyCode::Enter)),
            Click::Option(i) => {
                self.sel = Some(i.min(SEPARATORS.len() - 1));
                Outcome::Pending
            }
            Click::Tab(_) => {
                // the custom line: switch to typing
                self.sel = None;
                Outcome::Pending
            }
            Click::Key(k) => self.key(KeyEvent::from(k)),
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome<String> {
        let n = SEPARATORS.len();
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Up => self.sel = Some(self.sel.map_or(n - 1, |i| (i + n - 1) % n)),
            KeyCode::Down => self.sel = Some(self.sel.map_or(0, |i| (i + 1) % n)),
            KeyCode::Tab => {
                self.sel = None;
                self.input.clear();
            }
            KeyCode::Char(c) => {
                self.sel = None;
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                return Outcome::Done(match self.sel {
                    Some(i) => SEPARATORS[i].1.into(),
                    None => self.input.clone(),
                })
            }
            _ => {}
        }
        Outcome::Pending
    }

    pub fn draw(&self, f: &mut Frame) -> ClickMap {
        let area = centered(f.area(), 64, 16);
        f.render_widget(Clear, area);
        let body = inner(area);
        let mut hits: ClickMap = (0..SEPARATORS.len())
            .map(|i| {
                (
                    Rect::new(body.x, body.y + i as u16, body.width, 1),
                    Click::Option(i),
                )
            })
            .collect();
        let custom_row = body.y + SEPARATORS.len() as u16 + 1;
        hits.push((Rect::new(body.x, custom_row, body.width, 1), Click::Tab(0)));
        let mut lines: Vec<Line> = SEPARATORS
            .iter()
            .enumerate()
            .map(|(i, (name, v, d))| {
                let style = if self.sel == Some(i) {
                    Style::new().bg(Color::Cyan).fg(Color::Black)
                } else {
                    Style::new()
                };
                Line::styled(format!(" {name:8} {:>5}  {d}", format!("{v:?}")), style)
            })
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(format!(" Custom: {}█", self.input)));
        let help = " [↑↓] Presets  type for custom  [Tab] Clear  [Enter] Apply";
        lines.push(Line::styled(help, Style::new().fg(Color::Gray)));
        let at = Rect::new(body.x, custom_row + 1, body.width, 1);
        footer_button(help, "[Tab] Clear", KeyCode::Tab, at, &mut hits);
        footer_button(help, "[Enter] Apply", KeyCode::Enter, at, &mut hits);
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Separator")),
            area,
        );
        hits.retain(|(r, _)| r.y < body.bottom());
        hits
    }
}
