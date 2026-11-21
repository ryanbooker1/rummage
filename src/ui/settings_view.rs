use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::ui::theme::Theme;

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(5),    // settings body
            Constraint::Length(3), // keybindings
        ])
        .split(area);

    render_header(f, chunks[0]);
    render_body(f, chunks[1], app);
    render_keybindings(f, chunks[2]);
}

fn render_header(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::primary());

    let inner = block.inner(area);
    f.render_widget(block, area);

    let header = Line::from(vec![
        Span::styled("  Settings", Theme::title()),
        Span::raw("                                          "),
        Span::styled("[Tab/Esc] Back", Theme::dim()),
        Span::raw("  "),
    ]);
    f.render_widget(Paragraph::new(header), inner);
}

fn render_body(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::primary());

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 4 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // section label: Security
            Constraint::Length(1), // separator
            Constraint::Length(1), // toggle row 0
            Constraint::Length(1), // description 0
            Constraint::Length(1), // blank
            Constraint::Length(1), // section label: Search
            Constraint::Length(1), // separator
            Constraint::Length(1), // toggle row 1
            Constraint::Length(1), // description 1
            Constraint::Min(0),    // rest
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled("  Security", Theme::header())),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            " ────────────────────────────────────────────────── ",
            Theme::dim(),
        )),
        chunks[1],
    );

    // Toggle row 0: require_unlock_on_startup
    let sel0 = app.settings_popup_selected == 0;
    let row0_style = if sel0 { Theme::selected() } else { Theme::normal() };
    let require_unlock = app.config.settings.require_unlock_on_startup;
    let val0 = if require_unlock {
        Span::styled("[ON] ", Theme::on())
    } else {
        Span::styled("[OFF]", Theme::off())
    };
    let row0_line = Line::from(vec![
        Span::styled(if sel0 { " ▶ " } else { "   " }, row0_style),
        Span::styled("Require credential unlock on startup  ", row0_style),
        val0,
    ]);
    f.render_widget(Paragraph::new(row0_line), chunks[2]);

    f.render_widget(
        Paragraph::new(Span::styled(
            "   Uses your sudo password to protect SSH passwords",
            Theme::dim(),
        )),
        chunks[3],
    );

    // blank row
    f.render_widget(Paragraph::new(""), chunks[4]);

    f.render_widget(
        Paragraph::new(Span::styled("  Search", Theme::header())),
        chunks[5],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            " ────────────────────────────────────────────────── ",
            Theme::dim(),
        )),
        chunks[6],
    );

    // Toggle row 1: consecutive_search
    let sel1 = app.settings_popup_selected == 1;
    let row1_style = if sel1 { Theme::selected() } else { Theme::normal() };
    let consecutive = app.config.settings.consecutive_search;
    let val1 = if consecutive {
        Span::styled("[ON] ", Theme::on())
    } else {
        Span::styled("[OFF]", Theme::off())
    };
    let row1_line = Line::from(vec![
        Span::styled(if sel1 { " ▶ " } else { "   " }, row1_style),
        Span::styled("Consecutive matching (no fuzzy)       ", row1_style),
        val1,
    ]);
    f.render_widget(Paragraph::new(row1_line), chunks[7]);

    f.render_widget(
        Paragraph::new(Span::styled(
            "   Require letters to appear consecutively",
            Theme::dim(),
        )),
        chunks[8],
    );
}

fn render_keybindings(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::dim());

    let inner = block.inner(area);
    f.render_widget(block, area);

    let line = Line::from(vec![
        Span::styled("Space", Theme::primary()),
        Span::raw(" Toggle  │  "),
        Span::styled("Tab", Theme::primary()),
        Span::raw("/"),
        Span::styled("Esc", Theme::primary()),
        Span::raw(" Back"),
    ]);

    f.render_widget(Paragraph::new(line), inner);
}
