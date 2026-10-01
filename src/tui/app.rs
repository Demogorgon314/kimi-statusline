//! The configurator: title, live preview, theme bar, segment list, settings
//! panel and a key-help bar, laid out like CCometixLine's.

use super::pickers::{
    centered, to_ratatui, Click, ClickMap, ColorPicker, IconPicker, Outcome, SeparatorEditor,
};
use super::{button, button_bar, Button, Hits};
use crate::config::{AnsiColor, Config, Lang, SegmentConfig, SegmentId, StyleMode};
use crate::quota::{Entry, Quota};
use crate::render::Ctx;
use crate::session::{SessionStats, Usage};
use crate::{collect, kimi_config, themes};
use ansi_to_tui::IntoText;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Segments,
    Settings,
}

#[derive(Clone, PartialEq)]
enum Field {
    Enabled,
    Icon,
    IconColor,
    TextColor,
    BgColor,
    Bold,
    Opt(String),
}

enum Popup {
    Color(ColorPicker, Field),
    Icon(IconPicker),
    Separator(SeparatorEditor),
    Text {
        title: String,
        input: String,
        target: TextTarget,
    },
    Help,
}

enum TextTarget {
    Option(String),
    SaveTheme,
}

/// What a click on the main screen lands on.
#[derive(Clone)]
enum Target {
    Segment(usize),
    /// the [✓] box of a segment: toggles without selecting first
    SegmentCheck(usize),
    Field(usize),
    Theme(String),
    /// "mode: …" etc. in the preview border
    Key(KeyEvent),
    /// click anywhere to dismiss (help popup)
    Dismiss,
    Popup(Click),
    /// inside a popup but not on anything: swallow
    Inert,
    /// outside an open popup: close it
    Outside,
}

pub struct App {
    config: Config,
    saved: Config,
    ctx: Ctx,
    panel: Panel,
    seg: usize,
    field: usize,
    popup: Option<Popup>,
    status: Option<String>,
    confirm_quit: bool,
    quit: bool,
    hits: Hits<Target>,
    /// segment list geometry from the last draw: (inner rect, scroll offset)
    seg_list: Option<(Rect, usize)>,
    /// left button held on a segment row
    press: Option<Press>,
}

/// A left-button press on a segment row, which becomes a drag once the
/// pointer leaves the row it started on.
struct Press {
    /// the segment, by its current index (follows it while dragging)
    index: usize,
    /// it was already selected when pressed: releasing without a drag
    /// toggles it, like a second click
    was_selected: bool,
    start_row: u16,
    dragging: bool,
}

/// Plausible numbers for whatever the current directory's session lacks, so
/// every segment shows up in the preview.
fn fill_demo(ctx: &mut Ctx) {
    let u = |i, o, c| Usage {
        input_other: i,
        output: o,
        input_cache_read: c,
        input_cache_creation: 0,
    };
    if ctx.stats.as_ref().is_none_or(|s| s.total.is_empty()) {
        let mut st = SessionStats {
            total: u(48_000, 21_400, 1_210_000),
            ..Default::default()
        };
        st.sub_by_model
            .insert("kimi-code/k3".into(), u(9_000, 4_100, 310_000));
        ctx.stats = Some(st);
    }
    if ctx.payload.max_context_tokens == 0 {
        ctx.payload.context_tokens = 98_000;
        ctx.payload.max_context_tokens = 262_144;
        ctx.payload.context_usage = 98_000.0 / 262_144.0;
    }
    if ctx.quota.is_none() {
        let reset = |h: i64| (chrono::Utc::now() + chrono::Duration::hours(h)).to_rfc3339();
        ctx.quota = Some(Quota {
            limit_5h: Some(Entry {
                used_ratio: 0.42,
                reset_at: Some(reset(2)),
            }),
            limit_7d: Some(Entry {
                used_ratio: 0.13,
                reset_at: Some(reset(90)),
            }),
            month: Some(Entry {
                used_ratio: 0.31,
                reset_at: Some(reset(400)),
            }),
        });
    }
    if ctx.session_created.is_none() {
        ctx.session_created = Some(ctx.now - 3_720.0);
    }
    if ctx.goal.is_none() {
        ctx.goal =
            Some(serde_json::json!({"status": "active", "turnsUsed": 7, "wallClockMs": 240_000}));
    }
    if ctx.tasks == (0, 0) {
        ctx.tasks = (1, 0);
    }
    ctx.payload.plan_mode = true;
}

fn color_desc(c: &Option<AnsiColor>) -> String {
    match c {
        None => "Default".into(),
        Some(AnsiColor::Color16 { c16 }) => format!("16: {c16}"),
        Some(AnsiColor::Color256 { c256 }) => format!("256: {c256}"),
        Some(c) => c.describe(),
    }
}

