use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::app::App;
use crate::debug;
use crate::ui::theme::{Theme, BG, FG, FG4, AQUA, YELLOW, GREEN, ORANGE, RED};

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let popup_w = (area.width * 7 / 10).max(60).min(area.width);

    // Count rows: header + per-location table + misc fields
    let enabled_locs: Vec<_> = app.config.locations.iter().filter(|l| l.enabled).collect();
    let num_loc_rows = enabled_locs.len();
    // rows: query/target/mode(3) + blank + flags(3) + blank + table header + table rows + blank + misc(2) + hint = ~13 + num_loc_rows
    let num_rows = (13 + num_loc_rows) as u16;
    let popup_h = (num_rows + 4).min(area.height);

    let x = (area.width.saturating_sub(popup_w)) / 2;
    let y = (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(area.x + x, area.y + y, popup_w, popup_h);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ORANGE))
        .style(Style::default().bg(BG))
        .title(Span::styled(
            " Debug Overlay ",
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " ^B snapshot  ^G close ",
            Theme::dim(),
        ));

    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Build content lines
    let mut lines: Vec<Line> = Vec::new();

    // Query / target / mode
    lines.push(kv("query", &app.query));
    lines.push(kv("search_target", app.search_target.label()));
    lines.push(kv("search_mode", app.search_mode.label()));
    lines.push(Line::raw(""));

    // Flags
    lines.push(kv("content_active", &app.content_search_active.to_string()));
    lines.push(kv("name_active", &app.name_search_active.to_string()));
    lines.push(kv("search_stopped", &app.search_stopped.to_string()));
    lines.push(Line::raw(""));

    // Per-location table header
    let col_w = inner.width.saturating_sub(2) as usize;
    let header = format!(
        "  {:<16} {:>8} {:>8} {:>6}  {:<}",
        "location", "indexed", "scanned", "found", "status"
    );
    lines.push(Line::from(Span::styled(
        header[..header.len().min(col_w)].to_string(),
        Style::default().fg(FG).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "  ".to_string() + &"─".repeat(col_w.saturating_sub(2)),
        Style::default().fg(FG4),
    )));

    // Per-location rows
    let scan_counts_snap = app.content_scan_counts.lock()
        .map(|m| m.clone())
        .unwrap_or_default();

    for loc in &enabled_locs {
        let indexed = app.index_location_counts.get(&loc.id).copied().unwrap_or(0);
        let scanned = scan_counts_snap.get(&loc.id).copied().unwrap_or(0);
        let found = app.results.iter().filter(|r| r.file.location_id == loc.id).count();
        let status_raw = app.location_search_states.get(&loc.id)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let status_trunc: String = status_raw.chars().take(20).collect();

        let row_color = if status_raw == "Complete" { GREEN }
            else if !status_raw.is_empty() { YELLOW }
            else { FG4 };

        let row = format!(
            "  {:<16} {:>8} {:>8} {:>6}  {:<}",
            loc.name.chars().take(16).collect::<String>(),
            indexed,
            scanned,
            found,
            status_trunc,
        );
        lines.push(Line::from(Span::styled(
            row[..row.len().min(col_w)].to_string(),
            Style::default().fg(row_color),
        )));
    }

    // Location errors
    if !app.location_errors.is_empty() {
        lines.push(Line::raw(""));
        for (name, err) in &app.location_errors {
            let msg = format!("  ERR {}: {}", name, err);
            lines.push(Line::from(Span::styled(
                msg[..msg.len().min(col_w)].to_string(),
                Style::default().fg(RED),
            )));
        }
    }

    lines.push(Line::raw(""));

    // Misc
    let dbg_level = debug::level();
    let log_path = if dbg_level > 0 {
        "/tmp/rummage-debug.log".to_string()
    } else {
        "(disabled — set debug_level in config)".to_string()
    };
    lines.push(kv("debug_level", &dbg_level.to_string()));
    lines.push(kv("log", &log_path));

    // Render
    let visible_h = inner.height as usize;
    let total = lines.len();
    let skip = if total > visible_h { total - visible_h } else { 0 };
    let visible_lines: Vec<Line> = lines.into_iter().skip(skip).collect();

    f.render_widget(
        Paragraph::new(visible_lines).style(Style::default().bg(BG).fg(AQUA)),
        inner,
    );
}

fn kv<'a>(key: &str, val: &str) -> Line<'a> {
    let key_w = 16usize;
    let key_str = format!("  {:<width$}", key, width = key_w);
    Line::from(vec![
        Span::styled(key_str, Style::default().fg(FG4)),
        Span::styled(val.to_string(), Style::default().fg(FG)),
    ])
}
