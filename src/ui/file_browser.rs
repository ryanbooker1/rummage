use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

use crate::app::{App, FbColorSpan, FbEntry, FbPane, FileBrowser};
use crate::ui::theme::{Theme, BG, BG4, AQUA, YELLOW, BLUE, FG4};

// ── Layout helpers ────────────────────────────────────────────────────────────

/// Width col 1 needs to show every entry name without truncation (+2 borders).
/// Compute the three pane widths: fixed 25% / 25% / 50%.
fn compute_pane_widths(total: u16, _fb: &FileBrowser) -> (u16, u16, u16) {
    let t = total as usize;
    let w1 = t * 25 / 100;
    let w2 = t * 25 / 100;
    let w3 = t - w1 - w2;
    (w1 as u16, w2 as u16, w3 as u16)
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let fb = match &app.fb {
        Some(fb) => fb,
        None => return,
    };

    f.render_widget(Clear, area);

    let header_h = 3u16.min(area.height);
    let header_area  = Rect::new(area.x, area.y, area.width, header_h);
    let content_area = Rect::new(area.x, area.y + header_h, area.width, area.height.saturating_sub(header_h));

    render_header(f, header_area, app);

    let loc_name = app.config.locations.iter()
        .find(|l| l.id == fb.location_id)
        .map(|l| l.name.as_str())
        .unwrap_or("unknown");

    let title = if fb.at_location_root {
        " File Browser — Locations ".to_string()
    } else {
        let cur_path = fb.middle.path.trim_end_matches('/');
        format!(" File Browser — {}  {} ", loc_name, cur_path)
    };
    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(Theme::fb_border())
        .title(Span::styled(title, Theme::fb_border().add_modifier(Modifier::BOLD)));

    let inner = block.inner(content_area);
    f.render_widget(block, content_area);

    if inner.width < 20 || inner.height < 3 {
        return;
    }

    // Clear the full inner area before pane rendering.  Layout::split can
    // leave sub-pixel gaps (rounding) that the per-pane Clears miss; doing a
    // single Clear here guarantees every cell in `inner` is reset to ' '.
    f.render_widget(Clear, inner);

    let middle_sel_is_file = fb.middle.entries.get(fb.middle.selected)
        .map(|e| !e.is_dir)
        .unwrap_or(false);

    let (left_w, mid_w, right_w) = compute_pane_widths(inner.width, fb);

    // Build pane Rects manually to avoid Layout::split solver rounding.
    // compute_pane_widths guarantees left_w + mid_w + right_w == inner.width.
    let left_area  = Rect::new(inner.x,                       inner.y, left_w,  inner.height);
    let mid_area   = Rect::new(inner.x + left_w,              inner.y, mid_w,   inner.height);
    let right_area = Rect::new(inner.x + left_w + mid_w,      inner.y, right_w, inner.height);

    if fb.at_location_root {
        render_label_pane(f, left_area, " Locations ");
    } else {
        render_dir_pane(f, left_area, &fb.left, false, None, false);
    }
    render_dir_pane(f, mid_area, &fb.middle, !fb.preview_focused, None, fb.middle_wrap);

    if middle_sel_is_file {
        let file_entry = fb.middle.entries.get(fb.middle.selected);
        let displaying_old = fb.right_content_loading && !fb.right_lines.is_empty();
        let (display_name, display_entry) = if displaying_old {
            let name = fb.right_lines_path.rsplit('/').next().unwrap_or(&fb.right_lines_path);
            let entry = fb.middle.entries.iter().find(|e| e.name == name);
            (name, entry)
        } else {
            (file_entry.map(|e| e.name.as_str()).unwrap_or(""), file_entry)
        };
        render_file_content_pane(
            f, right_area,
            display_name, display_entry,
            &fb.right_lines, fb.right_content_loading,
            fb.preview_focused, fb.preview_scroll, fb.preview_wrap,
        );
    } else {
        let title_override = if fb.at_location_root && !fb.right.path.is_empty() {
            Some(format!(" {} ", fb.right.path.trim_end_matches('/')))
        } else {
            None
        };
        render_dir_pane(f, right_area, &fb.right, false, title_override.as_deref(), false);
    }

}