impl App {
    pub fn new() -> App {
        let config = Config::load();
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let payload = collect::sample_payload(&cwd, None);
        let mut ctx = collect::collect(payload, config.clone(), Instant::now());
        fill_demo(&mut ctx);
        App {
            saved: config.clone(),
            config,
            ctx,
            panel: Panel::Segments,
            seg: 0,
            field: 0,
            popup: None,
            status: None,
            confirm_quit: false,
            quit: false,
            hits: Hits::new(),
            seg_list: None,
            press: None,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let mut redraw = true;
        while !self.quit {
            if redraw {
                terminal.draw(|f| self.draw(f))?;
            }
            redraw = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    self.key(k);
                    true
                }
                Event::Mouse(m) => self.mouse(m),
                Event::Resize(..) => true,
                _ => false,
            };
        }
        Ok(())
    }

    fn current(&self) -> Option<&SegmentConfig> {
        self.config.segments.get(self.seg)
    }

    fn current_mut(&mut self) -> Option<&mut SegmentConfig> {
        self.config.segments.get_mut(self.seg)
    }

    fn fields(&self) -> Vec<Field> {
        let mut f = vec![
            Field::Enabled,
            Field::Icon,
            Field::IconColor,
            Field::TextColor,
            Field::BgColor,
            Field::Bold,
        ];
        if let Some(seg) = self.current() {
            f.extend(seg.options.keys().map(|k| Field::Opt(k.clone())));
        }
        f
    }

    fn preview(&mut self, width: usize) -> String {
        let name =
            (!self.config.style.palette.is_empty()).then_some(self.config.style.palette.as_str());
        self.ctx.palette = kimi_config::palette(name);
        self.ctx.config = self.config.clone();
        crate::render::render(&self.ctx, Some(width))
    }

    fn switch_theme(&mut self, name: &str) {
        let mut cfg = themes::get(name);
        cfg.style.lang = self.config.style.lang;
        cfg.style.palette = self.config.style.palette.clone();
        self.config = cfg;
        self.seg = self.seg.min(self.config.segments.len().saturating_sub(1));
        self.field = 0;
        self.status = Some(format!("Switched to {name} theme"));
    }

    fn cycle_theme(&mut self, step: isize) {
        let list = themes::list();
        let i = list
            .iter()
            .position(|n| *n == self.config.theme)
            .unwrap_or(0) as isize;
        let name = list[(i + step).rem_euclid(list.len() as isize) as usize].clone();
        self.switch_theme(&name);
    }

    fn toggle_segment(&mut self) {
        if let Some(s) = self.current_mut() {
            s.enabled = !s.enabled;
            let msg = format!(
                "{} segment {}",
                s.id.name(),
                if s.enabled { "enabled" } else { "disabled" }
            );
            self.status = Some(msg);
        }
    }

    /// Returns whether anything changed.
    fn mouse(&mut self, m: MouseEvent) -> bool {
        let scroll = match m.kind {
            MouseEventKind::Down(MouseButton::Left) => None,
            MouseEventKind::Drag(MouseButton::Left) => return self.drag(m.row),
            MouseEventKind::Up(MouseButton::Left) => return self.release(),
            MouseEventKind::ScrollUp => Some(false),
            MouseEventKind::ScrollDown => Some(true),
            _ => return false,
        };
        self.press = None;
        if let Some(down) = scroll {
            return self.scroll(down, m.column, m.row);
        }
        let Some(target) = self.hits.at(m.column, m.row) else {
            return false;
        };
        // a click on [Esc] Quit goes through key(), which keeps the pending
        // quit confirmation; anything else cancels it
        if !matches!(&target, Target::Key(k) if k.code == KeyCode::Esc) {
            self.confirm_quit = false;
        }
        match target {
            Target::Segment(i) => {
                // select now; toggling (on a second click) or reordering
                // (on a drag) is decided when the button comes up
                let was_selected = self.panel == Panel::Segments && self.seg == i;
                self.panel = Panel::Segments;
                self.seg = i;
                if !was_selected {
                    self.field = 0;
                }
                self.press = Some(Press {
                    index: i,
                    was_selected,
                    start_row: m.row,
                    dragging: false,
                });
            }
            Target::SegmentCheck(i) => {
                self.panel = Panel::Segments;
                self.seg = i;
                self.field = 0;
                self.toggle_segment();
            }
            Target::Field(i) => {
                if self.panel == Panel::Settings && self.field == i {
                    self.edit_field();
                } else {
                    self.panel = Panel::Settings;
                    self.field = i;
                }
            }
            Target::Theme(name) => self.switch_theme(&name),
            Target::Key(k) => self.key(k),
            Target::Dismiss | Target::Outside => self.popup = None,
            Target::Popup(c) => {
                if let Some(popup) = self.popup.take() {
                    self.popup_click(popup, c);
                }
            }
            Target::Inert => {}
        }
        true
    }

    /// Segment index under a screen row of the segment list, clamped to the
    /// visible rows so dragging past either end parks it at that end.
    fn segment_at_row(&self, row: u16) -> Option<usize> {
        let (inner, offset) = self.seg_list?;
        let n = self.config.segments.len();
        if n == 0 || inner.height == 0 {
            return None;
        }
        let last_row = inner.y + (inner.height.min((n - offset) as u16)) - 1;
        let row = row.clamp(inner.y, last_row);
        Some((offset + (row - inner.y) as usize).min(n - 1))
    }

    /// Drag a pressed segment: it moves live, so the preview shows the new
    /// order while the button is still held.
    fn drag(&mut self, row: u16) -> bool {
        let Some(press) = &self.press else {
            return false;
        };
        if !press.dragging && row == press.start_row {
            return false;
        }
        let from = press.index;
        let Some(to) = self.segment_at_row(row) else {
            return false;
        };
        let press = self.press.as_mut().expect("press");
        press.dragging = true;
        if to != from {
            let seg = self.config.segments.remove(from);
            self.config.segments.insert(to, seg);
            press.index = to;
            self.seg = to;
        }
        true
    }

    fn release(&mut self) -> bool {
        let Some(press) = self.press.take() else {
            return false;
        };
        if press.dragging {
            let name = self.config.segments[press.index].id.name();
            self.status = Some(format!(
                "Moved {name} to position {} of {}",
                press.index + 1,
                self.config.segments.len()
            ));
        } else if press.was_selected {
            self.toggle_segment();
        }
        true
    }

    /// The wheel moves whatever list is under the pointer.
    fn scroll(&mut self, down: bool, column: u16, row: u16) -> bool {
        let step = if down { 1 } else { -1 };
        match &mut self.popup {
            Some(Popup::Icon(p)) => p.scroll(down),
            Some(_) => return false,
            None => match self.hits.at(column, row) {
                Some(Target::Field(_)) => {
                    self.panel = Panel::Settings;
                    self.nav(step);
                }
                Some(Target::Segment(_) | Target::SegmentCheck(_)) => {
                    self.panel = Panel::Segments;
                    self.nav(step);
                }
                _ => self.nav(step),
            },
        }
        true
    }

    fn popup_click(&mut self, popup: Popup, c: Click) {
        match popup {
            Popup::Color(mut p, field) => match p.click(c) {
                Outcome::Pending => self.popup = Some(Popup::Color(p, field)),
                Outcome::Cancel => {}
                Outcome::Done(color) => self.apply_color(field, color),
            },
            Popup::Icon(mut p) => match p.click(c) {
                Outcome::Pending => self.popup = Some(Popup::Icon(p)),
                Outcome::Cancel => {}
                Outcome::Done((nerd, icon)) => self.apply_icon(nerd, icon),
            },
            Popup::Separator(mut p) => match p.click(c) {
                Outcome::Pending => self.popup = Some(Popup::Separator(p)),
                Outcome::Cancel => {}
                Outcome::Done(sep) => self.apply_separator(sep),
            },
            Popup::Text {
                title,
                input,
                target,
            } => match c {
                Click::Key(KeyCode::Enter) => self.commit_text(input, target),
                Click::Key(KeyCode::Esc) => {}
                _ => {
                    self.popup = Some(Popup::Text {
                        title,
                        input,
                        target,
                    })
                }
            },
            Popup::Help => {}
        }
    }

    fn apply_color(&mut self, field: Field, color: Option<AnsiColor>) {
        if let Some(seg) = self.current_mut() {
            match field {
                Field::IconColor => seg.colors.icon = color,
                Field::TextColor => seg.colors.text = color,
                Field::BgColor => seg.colors.background = color,
                _ => {}
            }
        }
        self.status = Some("Color updated".into());
    }

    fn apply_icon(&mut self, nerd: bool, icon: String) {
        if let Some(seg) = self.current_mut() {
            if nerd {
                seg.icon.nerd_font = icon;
            } else {
                seg.icon.plain = icon;
            }
        }
        self.status = Some(format!(
            "{} icon updated",
            if nerd { "Nerd Font" } else { "Plain" }
        ));
    }

    fn apply_separator(&mut self, sep: String) {
        self.config.style.separator = sep;
        self.status = Some("Separator updated".into());
    }

    fn key(&mut self, k: KeyEvent) {
        if let Some(popup) = self.popup.take() {
            self.popup_key(popup, k);
            return;
        }
        if !matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) {
            self.confirm_quit = false;
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.config != self.saved && !self.confirm_quit {
                    self.confirm_quit = true;
                    self.status =
                        Some("Unsaved changes — press Esc again to discard, S to save".into());
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('?') => self.popup = Some(Popup::Help),
            KeyCode::Char('s') if ctrl => {
                self.popup = Some(Popup::Text {
                    title: "Save as New Theme".into(),
                    input: String::new(),
                    target: TextTarget::SaveTheme,
                })
            }
            KeyCode::Char('s') | KeyCode::Char('S') => match self.config.save() {
                Ok(()) => {
                    self.saved = self.config.clone();
                    self.status = Some(
                        "Configuration saved — the status line picks it up on its next refresh"
                            .into(),
                    );
                }
                Err(e) => self.status = Some(format!("Failed to save config: {e}")),
            },
            KeyCode::Char('w') | KeyCode::Char('W') => {
                let name = self.config.theme.clone();
                self.status = Some(match themes::save(&name, &self.config) {
                    Ok(p) => format!("Wrote config to theme {name} ({})", p.display()),
                    Err(e) => format!("Failed to write theme {name}: {e}"),
                });
            }
            KeyCode::Char(c @ '1'..='9') => {
                let list = themes::list();
                if let Some(name) = list.get(c as usize - '1' as usize).cloned() {
                    self.switch_theme(&name);
                }
            }
            KeyCode::Char('p') => self.cycle_theme(1),
            KeyCode::Char('P') => self.cycle_theme(-1),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                let name = self.config.theme.clone();
                self.switch_theme(&name);
                self.status = Some(format!("Reset {name} theme to defaults"));
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                self.popup = Some(Popup::Separator(SeparatorEditor::new(
                    &self.config.style.separator,
                )))
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                let i = StyleMode::ALL
                    .iter()
                    .position(|m| *m == self.config.style.mode)
                    .unwrap_or(0);
                self.config.style.mode = StyleMode::ALL[(i + 1) % StyleMode::ALL.len()];
                if self.config.style.mode == StyleMode::Powerline {
                    self.config.style.separator = "\u{e0b0}".into();
                } else if self.config.style.separator == "\u{e0b0}" {
                    self.config.style.separator = " | ".into();
                }
                self.status = Some(format!("Style mode: {}", self.config.style.mode.name()));
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                self.config.style.lang = match self.config.style.lang {
                    Lang::En => Lang::Zh,
                    Lang::Zh => Lang::En,
                };
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.config.style.palette = match self.config.style.palette.as_str() {
                    "" => "dark",
                    "dark" => "light",
                    _ => "",
                }
                .into();
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.panel = match self.panel {
                    Panel::Segments => Panel::Settings,
                    Panel::Settings => Panel::Segments,
                }
            }
            KeyCode::Up if shift => self.move_segment(-1),
            KeyCode::Down if shift => self.move_segment(1),
            KeyCode::Char('K') => self.move_segment(-1),
            KeyCode::Char('J') => self.move_segment(1),
            KeyCode::Up | KeyCode::Char('k') => self.nav(-1),
            KeyCode::Down | KeyCode::Char('j') => self.nav(1),
            KeyCode::Left | KeyCode::Right => {
                self.step_field(if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Char(' ') if self.panel == Panel::Segments => self.toggle_segment(),
            KeyCode::Enter | KeyCode::Char(' ') => match self.panel {
                Panel::Segments => self.toggle_segment(),
                Panel::Settings => self.edit_field(),
            },
            _ => {}
        }
    }

    fn nav(&mut self, step: isize) {
        let len = match self.panel {
            Panel::Segments => self.config.segments.len(),
            Panel::Settings => self.fields().len(),
        };
        if len == 0 {
            return;
        }
        let idx = match self.panel {
            Panel::Segments => &mut self.seg,
            Panel::Settings => &mut self.field,
        };
        *idx = (*idx as isize + step).clamp(0, len as isize - 1) as usize;
        if self.panel == Panel::Segments {
            self.field = 0;
        }
    }

    fn move_segment(&mut self, step: isize) {
        if self.panel != Panel::Segments {
            return;
        }
        let to = self.seg as isize + step;
        if (0..self.config.segments.len() as isize).contains(&to) {
            self.config.segments.swap(self.seg, to as usize);
            self.seg = to as usize;
            self.status = Some(format!(
                "Moved segment {}",
                if step < 0 { "up" } else { "down" }
            ));
        }
    }

    /// ←/→: flip switches and step numbers in place.
    fn step_field(&mut self, step: i64) {
        if self.panel != Panel::Settings {
            return;
        }
        let Some(field) = self.fields().get(self.field).cloned() else {
            return;
        };
        let Some(seg) = self.current_mut() else {
            return;
        };
        match field {
            Field::Enabled => seg.enabled = !seg.enabled,
            Field::Bold => seg.styles.text_bold = !seg.styles.text_bold,
            Field::Opt(k) => match seg.options.get_mut(&k) {
                Some(toml::Value::Boolean(b)) => *b = !*b,
                Some(toml::Value::Integer(n)) => {
                    let unit = if k == "refresh_secs" { 30 } else { 1 };
                    *n = (*n + step * unit).max(0);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn edit_field(&mut self) {
        let Some(field) = self.fields().get(self.field).cloned() else {
            return;
        };
        let Some(seg) = self.current() else { return };
        let popup = match &field {
            Field::Enabled | Field::Bold => return self.step_field(1),
            Field::Icon => Popup::Icon(IconPicker::new(self.config.style.mode != StyleMode::Plain)),
            Field::IconColor => {
                Popup::Color(ColorPicker::new("Icon Color", &seg.colors.icon), field)
            }
            Field::TextColor => {
                Popup::Color(ColorPicker::new("Text Color", &seg.colors.text), field)
            }
            Field::BgColor => Popup::Color(
                ColorPicker::new("Background Color", &seg.colors.background),
                field,
            ),
            Field::Opt(k) => match seg.options.get(k) {
                Some(toml::Value::Boolean(_)) => return self.step_field(1),
                Some(v) => Popup::Text {
                    title: format!("Option: {k}"),
                    input: v.to_string().trim_matches('"').to_string(),
                    target: TextTarget::Option(k.clone()),
                },
                None => return,
            },
        };
        self.popup = Some(popup);
    }

    fn popup_key(&mut self, popup: Popup, k: KeyEvent) {
        match popup {
            Popup::Help => {}
            Popup::Color(mut p, field) => match p.key(k) {
                Outcome::Pending => self.popup = Some(Popup::Color(p, field)),
                Outcome::Cancel => {}
                Outcome::Done(color) => self.apply_color(field, color),
            },
            Popup::Icon(mut p) => match p.key(k) {
                Outcome::Pending => self.popup = Some(Popup::Icon(p)),
                Outcome::Cancel => {}
                Outcome::Done((nerd, icon)) => self.apply_icon(nerd, icon),
            },
            Popup::Separator(mut p) => match p.key(k) {
                Outcome::Pending => self.popup = Some(Popup::Separator(p)),
                Outcome::Cancel => {}
                Outcome::Done(sep) => self.apply_separator(sep),
            },
            Popup::Text {
                title,
                mut input,
                target,
            } => match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.commit_text(input, target),
                KeyCode::Backspace => {
                    input.pop();
                    self.popup = Some(Popup::Text {
                        title,
                        input,
                        target,
                    });
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    self.popup = Some(Popup::Text {
                        title,
                        input,
                        target,
                    });
                }
                _ => {
                    self.popup = Some(Popup::Text {
                        title,
                        input,
                        target,
                    })
                }
            },
        }
    }

    fn commit_text(&mut self, input: String, target: TextTarget) {
        match target {
            TextTarget::SaveTheme => {
                let name = input.trim().to_string();
                self.status = Some(match themes::save(&name, &self.config) {
                    Ok(_) => {
                        self.config.theme = name.clone();
                        format!("Saved as new theme: {name}")
                    }
                    Err(e) => format!("Failed to save theme: {e}"),
                });
            }
            TextTarget::Option(k) => {
                if let Some(seg) = self.current_mut() {
                    let v = match input.trim().parse::<i64>() {
                        Ok(n) => toml::Value::Integer(n),
                        Err(_) => toml::Value::String(input),
                    };
                    seg.options.insert(k, v);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // drawing
    // -----------------------------------------------------------------------

    fn draw(&mut self, f: &mut Frame) {
        self.hits.clear();
        let area = f.area();
        let inner_w = area.width.saturating_sub(2) as usize;
        let preview = self.preview(inner_w.max(20));
        let themes_list = themes::list();
        let (theme_lines, theme_pos) = theme_bar(&themes_list, &self.config.theme, inner_w);
        let help = help_buttons(self.panel);
        let probe = Rect::new(0, 0, inner_w as u16, u16::MAX);
        // one row is always reserved for the status message, so a message
        // appearing never moves the buttons out from under the pointer
        let help_lines = button_bar(&help, probe).0.len() as u16 + 1;

        let [title, preview_area, theme_area, body, help_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(theme_lines.len() as u16 + 2),
            Constraint::Min(10),
            Constraint::Length(help_lines + 2),
        ])
        .areas(area);

        let dirty = if self.config != self.saved {
            "  ● unsaved"
        } else {
            ""
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(
                        "kimi-statusline Configurator v{}",
                        env!("CARGO_PKG_VERSION")
                    ),
                    Style::new().fg(Color::Cyan),
                ),
                Span::styled(dirty, Style::new().fg(Color::Yellow)),
            ]))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL)),
            title,
        );

        let text = strip_osc(&preview)
            .into_text()
            .unwrap_or_else(|_| Text::raw(preview.clone()));
        // the style summary in the preview's bottom border: each part is a
        // button for the key that changes it
        let parts = [
            (format!("mode: {}", self.config.style.mode.name()), 'm'),
            (format!("sep: {:?}", self.config.style.separator), 'e'),
            (
                format!(
                    "lang: {}",
                    match self.config.style.lang {
                        Lang::En => "en",
                        Lang::Zh => "zh",
                    }
                ),
                'l',
            ),
            (
                format!(
                    "colors: {}",
                    if self.config.style.palette.is_empty() {
                        "tui.toml"
                    } else {
                        &self.config.style.palette
                    }
                ),
                'c',
            ),
        ];
        let style_info = format!(
            " {} ",
            parts
                .iter()
                .map(|(t, _)| t.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        );
        let info_w = style_info.chars().count() as u16;
        let info_x = preview_area.right().saturating_sub(1 + info_w);
        let mut x = info_x + 1;
        for (text, key) in &parts {
            let w = text.chars().count() as u16;
            self.hits.push(
                Rect::new(x, preview_area.bottom().saturating_sub(1), w, 1),
                Target::Key(KeyEvent::from(KeyCode::Char(*key))),
            );
            x += w + 3;
        }
        f.render_widget(
            Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Preview")
                    .title_bottom(
                        Line::styled(style_info, Style::new().fg(Color::DarkGray)).right_aligned(),
                    ),
            ),
            preview_area,
        );

        f.render_widget(
            Paragraph::new(theme_lines)
                .block(Block::default().borders(Borders::ALL).title("Themes")),
            theme_area,
        );
        for (name, (line, x, w)) in themes_list.iter().zip(theme_pos) {
            let row = theme_area.y + 1 + line;
            if row < theme_area.bottom().saturating_sub(1) {
                self.hits.push(
                    Rect::new(theme_area.x + 1 + x, row, w, 1),
                    Target::Theme(name.clone()),
                );
            }
        }

        let [list_area, settings_area] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(body);
        self.draw_segments(f, list_area);
        self.draw_settings(f, settings_area);

        let help_inner = Rect::new(
            help_area.x + 1,
            help_area.y + 1,
            help_area.width.saturating_sub(2),
            help_area.height.saturating_sub(2),
        );
        let (mut lines, help_hits) = button_bar(&help, help_inner);
        for (r, k) in help_hits {
            self.hits.push(r, Target::Key(k));
        }
        if let Some(s) = &self.status {
            lines.push(Line::styled(s.as_str(), Style::new().fg(Color::Green)));
        }
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Help")),
            help_area,
        );

        let palette = self.ctx.palette;
        // an open popup is modal: it takes over the whole hit map
        let popup_hits: Option<(Rect, ClickMap)> = match &self.popup {
            Some(Popup::Color(p, _)) => Some((centered(area, 74, 24), p.draw(f, &palette))),
            Some(Popup::Icon(p)) => Some((centered(area, 52, 24), p.draw(f))),
            Some(Popup::Separator(p)) => Some((centered(area, 64, 16), p.draw(f))),
            Some(Popup::Text { title, input, .. }) => {
                let r = centered(area, 50, 4);
                f.render_widget(Clear, r);
                let hint = "[Enter] OK  [Esc] Cancel";
                f.render_widget(
                    Paragraph::new(vec![
                        Line::from(format!("{input}█")),
                        Line::styled(hint, Style::new().fg(Color::Gray)),
                    ])
                    .block(Block::default().borders(Borders::ALL).title(title.as_str())),
                    r,
                );
                let row = r.y + 2;
                Some((
                    r,
                    vec![
                        (Rect::new(r.x + 1, row, 10, 1), Click::Key(KeyCode::Enter)),
                        (Rect::new(r.x + 13, row, 14, 1), Click::Key(KeyCode::Esc)),
                    ],
                ))
            }
            Some(Popup::Help) => {
                let r = centered(area, 66, 24);
                f.render_widget(Clear, r);
                f.render_widget(
                    Paragraph::new(HELP)
                        .wrap(Wrap { trim: false })
                        .block(Block::default().borders(Borders::ALL).title("Keys")),
                    r,
                );
                self.hits.clear();
                self.hits.push(area, Target::Dismiss);
                None
            }
            None => None,
        };
        if let Some((rect, clicks)) = popup_hits {
            self.hits.clear();
            self.hits.push(area, Target::Outside);
            self.hits.push(rect, Target::Inert);
            for (r, c) in clicks {
                self.hits.push(r, Target::Popup(c));
            }
        }
    }

    fn draw_segments(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .config
            .segments
            .iter()
            .map(|s| {
                let (mark, style) = if s.enabled {
                    ("[✓]", Style::new().fg(Color::Green))
                } else {
                    ("[ ]", Style::new().fg(Color::DarkGray))
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{mark} "), style),
                    Span::styled(
                        s.id.name(),
                        if s.enabled {
                            Style::new()
                        } else {
                            Style::new().fg(Color::DarkGray)
                        },
                    ),
                ]))
            })
            .collect();
        let n = items.len();
        let mut state = ListState::default().with_selected(Some(self.seg));
        let active = self.panel == Panel::Segments;
        let dragging = self.press.as_ref().is_some_and(|p| p.dragging);
        let (symbol, highlight) = if dragging {
            (
                "⇕ ",
                Style::new()
                    .bg(Color::Rgb(40, 60, 90))
                    .add_modifier(Modifier::BOLD),
            )
        } else if active {
            ("▶ ", Style::new().add_modifier(Modifier::BOLD))
        } else {
            ("  ", Style::new().add_modifier(Modifier::BOLD))
        };
        let mut block = panel_block("Segments", active);
        if active {
            block = block.title_bottom(
                Line::styled(" drag to reorder ", Style::new().fg(Color::DarkGray)).right_aligned(),
            );
        }
        f.render_stateful_widget(
            List::new(items)
                .highlight_symbol(symbol)
                .highlight_style(highlight)
                .block(block),
            area,
            &mut state,
        );
        let inner = inner_rect(area);
        self.seg_list = Some((inner, state.offset()));
        // the highlight symbol takes 2 columns, then "[✓]"
        for (i, row) in (state.offset()..n).zip(inner.y..inner.bottom()) {
            self.hits
                .push(Rect::new(inner.x, row, inner.width, 1), Target::Segment(i));
            self.hits.push(
                Rect::new(inner.x + 2, row, 3.min(inner.width.saturating_sub(2)), 1),
                Target::SegmentCheck(i),
            );
        }
        // a click on the panel's title switches to it
        self.hits.push(
            Rect::new(area.x, area.y, area.width, 1),
            Target::Key(KeyEvent::from(KeyCode::Tab)).filter_panel(self.panel, Panel::Segments),
        );
    }

    fn draw_settings(&mut self, f: &mut Frame, area: Rect) {
        let Some(seg) = self.current() else { return };
        let palette = self.ctx.palette;
        let icon = match self.config.style.mode {
            StyleMode::Plain => &seg.icon.plain,
            _ => &seg.icon.nerd_font,
        };
        let swatch = |c: &Option<AnsiColor>| -> Vec<Span<'static>> {
            let mut v = Vec::new();
            if let Some(col) = c.as_ref().and_then(|c| to_ratatui(c, &palette)) {
                v.push(Span::styled("██ ", Style::new().fg(col)));
            }
            v.push(Span::raw(color_desc(c)));
            v
        };
        let row = |label: &str, mut value: Vec<Span<'static>>| {
            let mut spans = vec![Span::styled(
                format!("{label:<16}"),
                Style::new().fg(Color::Gray),
            )];
            spans.append(&mut value);
            ListItem::new(Line::from(spans))
        };
        let on = |b: bool| {
            vec![Span::styled(
                if b { "✓ on" } else { "✗ off" },
                Style::new().fg(if b { Color::Green } else { Color::DarkGray }),
            )]
        };
        let fields = self.fields();
        let n_fields = fields.len();
        let mut items: Vec<ListItem> = fields
            .into_iter()
            .map(|field| match field {
                Field::Enabled => row("Enabled", on(seg.enabled)),
                Field::Icon => row(
                    "Icon",
                    vec![Span::raw(if icon.is_empty() {
                        "(none)".to_string()
                    } else {
                        format!(
                            "{icon}  ({})",
                            if self.config.style.mode == StyleMode::Plain {
                                "plain"
                            } else {
                                "nerd font"
                            }
                        )
                    })],
                ),
                Field::IconColor => row("Icon Color", swatch(&seg.colors.icon)),
                Field::TextColor => row("Text Color", swatch(&seg.colors.text)),
                Field::BgColor => row("Background", swatch(&seg.colors.background)),
                Field::Bold => row("Text Bold", on(seg.styles.text_bold)),
                Field::Opt(k) => {
                    let value = match seg.options.get(&k) {
                        Some(toml::Value::Boolean(b)) => on(*b),
                        Some(v) => vec![Span::raw(v.to_string())],
                        None => vec![],
                    };
                    row(&format!("· {k}"), value)
                }
            })
            .collect();
        items.push(ListItem::new(""));
        // wrap the description to the panel (List items don't wrap)
        let wrap_at = area.width.saturating_sub(6).max(20) as usize;
        let words: Vec<String> = segment_help(seg.id).split(' ').map(String::from).collect();
        for line in wrap_items(&words, wrap_at) {
            let line = line.replace("  ", " ");
            items.push(ListItem::new(Line::styled(
                line,
                Style::new().fg(Color::DarkGray),
            )));
        }
        let mut state = ListState::default().with_selected(Some(self.field));
        let active = self.panel == Panel::Settings;
        let title = format!("{} Settings", seg.id.name());
        f.render_stateful_widget(
            List::new(items)
                .highlight_symbol(if active { "▶ " } else { "  " })
                .highlight_style(Style::new().add_modifier(Modifier::BOLD))
                .block(panel_block(&title, active)),
            area,
            &mut state,
        );
        let inner = inner_rect(area);
        for (i, row) in (state.offset()..n_fields).zip(inner.y..inner.bottom()) {
            self.hits
                .push(Rect::new(inner.x, row, inner.width, 1), Target::Field(i));
        }
        self.hits.push(
            Rect::new(area.x, area.y, area.width, 1),
            Target::Key(KeyEvent::from(KeyCode::Tab)).filter_panel(self.panel, Panel::Settings),
        );
    }
}

