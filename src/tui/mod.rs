//! Interactive UI, modeled on CCometixLine's: a main menu, and a
//! configurator with live preview, theme bar, segment list, settings panel
//! and popup pickers for colors, icons and the separator.

mod app;
mod menu;
mod pickers;

use crossterm::event::{self, Event, KeyEventKind};

pub use app::run_configurator;

/// Main menu first (what running the binary in a terminal shows).
pub fn run_menu() -> Result<(), String> {
    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        let mut menu = menu::Menu::default();
        loop {
            terminal.draw(|f| menu.draw(f))?;
            let Event::Key(k) = event::read()? else {
                continue;
            };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match menu.key(k) {
                menu::Action::Stay => {}
                menu::Action::Quit => return Ok(()),
                menu::Action::Configure => {
                    app::App::new().run(&mut terminal)?;
                }
            }
        }
    })();
    ratatui::restore();
    result.map_err(|e| e.to_string())
}