// ── Header ────────────────────────────────────────────────────────────────────

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 { return; }
    let fb = match &app.fb {
        Some(fb) => fb,
        None => return,
    };
    let w = area.width as usize;
    let dim = Style::default().fg(FG4).bg(BG);
    let sep = Style::default().fg(BG4).bg(BG);

    // Row 0: ╭─ booker-search ─...─╮
    let title_prefix_w = 17usize; // "╭─ booker-search " = 17 chars
    let top_line = Line::from(vec![
        Span::styled("╭─ ",          Theme::primary()),
        Span::styled("booker-search", Style::default().fg(AQUA).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}╮", "─".repeat(w.saturating_sub(title_prefix_w + 1))),
                     Theme::primary()),
    ]);
    f.render_widget(Paragraph::new(top_line), Rect::new(area.x, area.y, area.width, 1));

    // Row 1: │ context-sensitive key hints │
    let hint_spans: Vec<Span> = if fb.preview_focused {
        vec![
            Span::styled("↑↓",      Theme::primary()), Span::styled(" Scroll",    dim),
            Span::styled(" │ ", sep),
            Span::styled("←",       Theme::primary()), Span::styled(" Back",      dim),
            Span::styled(" │ ", sep),
            Span::styled("w",       Theme::primary()), Span::styled(" Wrap",      dim),
            Span::styled(" │ ", sep),
            Span::styled("Enter",   Theme::primary()), Span::styled(" Open",      dim),
            Span::styled(" │ ", sep),
            Span::styled("F1",      Theme::primary()), Span::styled(" Info",      dim),
            Span::styled(" │ ", sep),
            Span::styled("^C/Esc",  Theme::primary()), Span::styled(" Close",     dim),
        ]
    } else if fb.at_location_root {
        vec![
            Span::styled("↑↓",       Theme::primary()), Span::styled(" Select",   dim),
            Span::styled(" │ ", sep),
            Span::styled("Enter/→",  Theme::primary()), Span::styled(" Open",     dim),
            Span::styled(" │ ", sep),
            Span::styled("←",        Theme::primary()), Span::styled(" Close",    dim),
            Span::styled(" │ ", sep),
            Span::styled("Space/F1", Theme::primary()), Span::styled(" Info",     dim),
            Span::styled(" │ ", sep),
            Span::styled("^C/Esc",   Theme::primary()), Span::styled(" Close",    dim),
        ]
    } else {
        vec![
            Span::styled("↑↓",       Theme::primary()), Span::styled(" Navigate", dim),
            Span::styled(" │ ", sep),
            Span::styled("←",        Theme::primary()), Span::styled(" Locations",dim),
            Span::styled(" │ ", sep),
            Span::styled("→",        Theme::primary()), Span::styled(" Preview",  dim),
            Span::styled(" │ ", sep),
            Span::styled("w",        Theme::primary()), Span::styled(" Wrap",     dim),
            Span::styled(" │ ", sep),
            Span::styled("Enter",    Theme::primary()), Span::styled(" Open",     dim),
            Span::styled(" │ ", sep),
            Span::styled("Space/F1", Theme::primary()), Span::styled(" Info",     dim),
            Span::styled(" │ ", sep),
            Span::styled("^C/Esc",   Theme::primary()), Span::styled(" Close",    dim),
        ]
    };

    let hint_w: usize = hint_spans.iter().map(|s| s.content.chars().count()).sum();
    let inner_w = w.saturating_sub(2);
    let pad = inner_w.saturating_sub(1 + hint_w); // 1 for leading space

    let mut mid: Vec<Span> = vec![
        Span::styled("│", Theme::primary()),
        Span::styled(" ", dim),
    ];
    mid.extend(hint_spans);
    if pad > 0 {
        mid.push(Span::styled(" ".repeat(pad), Style::default().bg(BG)));
    }
    mid.push(Span::styled("│", Theme::primary()));

    f.render_widget(
        Paragraph::new(Line::from(mid)).style(Style::default().bg(BG)),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );

    // Row 2: ╰─...─╯
    let bottom = format!("╰{}╯", "─".repeat(w.saturating_sub(2)));
    f.render_widget(
        Paragraph::new(Span::styled(bottom, Theme::primary())),
        Rect::new(area.x, area.y + 2, area.width, 1),
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn pane_title(path: &str) -> String {
    if path == "__locations__" { return " Locations ".to_string(); }
    let p = path.trim_end_matches('/');
    let last = p.rsplit('/').next().unwrap_or(p);
    if last.is_empty() { " / ".to_string() } else { format!(" {} ", last) }
}

/// Compute the display style for a directory entry.
fn entry_style(entry: &FbEntry, is_selected: bool) -> Style {
    let hidden = entry.name.starts_with('.');
    if is_selected {
        Theme::selected()
    } else if entry.is_dir {
        if hidden { Theme::dir_hidden() } else { Theme::dir() }
    } else if hidden {
        Style::default().fg(FG4)
    } else {
        let ext = entry.name.rsplit('.').next().unwrap_or("");
        match ext {
            "py" | "pyi" | "pyc" => Style::default().fg(YELLOW),
            "nix"                 => Style::default().fg(BLUE),
            _ => Theme::normal(),
        }
    }
}

/// Number of visual rows an entry occupies when wrap is active.
fn entry_visual_lines(entry: &FbEntry, max_name: usize) -> usize {
    if max_name == 0 { return 1; }
    let display_len = entry.name.len() + if entry.is_dir { 1 } else { 0 };
    if display_len == 0 { return 1; }
    (display_len + max_name - 1) / max_name
}

/// Auto-corrected scroll offset for a pane when wrap mode is active.
/// Scroll is in entry-index units; selection is always fully visible.
fn auto_scroll_wrapped(pane: &FbPane, pixel_h: usize, max_name: usize) -> usize {
    let total = pane.entries.len();
    if total == 0 || pixel_h == 0 { return 0; }
    let s = pane.scroll.min(total.saturating_sub(1));

    // Selected is above the viewport → scroll up to it
    if pane.selected < s {
        return pane.selected;
    }

    // Count visual rows from s through selected (inclusive)
    let mut rows = 0usize;
    for i in s..=pane.selected.min(total - 1) {
        rows += entry_visual_lines(&pane.entries[i], max_name);
    }

    if rows <= pixel_h {
        s  // selected is already visible from s
    } else {
        // Selected falls below viewport; find the highest s' that still shows it
        let mut new_s = pane.selected;
        let mut acc = entry_visual_lines(&pane.entries[pane.selected], max_name);
        while new_s > 0 {
            let prev = entry_visual_lines(&pane.entries[new_s - 1], max_name);
            if acc + prev > pixel_h { break; }
            new_s -= 1;
            acc += prev;
        }
        new_s
    }
}

// ── Pane renderers ────────────────────────────────────────────────────────────

fn render_dir_pane(
    f: &mut Frame,
    area: Rect,
    pane: &FbPane,
    is_active: bool,
    title_override: Option<&str>,
    wrap: bool,
) {
    if area.width < 3 { return; }

    let base_title = title_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| pane_title(&pane.path));
    let title = if wrap && is_active {
        // Append [wrap] indicator before the closing space
        let trimmed = base_title.trim_end_matches(' ');
        format!("{}  [wrap] ", trimmed)
    } else {
        base_title
    };

    let border_style = if is_active { Theme::pane_active() } else { Theme::pane_inactive() };
    let title_style  = if is_active { Theme::pane_active().add_modifier(Modifier::BOLD) } else { Theme::pane_inactive() };

    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, title_style));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 { return; }

    // Clear inner to prevent ghost symbols when pane width changes between frames.
    // Block::style only sets cell colors via set_style(), not the symbol characters.
    f.render_widget(Clear, inner);

    if pane.loading {
        f.render_widget(
            Paragraph::new(Span::styled(" Loading…", Theme::dim()))
                .style(Style::default().bg(BG)),
            inner,
        );
        return;
    }

    if pane.entries.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(" (empty)", Theme::dim()))
                .style(Style::default().bg(BG)),
            inner,
        );
        return;
    }

    let pixel_h = inner.height as usize;
    let total = pane.entries.len();
    // 2 chars for "▶ " / "  " prefix; name occupies the rest
    let max_name = (inner.width as usize).saturating_sub(2).max(1);

    if wrap {
        // ── Wrap mode: entries may span multiple visual rows ──────────────
        let scroll = if is_active {
            auto_scroll_wrapped(pane, pixel_h, max_name)
        } else {
            pane.scroll.min(total.saturating_sub(1))
        };

        let mut visual_row = 0usize;
        let mut entry_idx = scroll;
        while visual_row < pixel_h && entry_idx < total {
            let entry = &pane.entries[entry_idx];
            let is_sel = is_active && entry_idx == pane.selected;
            let style = entry_style(entry, is_sel);

            let name_slash = format!(
                "{}{}",
                entry.name,
                if entry.is_dir { "/" } else { "" }
            );
            let chars: Vec<char> = name_slash.chars().collect();
            let mut offset = 0;

            while offset < chars.len() && visual_row < pixel_h {
                let is_first = offset == 0;
                let prefix = if is_first && is_sel { "▶ " } else { "  " };
                let end = (offset + max_name).min(chars.len());
                let chunk: String = chars[offset..end].iter().collect();
                offset = end;

                let row = Rect::new(inner.x, inner.y + visual_row as u16, inner.width, 1);
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(prefix.to_string(), style),
                        Span::styled(chunk, style),
                    ])).style(Style::default().bg(BG)),
                    row,
                );
                visual_row += 1;
            }
            entry_idx += 1;
        }
        // Clear leftover rows from previously longer lists
        for i in visual_row..pixel_h {
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            f.render_widget(Paragraph::new("").style(Style::default().bg(BG)), row);
        }
    } else {
        // ── Normal mode: one entry per row, truncate long names ──────────
        let scroll = if is_active {
            let s = pane.scroll.min(total.saturating_sub(1));
            if pane.selected < s {
                pane.selected
            } else if pixel_h > 0 && pane.selected >= s + pixel_h {
                pane.selected + 1 - pixel_h
            } else {
                s
            }
        } else {
            pane.scroll.min(total.saturating_sub(1))
        };
        let end = (scroll + pixel_h).min(total);

        let items: Vec<ListItem> = pane.entries[scroll..end]
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let is_sel = is_active && (i + scroll) == pane.selected;
                build_entry_line(entry, is_sel, max_name)
            })
            .collect();

        let relative_selected = if is_active && pane.selected >= scroll && pane.selected < end {
            Some(pane.selected - scroll)
        } else {
            None
        };

        let mut state = ListState::default();
        state.select(relative_selected);

        let list = List::new(items)
            .style(Style::default().bg(BG))
            .highlight_style(Style::default())
            .highlight_symbol("");

        f.render_stateful_widget(list, inner, &mut state);
    }
}

