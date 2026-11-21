pub mod config_view;
pub mod debug_overlay;
pub mod file_browser;
pub mod info_popup;
pub mod search_view;
pub mod settings_view;
pub mod theme;

use ratatui::Frame;

use crate::app::{App, Screen};

pub fn render(f: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Search => search_view::render(f, app),
        Screen::AddLocation { .. } if app.return_to_popup => search_view::render(f, app),
        Screen::Config | Screen::AddLocation { .. } => config_view::render(f, app),
        Screen::Settings => settings_view::render(f, app),
    }
    if app.show_debug_overlay {
        debug_overlay::render(f, f.area(), app);
    }
}
