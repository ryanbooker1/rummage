use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, InputField, Screen, StatusLevel};
use crate::ui::theme::{Theme, BG, FG, FG4, AQUA, YELLOW};

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    let constraints = vec![
        Constraint::Length(3), // header
        Constraint::Min(5),    // locations list
        Constraint::Length(4), // keybindings (2 borders + 2 inner rows)
    ];

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_header(f, chunks[0]);
    render_locations(f, chunks[1], app);
    render_keybindings(f, chunks[2]);

    // Add/Edit modal overlay
    if let Screen::AddLocation { .. } = &app.screen {
        render_location_form(f, area, app);
    }

    // Delete confirmation overlay
    if app.delete_confirm.is_some() {
        render_delete_confirm(f, area, app);
    }
}

fn render_header(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::primary());

    let inner = block.inner(area);
    f.render_widget(block, area);

    let header = Line::from(vec![
        Span::styled("  Configuration", Theme::title()),
        Span::raw("                                    "),
        Span::styled("[Tab] Back", Theme::dim()),
        Span::raw("  "),
    ]);
    f.render_widget(Paragraph::new(header), inner);
}

fn render_locations(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::primary())
        .title(Span::styled(" Locations ", Theme::primary()));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.config.locations.is_empty() {
        let msg = Paragraph::new(Span::styled(
            "No locations configured. Press 'a' to add one.",
            Theme::dim(),
        ));
        f.render_widget(msg, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .config
        .locations
        .iter()
        .enumerate()
        .map(|(i, loc)| {
            let is_selected = i == app.config_selected;
            let prefix = if is_selected { "▶ " } else { "  " };

            let enabled_marker = if loc.enabled {
                Span::styled("[✓]", Theme::enabled())
            } else {
                Span::styled("[ ]", Theme::disabled())
            };

            let name_style = if is_selected {
                Theme::selected()
            } else {
                Theme::normal()
            };

            let name_display = format!("{} ", loc.name);
            let path_display = loc.path.clone();
            let kind_display = if loc.kind == "ssh" {
                let host = loc.ssh_host.as_deref().unwrap_or("unknown");
                let user = loc.ssh_user.as_deref().unwrap_or("user");
                format!("(ssh: {}@{}:{})", user, host, loc.ssh_port)
            } else {
                "(local)".to_string()
            };

            let line = Line::from(vec![
                Span::styled(prefix, name_style),
                enabled_marker,
                Span::raw(" "),
                Span::styled(name_display, name_style),
                Span::styled(path_display, Theme::dim()),
                Span::raw("  "),
                Span::styled(kind_display, Theme::dim()),
            ]);

            ListItem::new(line)
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.config_selected));

    let list = List::new(items)
        .highlight_style(Theme::selected())
        .highlight_symbol("");

    f.render_stateful_widget(list, inner, &mut state);
}

fn render_keybindings(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::dim());

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    let line1 = Line::from(vec![
        Span::styled("↑↓", Theme::primary()),
        Span::raw(" Navigate  │ "),
        Span::styled("a", Theme::primary()),
        Span::raw(" Add  │ "),
        Span::styled("e", Theme::primary()),
        Span::raw("/"),
        Span::styled("Enter", Theme::primary()),
        Span::raw(" Edit  │ "),
        Span::styled("d", Theme::primary()),
        Span::raw(" Delete  │ "),
        Span::styled("Space", Theme::primary()),
        Span::raw(" Toggle"),
    ]);

    let line2 = Line::from(vec![
        Span::styled("Tab", Theme::primary()),
        Span::raw(" Back to Search  │ "),
        Span::styled("s", Theme::primary()),
        Span::raw(" Settings"),
    ]);

    f.render_widget(Paragraph::new(line1), chunks[0]);
    f.render_widget(Paragraph::new(line2), chunks[1]);
}