/// Render a minimal bordered pane containing only a label — used for the left
/// column in location-picker mode.
fn render_label_pane(f: &mut Frame, area: Rect, label: &str) {
    if area.width < 3 { return; }
    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(Theme::pane_inactive());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 { return; }
    f.render_widget(
        Paragraph::new(Span::styled(label, Theme::dim()))
            .style(Style::default().bg(BG)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
}

fn build_entry_line(entry: &FbEntry, is_selected: bool, max_name: usize) -> ListItem<'static> {
    let style = entry_style(entry, is_selected);
    let prefix = if is_selected { "▶ " } else { "  " };

    let slash = if entry.is_dir { "/" } else { "" };
    let display_len = entry.name.len() + slash.len();
    let name_display = if display_len > max_name && max_name > 2 {
        let avail = max_name.saturating_sub(1 + slash.len());
        let truncated: String = entry.name.chars().take(avail).collect();
        format!("{}…{}", truncated, slash)
    } else {
        format!("{}{}", entry.name, slash)
    };

    ListItem::new(Line::from(vec![
        Span::styled(prefix.to_string(), style),
        Span::styled(name_display, style),
    ]))
}

fn render_file_content_pane(
    f: &mut Frame,
    area: Rect,
    file_name: &str,
    entry: Option<&FbEntry>,
    lines: &[Vec<FbColorSpan>],
    loading: bool,
    is_focused: bool,
    scroll: usize,
    wrap: bool,
) {
    if area.width < 3 { return; }

    let title = if file_name.is_empty() {
        if wrap { " Preview  [wrap] ".to_string() } else { " Preview ".to_string() }
    } else if let Some(e) = entry {
        let size_str = format_size(e.size);
        let mtime_str = e.modified.map(|t| {
            use chrono::{DateTime, Local};
            let dt: DateTime<Local> = t.into();
            dt.format("%Y-%m-%d %H:%M").to_string()
        }).unwrap_or_default();
        let wrap_tag = if wrap { "  [wrap]" } else { "" };
        if mtime_str.is_empty() {
            format!(" {} — {}{} ", file_name, size_str, wrap_tag)
        } else {
            format!(" {} — {}  {}{} ", file_name, size_str, mtime_str, wrap_tag)
        }
    } else {
        let wrap_tag = if wrap { "  [wrap]" } else { "" };
        format!(" {}{} ", file_name, wrap_tag)
    };

    let border_style = if is_focused { Theme::pane_active() } else { Theme::pane_inactive() };
    let title_style  = if is_focused { Theme::pane_active().add_modifier(Modifier::BOLD) } else { Theme::normal() };
    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, title_style));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 { return; }

    // Clear inner to prevent ghost symbols when pane width changes between frames.
    f.render_widget(Clear, inner);

    let content_y = inner.y;
    let content_h = inner.height;

    if loading && lines.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(" Loading…", Theme::dim()))
                .style(Style::default().bg(BG)),
            inner,
        );
        return;
    }

    if lines.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(" (empty)", Theme::dim()))
                .style(Style::default().bg(BG)),
            inner,
        );
        return;
    }

    let pixel_h = content_h as usize;
    // Reserve 1 column on the right for the scrollbar
    let scrollbar_x = inner.x + inner.width.saturating_sub(1);
    let content_w = inner.width.saturating_sub(1) as usize;
    let display_lines: Vec<Vec<FbColorSpan>> = if wrap {
        lines.iter().flat_map(|spans| wrap_line(spans, content_w.max(1))).collect()
    } else {
        lines.iter().cloned().collect()
    };

    let total_lines = display_lines.len();
    let scroll = scroll.min(total_lines.saturating_sub(pixel_h));
    let end = (scroll + pixel_h).min(total_lines);
    let rendered_lines = end - scroll;

    // Write every cell in the content area directly into the buffer.
    // This guarantees no cell is left with stale content from a previous frame,
    // bypassing any Paragraph/style-only-update ambiguity entirely.
    let buf = f.buffer_mut();
    for i in 0..pixel_h {
        let row_y = content_y + i as u16;
        let mut col_x = inner.x;
        let col_end = inner.x + content_w as u16;

        if i < rendered_lines {
            for (text, (r, g, b)) in &display_lines[scroll + i] {
                for ch in text.chars() {
                    if ch == '\n' || ch == '\r' || ch.is_control() { continue; }
                    let ch_w = UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
                    if col_x + ch_w > col_end { break; }
                    buf[(col_x, row_y)]
                        .set_char(ch)
                        .set_fg(Color::Rgb(*r, *g, *b))
                        .set_bg(BG);
                    if ch_w == 2 && col_x + 1 < col_end {
                        buf[(col_x + 1, row_y)].set_char(' ').set_fg(Color::Reset).set_bg(BG);
                    }
                    col_x += ch_w;
                }
            }
        }

        // Fill the remainder of the content columns.
        while col_x < col_end {
            buf[(col_x, row_y)].set_char(' ').set_fg(Color::Reset).set_bg(BG);
            col_x += 1;
        }
    }

    // Scrollbar
    if total_lines > pixel_h && inner.width > 1 {
        let track_h = pixel_h;
        let thumb_h = ((pixel_h * track_h) / total_lines).max(1);
        let max_scroll = total_lines.saturating_sub(pixel_h);
        let thumb_top = if max_scroll > 0 { (scroll * (track_h - thumb_h)) / max_scroll } else { 0 };
        let buf = f.buffer_mut();
        for row in 0..track_h {
            let ch = if row >= thumb_top && row < thumb_top + thumb_h { '█' } else { '│' };
            let style = if row >= thumb_top && row < thumb_top + thumb_h {
                Theme::scrollbar_thumb()
            } else {
                Theme::scrollbar_track()
            };
            buf[(scrollbar_x, content_y + row as u16)]
                .set_char(ch)
                .set_fg(style.fg.unwrap_or(Color::Reset))
                .set_bg(BG);
        }
    }
}