fn panel_block(title: &str, active: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(if active {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new()
        })
}

fn inner_rect(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

impl Target {
    /// A panel-title click switches panels only when that panel is not
    /// already active; otherwise it does nothing.
    fn filter_panel(self, current: Panel, own: Panel) -> Target {
        if current == own {
            Target::Inert
        } else {
            self
        }
    }
}

/// Theme bar lines plus each theme's (line, x, width) for click targets.
fn theme_bar(
    list: &[String],
    current: &str,
    width: usize,
) -> (Vec<Line<'static>>, Vec<(u16, u16, u16)>) {
    let labels: Vec<String> = list
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = if t == current { "[✓]" } else { "[ ]" };
            if i < 9 {
                format!("{} {mark} {t}", i + 1)
            } else {
                format!("{mark} {t}")
            }
        })
        .collect();
    let widths: Vec<usize> = labels.iter().map(|l| l.chars().count()).collect();
    let pos = super::flow(&widths, width);
    let lines = theme_bar_lines(list, current, width);
    let hits = pos
        .iter()
        .zip(&widths)
        .map(|(&(l, x), &w)| (l, x, w as u16))
        .collect();
    (lines, hits)
}

fn theme_bar_lines(list: &[String], current: &str, width: usize) -> Vec<Line<'static>> {
    let items: Vec<String> = list
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = if t == current { "[✓]" } else { "[ ]" };
            if i < 9 {
                format!("{} {mark} {t}", i + 1)
            } else {
                format!("{mark} {t}")
            }
        })
        .collect();
    wrap_items(&items, width)
        .into_iter()
        .map(|l| {
            // highlight the current theme
            let needle = format!("[✓] {current}");
            match l.find(&needle) {
                Some(i) => Line::from(vec![
                    Span::raw(l[..i].to_string()),
                    Span::styled(
                        needle.clone(),
                        Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(l[i + needle.len()..].to_string()),
                ]),
                None => Line::from(l),
            }
        })
        .collect()
}

