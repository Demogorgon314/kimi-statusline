use crate::config::{config_path, Config};
use crate::{install, quota, themes};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

pub enum Action {
    Stay,
    Quit,
    Configure,
}

#[derive(Default)]
pub struct Menu {
    selected: usize,
    status: Option<(String, bool)>,
    about: bool,
}

const ITEMS: [(&str, &str); 8] = [
    (
        "Configuration Mode",
        "Edit segments, colors, icons and themes",
    ),
    ("Initialize Config", "Write config.toml from the kimi theme"),
    ("Check Configuration", "Validate config.toml"),
    (
        "Install to Kimi Code",
        "Set [status_line].command in tui.toml",
    ),
    (
        "Uninstall from Kimi Code",
        "Remove our [status_line].command",
    ),
    ("Test Quota", "Fetch 5h / 7d plan quota now"),
    ("About", "Show application information"),
    ("Exit", "Leave kimi-statusline"),
];

impl Menu {
    fn ok(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), false));
    }

    fn err(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), true));
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        if self.about {
            self.about = false;
            return Action::Stay;
        }
        self.status = None;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::Quit,
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(ITEMS.len() - 1)
            }
            KeyCode::Enter => return self.select(),
            _ => {}
        }
        Action::Stay
    }

    fn select(&mut self) -> Action {
        match self.selected {
            0 => return Action::Configure,
            1 => {
                let path = config_path();
                if path.exists() {
                    self.ok(format!("Config already exists at {}", path.display()));
                } else {
                    match themes::get("kimi").save() {
                        Ok(()) => self.ok(format!("✓ Created {}", path.display())),
                        Err(e) => self.err(format!("✗ {e}")),
                    }
                }
            }
            2 => match Config::try_load() {
                Ok(c) => self.ok(format!(
                    "✓ Valid: theme {}, {} of {} segments enabled",
                    c.theme,
                    c.segments.iter().filter(|s| s.enabled).count(),
                    c.segments.len()
                )),
                Err(e) if e.is_empty() => self.ok("No config.toml yet: the kimi theme is used"),
                Err(e) => self.err(format!("✗ Invalid config.toml: {e}")),
            },
            3 => match install::install(None, false, false) {
                Ok(o) if o.changed => self.ok("✓ Installed. Run /reload-tui in Kimi Code"),
                Ok(_) => self.ok("Already installed"),
                Err(e) => self.err(format!("✗ {e}")),
            },
            4 => match install::uninstall() {
                Ok(true) => self.ok("✓ Removed. Run /reload-tui in Kimi Code"),
                Ok(false) => self.ok("Nothing to remove"),
                Err(e) => self.err(format!("✗ {e}")),
            },
            5 => match quota::fetch_and_store() {
                Ok(q) => {
                    let f = |e: &Option<quota::Entry>| {
                        e.as_ref()
                            .map_or("-".into(), |e| format!("{:.0}%", e.used_ratio * 100.0))
                    };
                    self.ok(format!(
                        "✓ 5h {}  ·  7d {}  ·  monthly {}",
                        f(&q.limit_5h),
                        f(&q.limit_7d),
                        f(&q.month)
                    ))
                }
                Err(e) => self.err(format!("✗ {e}")),
            },
            6 => self.about = true,
            _ => return Action::Quit,
        }
        Action::Stay
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let footer = if self.status.is_some() { 4 } else { 3 };
        let [header, body, foot] = Layout::vertical([
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(footer),
        ])
        .areas(f.area());

        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        "kimi-statusline",
                        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" v", Style::new().fg(Color::Gray)),
                    Span::styled(env!("CARGO_PKG_VERSION"), Style::new().fg(Color::Yellow)),
                ]),
                Line::from(""),
                Line::styled(
                    "High-performance Kimi Code status line",
                    Style::new().fg(Color::Gray),
                ),
            ])
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title("Welcome")),
            header,
        );

        let items: Vec<ListItem> = ITEMS
            .iter()
            .map(|(t, d)| {
                ListItem::new(Line::from(vec![
                    Span::raw(format!(" {t}")),
                    Span::styled(format!(" - {d}"), Style::new().fg(Color::Gray)),
                ]))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.selected));
        f.render_stateful_widget(
            List::new(items)
                .highlight_style(Style::new().bg(Color::Cyan).fg(Color::Black))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Main Menu")
                        .title_style(Style::new().fg(Color::Green)),
                ),
            body,
            &mut state,
        );

        let mut lines = vec![Line::styled(
            "[↑↓] Navigate  [Enter] Select  [Esc] Exit",
            Style::new().fg(Color::Gray),
        )];
        if let Some((msg, is_err)) = &self.status {
            lines.push(Line::styled(
                msg.as_str(),
                Style::new().fg(if *is_err { Color::Red } else { Color::Green }),
            ));
        }
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL)),
            foot,
        );

        if self.about {
            let area = super::pickers::centered(f.area(), 64, 12);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        format!("kimi-statusline v{}", env!("CARGO_PKG_VERSION")),
                        Style::new().fg(Color::Cyan),
                    ),
                    Line::from(""),
                    Line::from("Status line for Kimi Code CLI: session tokens, cache"),
                    Line::from("hit rate, sub-agents, 5h / 7d quota, git + PR, themes."),
                    Line::from(""),
                    Line::from(format!("Config: {}", config_path().display())),
                    Line::from(""),
                    Line::styled("Press any key to close", Style::new().fg(Color::Gray)),
                ])
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title("About")),
                area,
            );
        }
    }
}