pub fn render_location_form(f: &mut Frame, area: Rect, app: &App) {
    let is_editing = matches!(&app.screen, Screen::AddLocation { editing_id: Some(_) });
    let title = if is_editing {
        " Edit Location "
    } else {
        " Add Location "
    };

    let modal_width = 60u16.min(area.width.saturating_sub(4));
    let field_count = if app.form.is_ssh { 7 } else { 3 };
    let modal_height = (field_count as u16 * 3 + 4).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(x, y, modal_width, modal_height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(Theme::primary())
        .title(Span::styled(title, Theme::primary()));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let fields_to_show: Vec<InputField> = if app.form.is_ssh {
        vec![
            InputField::Name,
            InputField::Type,
            InputField::Path,
            InputField::SshHost,
            InputField::SshPort,
            InputField::SshUser,
            InputField::SshPassword,
        ]
    } else {
        vec![InputField::Name, InputField::Type, InputField::Path]
    };

    let num_fields = fields_to_show.len();

    // Build row constraints: each field = 2 rows (label + input), plus footer
    let mut constraints: Vec<Constraint> = fields_to_show
        .iter()
        .map(|_| Constraint::Length(2))
        .collect();
    constraints.push(Constraint::Min(1)); // footer

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, field) in fields_to_show.iter().enumerate() {
        let is_focused = i == app.form.focused_field;
        let chunk = chunks[i];

        let label_style = if is_focused {
            Theme::primary()
        } else {
            Theme::dim()
        };

        let label_area = Rect::new(chunk.x, chunk.y, chunk.width, 1);
        let input_area = Rect::new(chunk.x, chunk.y + 1, chunk.width, 1);

        f.render_widget(
            Paragraph::new(Span::styled(field.label(), label_style))
                .style(Style::default().bg(BG)),
            label_area,
        );

        let input_line = if field == &InputField::Type {
            // Render as a clear toggle button
            let (local_style, ssh_style) = if app.form.is_ssh {
                (
                    Style::default().fg(FG4).bg(BG),
                    Style::default().fg(BG).bg(AQUA),
                )
            } else {
                (
                    Style::default().fg(BG).bg(AQUA),
                    Style::default().fg(FG4).bg(BG),
                )
            };
            let hint_style = if is_focused {
                Style::default().fg(YELLOW).bg(BG)
            } else {
                Style::default().fg(FG4).bg(BG)
            };
            Line::from(vec![
                Span::styled("  ", Style::default().bg(BG)),
                Span::styled(" Local ", local_style),
                Span::styled("  ", Style::default().bg(BG)),
                Span::styled(" SSH ", ssh_style),
                Span::styled("  ◄► or Space to switch", hint_style),
            ])
        } else {
            let value = if field == &InputField::SshPassword {
                "*".repeat(app.form.get_field(field).len())
            } else {
                app.form.get_field(field).to_string()
            };

            if is_focused {
                let cursor_pos = app.form.cursor_pos.min(value.chars().count());
                let before: String = value.chars().take(cursor_pos).collect();
                let at_cursor = value.chars().nth(cursor_pos);
                let cursor_char: String = at_cursor.map(|c| c.to_string()).unwrap_or_else(|| " ".to_string());
                let after: String = if at_cursor.is_some() {
                    value.chars().skip(cursor_pos + 1).collect()
                } else {
                    String::new()
                };
                Line::from(vec![
                    Span::styled("> ", Style::default().fg(FG4).bg(BG)),
                    Span::styled(before, Style::default().fg(FG).bg(BG)),
                    Span::styled(cursor_char, Theme::cursor().bg(BG)),
                    Span::styled(after, Style::default().fg(FG).bg(BG)),
                ])
            } else {
                Line::from(vec![
                    Span::styled("> ", Style::default().fg(FG4).bg(BG)),
                    Span::styled(value, Style::default().fg(FG4).bg(BG)),
                ])
            }
        };
        f.render_widget(
            Paragraph::new(input_line).style(Style::default().bg(BG)),
            input_area,
        );
    }

    // Footer
    let footer_area = chunks[num_fields];
    let gray = Style::default().fg(FG4).bg(BG);
    let mut footer_spans = vec![
        Span::styled("↑↓", Theme::primary()),
        Span::styled("/", gray),
        Span::styled("Tab", Theme::primary()),
        Span::styled(" Navigate  ", gray),
        Span::styled("Enter", Theme::primary()),
        Span::styled(" Save  ", gray),
        Span::styled("Esc", Theme::primary()),
        Span::styled(" Cancel", gray),
    ];
    if app.form.is_ssh {
        footer_spans.push(Span::styled("  ^Z", Theme::primary()));
        footer_spans.push(Span::styled(" Test", gray));
    }
    let footer = Line::from(footer_spans);

    if let Some((msg, level)) = &app.status_msg {
        let footer_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(footer_area);
        f.render_widget(
            Paragraph::new(footer).style(Style::default().bg(BG)),
            footer_chunks[0],
        );
        let msg_style = match level {
            StatusLevel::Info    => Theme::primary(),
            StatusLevel::Success => Theme::success(),
            StatusLevel::Warning => Theme::warning(),
            StatusLevel::Error   => Theme::error(),
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg.as_str(), msg_style))
                .style(Style::default().bg(BG))
                .wrap(Wrap { trim: false }),
            footer_chunks[1],
        );
    } else {
        f.render_widget(
            Paragraph::new(footer).style(Style::default().bg(BG)),
            footer_area,
        );
    }
}

pub fn render_delete_confirm(f: &mut Frame, area: Rect, app: &App) {
    let deleting = app.location_deleting;
    let modal_width = 50u16.min(area.width.saturating_sub(4));
    let modal_height = if deleting { 6u16 } else { 5u16 };
    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(x, y, modal_width, modal_height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(Theme::error())
        .title(Span::styled(" Confirm Delete ", Theme::error()));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let name = app
        .delete_confirm
        .as_deref()
        .and_then(|id| app.config.locations.iter().find(|l| l.id == id))
        .map(|l| l.name.as_str())
        .unwrap_or("this location");

    let constraints: Vec<Constraint> = if deleting {
        vec![Constraint::Length(1); 4]
    } else {
        vec![Constraint::Length(1); 3]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled(
            format!("Delete '{}'?", name),
            Theme::warning(),
        )),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            "This will also remove indexed files for this location.",
            Theme::dim(),
        )),
        chunks[1],
    );

    if deleting {
        const SPINNERS: &[char] = &['|', '/', '-', '\\'];
        let anim_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as usize;
        let spinner = SPINNERS[(anim_ms / 100) % SPINNERS.len()];
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("{} Deleting...", spinner),
                Theme::primary(),
            )),
            chunks[2],
        );
    }

    let controls_row = if deleting { 3 } else { 2 };
    let confirm_line = if deleting {
        Line::from(vec![
            Span::styled("Enter", Theme::dim()),
            Span::styled(" Confirm  ", Theme::dim()),
            Span::styled("Esc", Theme::dim()),
            Span::styled(" Cancel", Theme::dim()),
        ])
    } else {
        Line::from(vec![
            Span::styled("Enter", Theme::error()),
            Span::raw(" Confirm  "),
            Span::styled("Esc", Theme::primary()),
            Span::raw(" Cancel"),
        ])
    };
    f.render_widget(Paragraph::new(confirm_line), chunks[controls_row]);
}