fn wrap_items(items: &[String], width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for item in items {
        let line = lines.last_mut().expect("line");
        let need = item.chars().count() + if line.is_empty() { 0 } else { 2 };
        if !line.is_empty() && line.chars().count() + need > width {
            lines.push(item.clone());
        } else {
            if !line.is_empty() {
                line.push_str("  ");
            }
            line.push_str(item);
        }
    }
    lines
}

/// The help bar; every entry with an action is also a clickable button.
fn help_buttons(panel: Panel) -> Vec<Button> {
    let k = |c: char| Some(KeyEvent::from(KeyCode::Char(c)));
    let mut v = vec![
        button("Tab", "Switch Panel", Some(KeyEvent::from(KeyCode::Tab))),
        button("↑↓", "Navigate", None),
    ];
    match panel {
        Panel::Segments => v.extend([
            button("Enter", "Show/Hide", Some(KeyEvent::from(KeyCode::Enter))),
            button("K", "Move Up", k('K')),
            button("J", "Move Down", k('J')),
            button("Drag", "Reorder", None),
        ]),
        Panel::Settings => v.extend([
            button("Enter", "Edit", Some(KeyEvent::from(KeyCode::Enter))),
            button("←→", "Toggle/Adjust", Some(KeyEvent::from(KeyCode::Right))),
        ]),
    }
    v.extend([
        button("P", "Next Theme", k('p')),
        button("M", "Style Mode", k('m')),
        button("E", "Separator", k('e')),
        button("L", "Language", k('l')),
        button("C", "Colors", k('c')),
        button("R", "Reset", k('r')),
        button("S", "Save", k('s')),
        button("W", "Write Theme", k('w')),
        button(
            "Ctrl+S",
            "Save As Theme",
            Some(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
        ),
        button("?", "Help", k('?')),
        button("Esc", "Quit", Some(KeyEvent::from(KeyCode::Esc))),
    ]);
    v
}

fn segment_help(id: SegmentId) -> &'static str {
    match id {
        SegmentId::Mode => "Permission mode, plan, swarm and tower badges.",
        SegmentId::Goal => "Live /goal badge: status, elapsed time, turns.",
        SegmentId::Model => "Model + thinking effort; dance: rainbow after /dance.",
        SegmentId::Tasks => "Running background shell tasks and agents.",
        SegmentId::Directory => "Working directory; depth: path segments kept.",
        SegmentId::Git => "Branch, diff stats, ahead/behind; pr: open PR via gh.",
        SegmentId::Context => "Context window fill, colored by pressure.",
        SegmentId::Usage => "Whole-session input ↑, output ↓ and cache hit rate.",
        SegmentId::Subagent => "Usage of the heaviest sub-agent model.",
        SegmentId::Session => "Time since the session was created.",
        SegmentId::Quota => "Plan quota: 5h / 7d (and monthly) used % with reset time. Needs a Kimi Code OAuth login; refreshed in the background every refresh_secs.",
    }
}