/// Reflow a single logical line of colour spans into one or more visual rows
/// that each fit within `max_w` characters.
fn wrap_line(spans: &[FbColorSpan], max_w: usize) -> Vec<Vec<FbColorSpan>> {
    if max_w == 0 { return vec![Vec::new()]; }
    let mut rows: Vec<Vec<FbColorSpan>> = Vec::new();
    let mut current: Vec<FbColorSpan> = Vec::new();
    let mut used = 0usize;

    for (text, color) in spans {
        let clean: String = text.chars().filter(|&c| c != '\n' && c != '\r').collect();
        if clean.is_empty() { continue; }
        let mut chars = clean.chars().peekable();
        while chars.peek().is_some() {
            let avail = max_w.saturating_sub(used);
            if avail == 0 {
                rows.push(std::mem::take(&mut current));
                used = 0;
                continue;
            }
            let chunk: String = chars.by_ref().take(avail).collect();
            let len = chunk.chars().count();
            current.push((chunk, *color));
            used += len;
            if used >= max_w {
                rows.push(std::mem::take(&mut current));
                used = 0;
            }
        }
    }
    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    rows
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 { return "0 B".to_string(); }
    let mut size = bytes as f64;
    let mut idx = 0;
    while size >= 1024.0 && idx < UNITS.len() - 1 {
        size /= 1024.0;
        idx += 1;
    }
    if idx == 0 { format!("{} B", bytes) } else { format!("{:.1} {}", size, UNITS[idx]) }
}
