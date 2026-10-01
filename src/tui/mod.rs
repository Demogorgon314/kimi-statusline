//! Interactive UI, modeled on CCometixLine's: a main menu, and a
//! configurator with live preview, theme bar, segment list, settings panel
//! and popup pickers for colors, icons and the separator.
//!
//! Everything works with the keyboard and the mouse. Each frame records the
//! screen rectangle of every clickable thing (a "hit map"); mouse events are
//! resolved against the map of the frame the user is looking at.

mod app;
mod menu;
mod pickers;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEvent, KeyEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::DefaultTerminal;
use unicode_width::UnicodeWidthStr;

pub use app::run_configurator;

/// Enter the alternate screen with mouse reporting on. The panic hook turns
/// mouse reporting off again, or the user's shell would keep receiving
/// escape codes for every click.
pub fn init() -> DefaultTerminal {
    let terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        prev(info);
    }));
    terminal
}

pub fn restore() {
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

/// Clickable regions of one frame; later entries sit on top.
pub struct Hits<T>(Vec<(Rect, T)>);

impl<T: Clone> Hits<T> {
    pub fn new() -> Self {
        Hits(Vec::new())
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn push(&mut self, rect: Rect, target: T) {
        self.0.push((rect, target));
    }

    pub fn at(&self, column: u16, row: u16) -> Option<T> {
        let pos = Position::new(column, row);
        self.0
            .iter()
            .rev()
            .find(|(r, _)| r.contains(pos))
            .map(|(_, t)| t.clone())
    }
}

/// Lay items of the given widths left to right, two columns apart, wrapping
/// at `width`. Returns each item's (line, x).
pub fn flow(widths: &[usize], width: usize) -> Vec<(u16, u16)> {
    let (mut line, mut x) = (0u16, 0usize);
    widths
        .iter()
        .map(|&w| {
            if x > 0 && x + 2 + w > width {
                line += 1;
                x = 0;
            } else if x > 0 {
                x += 2;
            }
            let at = (line, x as u16);
            x += w;
            at
        })
        .collect()
}

/// A help-bar entry, `[key] label`. Entries with an action are clickable
/// and send that key, so a click does exactly what the shortcut does.
pub struct Button {
    pub key: &'static str,
    pub label: &'static str,
    pub action: Option<KeyEvent>,
}

pub fn button(key: &'static str, label: &'static str, action: Option<KeyEvent>) -> Button {
    Button { key, label, action }
}

/// Render buttons into lines for a paragraph drawn at `area`, and the
/// clickable rectangles of the ones that have an action.
pub fn button_bar(buttons: &[Button], area: Rect) -> (Vec<Line<'static>>, Vec<(Rect, KeyEvent)>) {
    let texts: Vec<String> = buttons
        .iter()
        .map(|b| format!("[{}] {}", b.key, b.label))
        .collect();
    let widths: Vec<usize> = texts.iter().map(|t| t.width()).collect();
    let pos = flow(&widths, area.width as usize);
    let lines_n = pos.last().map_or(1, |(l, _)| *l as usize + 1);
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new(); lines_n];
    let mut cursor = vec![0u16; lines_n];
    let mut hits = Vec::new();
    for (i, b) in buttons.iter().enumerate() {
        let (l, x) = pos[i];
        let line = &mut lines[l as usize];
        let gap = x - cursor[l as usize];
        if gap > 0 {
            line.push(Span::raw(" ".repeat(gap as usize)));
        }
        let (key_style, label_style) = if b.action.is_some() {
            (Style::new().fg(Color::Cyan), Style::new().fg(Color::Gray))
        } else {
            (
                Style::new().fg(Color::DarkGray),
                Style::new().fg(Color::DarkGray),
            )
        };
        line.push(Span::styled(format!("[{}]", b.key), key_style));
        line.push(Span::styled(format!(" {}", b.label), label_style));
        cursor[l as usize] = x + widths[i] as u16;
        if let Some(k) = b.action {
            if area.y + l < area.bottom() {
                let w = (widths[i] as u16).min(area.width.saturating_sub(x));
                hits.push((Rect::new(area.x + x, area.y + l, w, 1), k));
            }
        }
    }
    (lines.into_iter().map(Line::from).collect(), hits)
}

/// Main menu first (what running the binary in a terminal shows).
pub fn run_menu() -> Result<(), String> {
    let mut terminal = init();
    let result = (|| -> std::io::Result<()> {
        let mut menu = menu::Menu::default();
        let mut redraw = true;
        loop {
            if menu.poll() {
                redraw = true;
            }
            if redraw {
                terminal.draw(|f| menu.draw(f))?;
            }
            // wake up now and then to pick up the background update check
            if !event::poll(std::time::Duration::from_millis(250))? {
                redraw = false;
                continue;
            }
            let action = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => Some(menu.key(k)),
                Event::Mouse(m) => menu.mouse(m),
                Event::Resize(..) => Some(menu::Action::Stay),
                _ => None,
            };
            redraw = action.is_some();
            match action {
                Some(menu::Action::Quit) => return Ok(()),
                Some(menu::Action::Configure) => app::App::new().run(&mut terminal)?,
                _ => {}
            }
        }
    })();
    restore();
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn flow_wraps() {
        assert_eq!(flow(&[4, 4, 4], 10), vec![(0, 0), (0, 6), (1, 0)]);
    }

    #[test]
    fn buttons_are_clickable_where_drawn() {
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let bar = [button("↑↓", "Move", None), button("Esc", "Quit", Some(esc))];
        let (lines, hits) = button_bar(&bar, Rect::new(5, 3, 80, 2));
        assert_eq!(lines.len(), 1);
        // "[↑↓] Move" is 9 columns, then two spaces
        assert_eq!(hits, vec![(Rect::new(5 + 11, 3, 10, 1), esc)]);
        let hit_map = {
            let mut h = Hits::new();
            for (r, k) in hits {
                h.push(r, k);
            }
            h
        };
        assert_eq!(hit_map.at(16, 3), Some(esc));
        assert_eq!(hit_map.at(15, 3), None);
    }
}