const HELP: &str = "\
 Segments panel
   ↑ ↓ / j k        select segment
   Enter / Space    show / hide the segment
   Shift+↑↓ / J K   move it left / right in the line

 Settings panel
   Enter            edit: color & icon pickers, option values
   ← →              flip switches, step numbers

 Mouse
   click            select a segment / setting / theme
   click again      toggle the segment, or edit the setting
   click [✓]        show / hide a segment directly
   drag a segment   move it to a new position (live preview)
   wheel            scroll the list under the pointer
   bottom bar, mode/sep/lang/colors in the preview border are buttons

 Anywhere
   1-9 / P          pick / cycle theme      R  reset theme
   M                plain → nerd_font → powerline
   E                separator               L  language en/zh
   C                colors: tui.toml → dark → light
   S                save config.toml        W  write current theme
   Ctrl+S           save as a new theme     Esc quit

 Press any key or click to close";

fn strip_osc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("\x1b]") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail
            .find('\x07')
            .map(|j| j + 1)
            .or_else(|| tail.find("\x1b\\").map(|j| j + 2))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

pub fn run_configurator() -> Result<(), String> {
    let mut terminal = super::init();
    let result = App::new().run(&mut terminal);
    super::restore();
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_hits_match_drawn_text() {
        let list: Vec<String> = themes::BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
        for width in [40, 80, 132, 200] {
            let (lines, pos) = theme_bar(&list, "nord", width);
            let text: Vec<String> = lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();
            for (name, (line, x, w)) in list.iter().zip(pos) {
                let drawn: String = text[line as usize]
                    .chars()
                    .skip(x as usize)
                    .take(w as usize)
                    .collect();
                assert!(
                    drawn.ends_with(name.as_str()),
                    "{width}: {drawn:?} vs {name}"
                );
            }
        }
    }

    #[test]
    fn osc_is_stripped() {
        assert_eq!(strip_osc("a\x1b]8;;https://x\x07PR\x1b]8;;\x07b"), "aPRb");
    }

    #[test]
    fn items_wrap() {
        let items: Vec<String> = ["aaaa", "bbbb", "cccc"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(wrap_items(&items, 10), vec!["aaaa  bbbb", "cccc"]);
    }
}
