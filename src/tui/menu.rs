use super::{button, button_bar, Hits};
use crate::config::{config_path, Config};
use crate::{install, quota, themes};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

/// The project page, from Cargo.toml's `repository`.
pub const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const ISSUES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues");

pub enum Action {
    Stay,
    Quit,
    Configure,
}

#[derive(Clone)]
enum Target {
    Item(usize),
    Key(KeyEvent),
    /// open this URL in the browser
    Url(&'static str),
}

pub struct Menu {
    selected: usize,
    status: Option<(String, bool)>,
    about: bool,
    hits: Hits<Target>,
}

impl Default for Menu {
    fn default() -> Self {
        Menu {
            selected: 0,
            status: None,
            about: false,
            hits: Hits::new(),
        }
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
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

    /// None when the event changes nothing (no redraw needed).
    pub fn mouse(&mut self, m: MouseEvent) -> Option<Action> {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {}
            MouseEventKind::ScrollUp => return Some(self.key(key(KeyCode::Up))),
            MouseEventKind::ScrollDown => return Some(self.key(key(KeyCode::Down))),
            _ => return None,
        }
        if self.about {
            // links stay clickable in the About box; anywhere else closes it
            if let Some(Target::Url(url)) = self.hits.at(m.column, m.row) {
                self.open_url(url);
            } else {
                self.about = false;
            }
            return Some(Action::Stay);
        }
        match self.hits.at(m.column, m.row)? {
            // first click selects, a click on the selected item runs it
            Target::Item(i) if i == self.selected => Some(self.key(key(KeyCode::Enter))),
            Target::Item(i) => {
                self.selected = i;
                self.status = None;
                Some(Action::Stay)
            }
            Target::Key(k) => Some(self.key(k)),
            Target::Url(url) => {
                self.open_url(url);
                Some(Action::Stay)
            }
        }
    }

    fn open_url(&mut self, url: &str) {
        match open_in_browser(url) {
            Ok(()) => self.ok(format!("Opened {url}")),
            Err(e) => self.err(format!("Could not open a browser ({e}); visit {url}")),
        }
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
        self.hits.clear();
        let buttons = [
            button("↑↓", "Navigate", None),
            button("Enter", "Select", Some(key(KeyCode::Enter))),
            button("Esc", "Exit", Some(key(KeyCode::Esc))),
            button("Click", "select, click again to run", None),
        ];
        let inner_w = f.area().width.saturating_sub(2);
        let bar_rows = button_bar(&buttons, Rect::new(0, 0, inner_w, u16::MAX))
            .0
            .len() as u16;
        // reserve the status row up front so buttons never shift under the pointer
        let footer = bar_rows + 3;
        let [header, body, foot] = Layout::vertical([
            Constraint::Length(6),
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
                Line::styled(
                    REPO,
                    Style::new()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::UNDERLINED),
                ),
            ])
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title("Welcome")),
            header,
        );
        // Paragraph centers within the inner width (borders excluded)
        let repo_w = (REPO.len() as u16).min(header.width.saturating_sub(2));
        self.hits.push(
            Rect::new(
                header.x + 1 + (header.width.saturating_sub(2) - repo_w) / 2,
                header.y + 4,
                repo_w,
                1,
            ),
            Target::Url(REPO),
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
        let inner = Rect::new(
            body.x + 1,
            body.y + 1,
            body.width.saturating_sub(2),
            body.height.saturating_sub(2),
        );
        let offset = state.offset();
        for (i, row) in (offset..ITEMS.len()).zip(inner.y..inner.bottom()) {
            self.hits
                .push(Rect::new(inner.x, row, inner.width, 1), Target::Item(i));
        }

        let foot_inner = Rect::new(
            foot.x + 1,
            foot.y + 1,
            foot.width.saturating_sub(2),
            bar_rows,
        );
        let (bar, bar_hits) = button_bar(&buttons, foot_inner);
        for (r, k) in bar_hits {
            self.hits.push(r, Target::Key(k));
        }
        let mut lines = bar;
        if let Some((msg, is_err)) = &self.status {
            lines.push(Line::styled(
                msg.as_str(),
                Style::new().fg(if *is_err { Color::Red } else { Color::Green }),
            ));
        }
        // no wrapping: the button bar is laid out by button_bar() already,
        // and the hit boxes must match what is drawn
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
            foot,
        );

        if self.about {
            let link = |url: &'static str| {
                Span::styled(
                    url,
                    Style::new()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::UNDERLINED),
                )
            };
            let label = |t: &'static str| Span::styled(t, Style::new().fg(Color::Gray));
            let lines = vec![
                Line::styled(
                    format!("kimi-statusline v{}", env!("CARGO_PKG_VERSION")),
                    Style::new().fg(Color::Cyan),
                ),
                Line::from(""),
                Line::from("Status line for Kimi Code CLI: session tokens, cache"),
                Line::from("hit rate, sub-agents, 5h / 7d quota, git + PR, themes."),
                Line::from(""),
                Line::from(vec![label("GitHub: "), link(REPO)]),
                Line::from(vec![label("Issues: "), link(ISSUES)]),
                Line::from(""),
                Line::from(format!("Config: {}", config_path().display())),
                Line::from(""),
                Line::styled(
                    "Click a link to open it · any key or click elsewhere closes",
                    Style::new().fg(Color::Gray),
                ),
            ];
            // link rows come from the lines themselves, so editing the text
            // above can't desync the click targets (no wrapping: one line
            // per row)
            let rows: Vec<(u16, &'static str)> = lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| {
                    let url = [REPO, ISSUES]
                        .into_iter()
                        .find(|u| l.spans.iter().any(|s| s.content == *u))?;
                    Some((i as u16, url))
                })
                .collect();
            let area = super::pickers::centered(f.area(), 76, lines.len() as u16 + 2);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("About")),
                area,
            );
            // after the border, "GitHub: " / "Issues: " take 8 columns
            let room = area.width.saturating_sub(10);
            for (row, url) in rows {
                let w = (url.len() as u16).min(room);
                self.hits.push(
                    Rect::new(area.x + 9, area.y + 1 + row, w, 1),
                    Target::Url(url),
                );
            }
        }
    }
}

/// Open a URL with the platform's default handler.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        Command::new("xdg-open")
    };
    cmd.arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}
