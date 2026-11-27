use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, MatchType, SearchMode, SearchTarget};
use crate::ui::theme::{Theme, BG, BG1, BG4, FG, FG4, YELLOW, PURPLE, AQUA, ORANGE};
use crate::ui::config_view;

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    // Layout (top → bottom):
    //   Logo            3 rows
    //   Results         Min(5)
    //   Index progress  4 rows  (purple border, optional — only when indexing)
    //   Status bar      4 rows  (orange border, always visible)
    //   Search bar      3 rows  (cyan border)

    let mut constraints = vec![
        Constraint::Length(3), // logo
        Constraint::Min(5),    // results
    ];

    if app.db_progress.is_some() {
        constraints.push(Constraint::Length(4)); // index progress (purple)
    }

    let num_enabled_locs = app.config.locations.iter().filter(|l| l.enabled).count();
    let show_index_row = !matches!(app.search_mode, crate::app::SearchMode::Live);
    let show_live_rows = !matches!(app.search_mode, crate::app::SearchMode::Index)
        && !app.both_mode_pending_live;
    // Expanded mode always shows per-location rows for Live/Both, even during the index phase.
    let show_expanded_locs = !matches!(app.search_mode, crate::app::SearchMode::Index);
    let status_h = if !app.status_bar_visible {
        0u16
    } else if app.status_bar_expanded && num_enabled_locs > 0 && show_expanded_locs {
        // border + optional index row + one row per location
        2 + (show_index_row as u16) + num_enabled_locs as u16
    } else {
        // border + up to 2 content rows (index + live), but only the ones that apply
        2 + (show_index_row as u16) + (show_live_rows as u16)
    };
    constraints.push(Constraint::Length(status_h)); // status bar (orange, always or hidden)
    constraints.push(Constraint::Length(3)); // search bar

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;

    render_logo(f, chunks[idx], app);
    idx += 1;

    render_results(f, chunks[idx], app);
    idx += 1;

    if app.db_progress.is_some() {
        render_progress(f, chunks[idx], app);
        idx += 1;
    }

    render_status_bar(f, chunks[idx], app);
    idx += 1;

    render_search_bar(f, chunks[idx], app);

    if app.show_master_password_prompt {
        render_master_password_modal(f, area, app);
    }

    if app.show_settings_popup {
        render_settings_popup(f, area, app);
    }

    if app.fb.is_some() {
        crate::ui::file_browser::render(f, area, app);
    }

    if app.show_info_popup {
        crate::ui::info_popup::render(f, area, app);
    }

    // Location form opened from popup — render as overlay on search screen
    if let crate::app::Screen::AddLocation { .. } = &app.screen {
        if app.return_to_popup {
            config_view::render_location_form(f, area, app);
            if app.ssh_plain_warn {
                config_view::render_ssh_plain_warn(f, area);
            }
        }
    }

    if app.delete_confirm.is_some() {
        config_view::render_delete_confirm(f, area, app);
    }

    let _ = idx;
}

fn render_logo(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 || area.width < 10 {
        return;
    }

    let w = area.width as usize;
    let no_active_locs = app.config.locations.is_empty()
        || app.config.locations.iter().all(|l| !l.enabled);
    let searching = !no_active_locs && (app.name_search_active || app.content_search_active || app.search_pending);
    let vis_count = app.visible_count;
    let total = app.results.len();

    // ── Row 0: ╭─ rummage ─...─╮ ──────────────────────────────────
    // span1("╭─ "=3) + span2("rummage"=7) + span3(" " + dashes + "╮") = w
    // → dashes = w - 3 - 7 - 2 = w - 12
    let top_line = Line::from(vec![
        Span::styled("╭─ ",          Theme::primary()),
        Span::styled("rummage", Style::default().fg(AQUA).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}╮", "─".repeat(w.saturating_sub(12))),
                     Theme::primary()),
    ]);
    f.render_widget(
        Paragraph::new(top_line),
        Rect::new(area.x, area.y, area.width, 1),
    );

    // ── Row 1: │ count ▶ MODE ▶ TARGET ▶ Tab settings · ^Q quit [pad] │ ──
    let count_str = if vis_count != total {
        format!(" {}/{} results ", vis_count, total)
    } else if vis_count == 1 {
        format!(" {} result ", vis_count)
    } else if searching {
        format!(" {} results… ", vis_count)
    } else {
        format!(" {} results ", vis_count)
    };

    let count_style = if searching {
        Style::default().fg(YELLOW).bg(BG1).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG).bg(BG1)
    };

    // Build content spans first so we can measure them exactly
    let hint_spans: Vec<Span> = vec![
        Span::styled(" │ ",       Style::default().fg(BG4).bg(BG)),
        Span::styled("Tab",       Theme::primary()),
        Span::styled(" settings", Style::default().fg(FG4).bg(BG)),
        Span::styled(" │ ",       Style::default().fg(BG4).bg(BG)),
        Span::styled("^Q",        Theme::primary()),
        Span::styled(" quit",     Style::default().fg(FG4).bg(BG)),
        Span::styled(" │ ",       Style::default().fg(BG4).bg(BG)),
        Span::styled("^N",             Theme::primary()),
        Span::styled(" file/dir info", Style::default().fg(FG4).bg(BG)),
        Span::styled(" │ ",            Style::default().fg(BG4).bg(BG)),
        Span::styled("→",              Theme::primary()),
        Span::styled(" file browser",  Style::default().fg(FG4).bg(BG)),
    ];
    let count_w: usize = count_str.chars().count();
    let hint_w:  usize = hint_spans.iter().map(|s| s.content.chars().count()).sum();
    let inner_w  = w.saturating_sub(2);
    let pad = inner_w.saturating_sub(count_w + hint_w);

    let mut mid: Vec<Span> = vec![
        Span::styled("│", Theme::primary()),
        Span::styled(count_str, count_style),
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

    // ── Row 2: ╰────────────────────────────────────────────────────────╯ ──
    let bottom = format!("╰{}╯", "─".repeat(w.saturating_sub(2)));
    f.render_widget(
        Paragraph::new(Span::styled(bottom, Theme::primary())),
        Rect::new(area.x, area.y + 2, area.width, 1),
    );
}


fn render_status_bar(f: &mut Frame, area: Rect, app: &App) {
    if !app.status_bar_visible { return; }

    // Include search_pending so the status stays orange during debounce (prevents flash to green).
    // Skip when no locations are configured — nothing will search, so never show "Searching" state.
    let no_active_locs = app.config.locations.is_empty()
        || app.config.locations.iter().all(|l| !l.enabled);
    let searching = !no_active_locs && (app.name_search_active || app.content_search_active || app.search_pending);
    let total_results = app.results.len();

    let state_str = if searching {
        "Searching"
    } else if app.search_stopped {
        "Stopped"
    } else if app.db_progress.is_some() {
        "Indexing"
    } else if !app.query.is_empty() && total_results > 0 {
        "Complete"
    } else if !app.query.is_empty() {
        "No Results"
    } else {
        "Idle"
    };
    // When searching, show what types are being searched (e.g. "Files/Dirs/Content").
    let state_display = if searching {
        match app.search_target {
            crate::app::SearchTarget::Content => "Searching Content".to_string(),
            _ => {
                let name_part = match app.search_target {
                    crate::app::SearchTarget::Files   => "Files",
                    crate::app::SearchTarget::Folders => "Dirs",
                    crate::app::SearchTarget::Both    => "Files/Dirs",
                    crate::app::SearchTarget::Content => unreachable!(),
                };
                if app.content_search {
                    format!("Searching {}/Content", name_part)
                } else {
                    format!("Searching {}", name_part)
                }
            }
        }
    } else {
        state_str.to_string()
    };
    let title = if total_results > 0 {
        format!(" Status · {} ({} results) ", state_display, total_results)
    } else {
        format!(" Status · {} ", state_display)
    };

    let show_live_rows = !matches!(app.search_mode, crate::app::SearchMode::Index)
        && !app.both_mode_pending_live;

    let block = if show_live_rows {
        let expand_label = if app.status_bar_expanded { "[^V Collapse]" } else { "[^V Expand]" };
        Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::status_border())
            .title(Line::from(vec![
                Span::styled(title, Theme::status_border()),
                Span::styled("────", Theme::status_border()),
                Span::styled(expand_label, Theme::status_border()),
            ]))
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::status_border())
            .title(Span::styled(title, Theme::status_border()))
    };

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let show_index_row = !matches!(app.search_mode, crate::app::SearchMode::Live);
    let show_live_rows = !matches!(app.search_mode, crate::app::SearchMode::Index)
        && !app.both_mode_pending_live;
    let show_expanded_locs = !matches!(app.search_mode, crate::app::SearchMode::Index);

    // Calculate max location name width up front so the index row can align with it.
    let max_name_w = app.config.locations.iter()
        .filter(|l| l.enabled)
        .map(|l| l.name.chars().count())
        .max()
        .unwrap_or(8)
        .max("Index".len()); // ensure "Index" fits without truncation

    // ── Index progress bar row ────────────────────────────────────────────────
    let mut next_row: u16 = 0;
    if show_index_row {
        let index_row = Rect::new(inner.x, inner.y, inner.width, 1);
        render_index_progress_row(f, index_row, app, max_name_w);
        next_row = 1;
    }

    // Spinner + indeterminate bar animation: both driven by wall-clock time, no extra state.
    const SPINNERS: &[char] = &['|', '/', '-', '\\'];
    let anim_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as usize;
    let spinner = SPINNERS[(anim_ms / 100) % SPINNERS.len()];

    if app.status_bar_expanded && show_expanded_locs {
        // ── Expanded: optional index row above, then one text row per location ──
        // (location rows show even during the index phase of Both mode)

        let mut results_per_loc: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for r in &app.results {
            *results_per_loc.entry(r.file.location_id.clone()).or_insert(0) += 1;
        }

        let search_done = !searching && app.query.len() >= 2 && !app.search_stopped;

        for (i, loc) in app.config.locations.iter().filter(|l| l.enabled).enumerate() {
            let row = next_row + i as u16;
            if row >= inner.height { break; }
            let found = results_per_loc.get(&loc.id).copied().unwrap_or(0);
            let (loc_scan_count, current_dir) = app.location_search_states.get(&loc.id)
                .cloned()
                .unwrap_or((0, String::new()));
            // For content search use the shared Mutex counter (bypasses channel congestion).
            // For name search use the count from location_search_states (files scanned so far).
            // Never read content_scan_counts during name search — it retains stale values.
            let scan_count = if app.content_search_active {
                app.content_scan_counts.lock().ok()
                    .and_then(|m| m.get(&loc.id).copied())
                    .unwrap_or(0)
            } else {
                loc_scan_count
            };

            let loc_excluded = app.active_location.as_ref().map(|id| id != &loc.id).unwrap_or(false);

            let has_error = !loc_excluded && app.location_errors.iter().any(|(n, _)| n == &loc.name);
            let is_complete = !loc_excluded && current_dir == "Complete";
            let is_queued = !loc_excluded && current_dir == "queued…" && searching;
            let is_active = !loc_excluded && searching && !is_complete && !is_queued && !current_dir.is_empty();

            let row_style = if loc_excluded {
                Theme::dim()
            } else if has_error {
                Theme::status_error()
            } else if is_complete || search_done {
                Theme::status_complete()
            } else if searching {
                Theme::status_searching()
            } else {
                Theme::dim()
            };

            let name_padded = format!("{:<width$}", loc.name, width = max_name_w);

            // Left: icon + name + counts
            let left = if loc_excluded {
                format!("  {}  —", name_padded)
            } else if has_error {
                format!("⚠ {}  error", name_padded)
            } else if is_complete || search_done {
                format!("✓ {}  {} found", name_padded, found)
            } else if is_queued {
                format!("  {}  [queued]", name_padded)
            } else if is_active {
                if scan_count > 0 {
                    format!("{} {}  {} found, {} scanned", spinner, name_padded, found, scan_count)
                } else {
                    format!("{} {}  {} found", spinner, name_padded, found)
                }
            } else {
                format!("  {}  {} found", name_padded, found)
            };

            let left_w = left.chars().count() as u16;
            let path_avail = inner.width.saturating_sub(left_w + 1);
            let row_area = Rect::new(inner.x, inner.y + row, inner.width, 1);

            // Right: current path shown when active, truncated from the left so the filename end
            // is always visible — the most specific / useful part of a long path.
            let show_path = is_active
                && app.content_search_active
                && !current_dir.is_empty()
                && current_dir != "queued…"
                && path_avail > 4;

            if show_path {
                let label = " Last match: ";
                let label_w = label.chars().count();
                let path_chars_avail = (path_avail as usize).saturating_sub(label_w);
                let path_display = if path_chars_avail > 4 {
                    truncate_from_start(&current_dir, path_chars_avail)
                } else {
                    String::new()
                };
                let left_area = Rect::new(inner.x, inner.y + row, left_w, 1);
                let path_area = Rect::new(inner.x + left_w, inner.y + row, path_avail, 1);
                f.render_widget(Paragraph::new(Span::styled(left, row_style)), left_area);
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(label, Theme::dim()),
                        Span::styled(path_display, Theme::dim()),
                    ])),
                    path_area,
                );
            } else {
                f.render_widget(Paragraph::new(Span::styled(left, row_style)), row_area);
            }
        }
    } else if show_live_rows {
        // ── Collapsed: live location summary on the next available row ──
        if next_row >= inner.height { return; }
        let line_area = Rect::new(inner.x, inner.y + next_row, inner.width, 1);

        {
            let mut results_per_loc: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for r in &app.results {
                *results_per_loc.entry(r.file.location_id.clone()).or_insert(0) += 1;
            }

            let prefix = if searching { "Searching" } else { state_str };
            let prefix_str = format!(" {}: ", prefix);
            let prefix_w = prefix_str.chars().count();
            let avail_w = inner.width as usize;
            let content_w = avail_w.saturating_sub(prefix_w);

            let enabled_locs: Vec<_> = app.config.locations.iter().filter(|l| l.enabled).collect();
            let n_locs = enabled_locs.len();

            // Build (label, style) for each location — count only, no file paths
            let loc_entries: Vec<(String, Style)> = enabled_locs.iter().map(|loc| {
                let loc_excluded = app.active_location.as_ref().map(|id| id != &loc.id).unwrap_or(false);
                let n = results_per_loc.get(&loc.id).copied().unwrap_or(0);
                let (_, current_dir) = app.location_search_states.get(&loc.id)
                    .cloned()
                    .unwrap_or((0, String::new()));
                let has_error = !loc_excluded && app.location_errors.iter().any(|(name, _)| name == &loc.name);
                let is_complete = !loc_excluded && current_dir == "Complete";
                let search_done_loc = !loc_excluded && !searching && app.query.len() >= 2 && !app.search_stopped;
                let is_active = !loc_excluded && searching && !is_complete && current_dir != "queued…" && !current_dir.is_empty();
                let style = if loc_excluded {
                    Theme::dim()
                } else if has_error {
                    Theme::status_error()
                } else if is_complete || search_done_loc {
                    Theme::status_complete()
                } else if searching {
                    Theme::status_searching()
                } else {
                    Theme::dim()
                };
                let label = if loc_excluded {
                    format!("{} —", loc.name)
                } else if is_active {
                    format!("{} {} ({})", spinner, loc.name, n)
                } else if current_dir == "queued…" && searching {
                    format!("{} [queued]", loc.name)
                } else {
                    format!("{} ({})", loc.name, n)
                };
                (label, style)
            }).collect();

            // Build the full location text as (char, Style) pairs for marquee scrolling.
            let mut char_styles: Vec<(char, Style)> = Vec::new();
            for (i, (label, style)) in loc_entries.iter().enumerate() {
                if i > 0 {
                    for c in ", ".chars() { char_styles.push((c, Theme::dim())); }
                }
                for c in label.chars() { char_styles.push((c, *style)); }
            }
            let total_w = char_styles.len();

            let mut spans: Vec<Span> = vec![Span::raw(prefix_str)];
            if n_locs == 0 {
                spans.push(Span::raw("…".to_string()));
            } else if total_w <= content_w {
                // Everything fits — render statically with per-location styles
                let mut run = String::new();
                let mut run_style = char_styles[0].1;
                for (c, style) in &char_styles {
                    if *style == run_style {
                        run.push(*c);
                    } else {
                        spans.push(Span::styled(run.clone(), run_style));
                        run = c.to_string();
                        run_style = *style;
                    }
                }
                if !run.is_empty() { spans.push(Span::styled(run, run_style)); }
            } else {
                // Marquee: advance 1 character every 150 ms for smooth scrolling.
                // Pad the end with spaces so there's a visible gap before the loop point.
                for c in "     ".chars() { char_styles.push((c, Theme::dim())); }
                let total_w = char_styles.len();
                let offset = (anim_ms / 150) % total_w;

                let mut run = String::new();
                let mut run_style = char_styles[offset % total_w].1;
                for i in 0..content_w {
                    let (c, style) = char_styles[(offset + i) % total_w];
                    if style == run_style {
                        run.push(c);
                    } else {
                        spans.push(Span::styled(run.clone(), run_style));
                        run = c.to_string();
                        run_style = style;
                    }
                }
                if !run.is_empty() { spans.push(Span::styled(run, run_style)); }
            }

            f.render_widget(Paragraph::new(ratatui::text::Line::from(spans)), line_area);
        }
    }
}

fn render_index_progress_row(f: &mut Frame, area: Rect, app: &App, max_name_w: usize) {
    let (processed, total) = app.index_search_progress.unwrap_or((0, 0));

    const SPINNERS: &[char] = &['|', '/', '-', '\\'];
    let anim_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as usize;
    let spinner = SPINNERS[(anim_ms / 100) % SPINNERS.len()];

    // Pad "Index" to max_name_w so counts align with per-location rows below.
    let name_padded = format!("{:<width$}", "Index", width = max_name_w);

    // While a DB rebuild is running, show a live rebuilding indicator.
    if let Some(ref prog) = app.db_progress {
        if !prog.done {
            let label = format!("{} {}  rebuilding…", spinner, name_padded);
            f.render_widget(
                Paragraph::new(Span::styled(label, Theme::status_searching())),
                area,
            );
            return;
        }
    }

    if total == 0 {
        // No index search has started (or index is empty) — show idle text
        let label = format!("  {}  {} files", name_padded, app.index_total_count);
        f.render_widget(
            Paragraph::new(Span::styled(label, Theme::dim())),
            area,
        );
        return;
    }

    let ratio = if total > 0 { processed as f64 / total as f64 } else { 0.0 };
    let percent = (ratio * 100.0) as u16;
    let done = processed >= total;

    let bar_style  = if done { Theme::status_complete() } else { Theme::status_searching() };
    let label_style = bar_style;

    let label = if done {
        format!("✓ {}  {}/{}", name_padded, processed, total)
    } else {
        format!("{} {}  {}/{}", spinner, name_padded, processed, total)
    };

    // Reserve space: label | space + [bar] pct%
    // "[" + bar_chars + "]" + " " + "100%" = bar_chars + 7 overhead chars minimum
    let label_w = label.chars().count().min(area.width as usize / 2) as u16;
    let bar_area_w = area.width.saturating_sub(label_w);

    if label_w > 0 {
        let label_area = Rect::new(area.x, area.y, label_w, 1);
        f.render_widget(
            Paragraph::new(Span::styled(label.clone(), label_style)),
            label_area,
        );
    }

    // bar_area_w includes: 1 space + "[" + bar_chars + "]" + " " + pct_str
    // pct_str is at most "100%" = 4 chars; overhead = 1+1+1+1+4 = 8
    if bar_area_w > 8 {
        let pct_str = format!("{:3}%", percent);
        // overhead: 1 (space) + 1 ([) + 1 (]) + 1 ( ) + 4 ("100%") — fixed width so bar never shifts
        let overhead = 4 + 4u16;
        let bar_chars = bar_area_w.saturating_sub(overhead) as usize;
        let filled = ((ratio * bar_chars as f64).round() as usize).min(bar_chars);
        let empty  = bar_chars - filled;
        let bar_str = format!(
            " [{}{}] {}",
            "█".repeat(filled),
            "░".repeat(empty),
            pct_str,
        );
        let bar_area = Rect::new(area.x + label_w, area.y, bar_area_w, 1);
        f.render_widget(
            Paragraph::new(Span::styled(bar_str, bar_style)),
            bar_area,
        );
    }
}

fn render_search_bar(f: &mut Frame, area: Rect, app: &App) {
    let consec_on = app.config.settings.consecutive_search;
    let case_on   = app.config.settings.case_sensitive_search;

    let dim_style = Theme::dim();

    let wildcard_indicator = if crate::search::is_glob_query(&app.query) {
        Span::styled(" [wildcard]", Theme::highlight())
    } else {
        Span::raw("")
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::primary())
        .title(Span::styled(" Search ", Theme::primary()));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let sep = Span::styled(" │ ", Style::default().fg(BG4));
    let chars: Vec<char> = app.query.chars().collect();
    let cur = app.cursor_pos.min(chars.len());
    let before_cursor: String = chars[..cur].iter().collect();
    let cursor_char = chars.get(cur).copied().unwrap_or(' ');
    let after_cursor: String = if cur + 1 < chars.len() { chars[cur + 1..].iter().collect() } else { String::new() };
    let mut search_spans = vec![
        Span::styled(" ", dim_style),
        Span::styled(before_cursor, Theme::input_active()),
        Span::styled(cursor_char.to_string(), Theme::cursor()),
        Span::styled(after_cursor, Theme::input_active()),
        wildcard_indicator,
    ];
    if app.controls_bar_visible {
        search_spans.extend([
            Span::styled("  ^Z ", Theme::primary()),
            Span::styled("fuzzy_match: ", dim_style),
            Span::styled(if consec_on { "OFF" } else { "ON" }, if consec_on { Theme::off() } else { Theme::on() }),
            sep.clone(),
            Span::styled("^A ", Theme::primary()),
            Span::styled("case sensitive: ", dim_style),
            Span::styled(if case_on { "ON" } else { "OFF" }, if case_on { Theme::on() } else { Theme::off() }),
        ]);
    }
    f.render_widget(Paragraph::new(Line::from(search_spans)), inner);
}

fn render_results(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::primary());

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 4 {
        return;
    }

    let loc_col_w = app.config.locations.iter()
        .filter(|l| l.enabled)
        .map(|l| l.name.chars().count())
        .max()
        .unwrap_or(0)
        .max("Location".len());
    let match_col_w = 16usize; // badge(11) + space + [I/S](3) + space
    let scrollbar_w = 1usize;
    // When controls bar is hidden: Row 0: column labels; Rows 1+: results
    // When controls bar is shown:  Row 0: settings header; Row 1: divider; Row 2: column labels; Rows 3+: results
    let controls = app.controls_bar_visible;
    let col_row_offset  = if controls { 2u16 } else { 0 };
    let results_offset  = if controls { 3u16 } else { 1 };
    let results_shrink: u16 = if controls { 3 } else { 1 };
    let settings_header_area = Rect::new(inner.x, inner.y,                        inner.width, 1);
    let divider_area          = Rect::new(inner.x, inner.y + 1,                   inner.width, 1);
    let header_area           = Rect::new(inner.x, inner.y + col_row_offset,       inner.width, 1);
    let results_area          = Rect::new(inner.x, inner.y + results_offset,       inner.width, inner.height.saturating_sub(results_shrink));

    // Scrollbar area (rightmost column of results_area)
    let list_area = if inner.width > scrollbar_w as u16 + 1 {
        Rect::new(
            results_area.x,
            results_area.y,
            results_area.width.saturating_sub(scrollbar_w as u16),
            results_area.height,
        )
    } else {
        results_area
    };
    let scrollbar_area = Rect::new(
        results_area.x + list_area.width,
        results_area.y,
        scrollbar_w as u16,
        results_area.height,
    );

    let visible = app.visible_results();

    // Conflict flags: content search is a live-only scan and cannot use the index.
    let mode_is_index = matches!(app.search_mode, crate::app::SearchMode::Index);
    let target_is_content = app.search_target == SearchTarget::Content;
    // Case A: file_content ON + index mode + non-content target (name results still appear)
    let warn_content_blocked = app.content_search && mode_is_index && !target_is_content;
    // Case B: content target + index mode (nothing can run at all)
    let err_content_target = mode_is_index && target_is_content;

    // Search state indicator for column header: spinner in-progress, ✓ complete, · idle
    let (state_label, state_style): (String, Style) = if app.name_search_active || app.content_search_active {
        const SPINNERS: &[char] = &['|', '/', '-', '\\'];
        let anim_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as usize;
        let spinner = SPINNERS[(anim_ms / 100) % SPINNERS.len()];
        (format!(" {}", spinner), Theme::status_searching())
    } else if !visible.is_empty() {
        (" ✓".to_string(), Theme::status_complete())
    } else if app.query.len() >= 2 {
        (" ○".to_string(), Theme::dim())
    } else {
        (String::new(), Theme::dim())
    };

    let col_header = Line::from(vec![
        Span::styled(
            format!("  {:<width$}", "Location", width = loc_col_w),
            Theme::header(),
        ),
        Span::styled(
            format!("  {:<width$}", "Match", width = match_col_w),
            Theme::header(),
        ),
        Span::styled("  Path", Theme::header()),
        Span::styled(state_label, state_style),
    ]);
    if controls {
        render_results_settings_header(f, settings_header_area, app);
        f.render_widget(
            Paragraph::new(Span::styled("─".repeat(inner.width as usize), Theme::primary())),
            divider_area,
        );
    }
    f.render_widget(Paragraph::new(col_header), header_area);

    if visible.is_empty() {
        let index_empty = app.index_total_count == 0;
        let no_locations = app.config.locations.is_empty();
        let no_enabled   = !no_locations && app.config.locations.iter().all(|l| !l.enabled);
        let row0 = Rect::new(list_area.x, list_area.y,     list_area.width, 1);
        let row1 = Rect::new(list_area.x, list_area.y + 1, list_area.width, list_area.height.saturating_sub(1));

        if mode_is_index && index_empty && app.query.len() >= 2 && !app.name_search_active {
            // Index mode + empty index + active query: always show the actionable index error,
            // even when no locations are configured — the root fix is the same either way.
            f.render_widget(
                Paragraph::new(Span::styled(
                    "✗ No indexed files — press ^R to build the index, or switch to Live or Both mode (^S)",
                    Theme::error(),
                )),
                row0,
            );
        } else if no_locations {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "⚠ No locations configured — Tab to add one",
                    Theme::warning(),
                )),
                row0,
            );
        } else if no_enabled {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "⚠ No locations are enabled — open settings to enable one (Tab)",
                    Theme::warning(),
                )),
                row0,
            );
        } else if err_content_target {
            // Case B: search_target=content + index mode — nothing runs; error only, no hint row.
            f.render_widget(
                Paragraph::new(Span::styled(
                    "⚠ Content target requires Live or Both mode — file contents are not in the index (^S to switch)",
                    Theme::warning(),
                )),
                row0,
            );
        } else if warn_content_blocked && app.query.len() < 2 {
            // Case A, idle: file_content ON + index mode — warning on row 0, hint on row 1.
            f.render_widget(
                Paragraph::new(Span::styled(
                    "⚠ File content search is disabled in Index mode — switch to Live or Both mode (^S)",
                    Theme::warning(),
                )),
                row0,
            );
            let hint = if index_empty {
                "Type to search (live mode — press ^R to build index for speed)"
            } else {
                "Type at least 2 characters to search..."
            };
            f.render_widget(Paragraph::new(Span::styled(hint, Theme::dim())), row1);
        } else {
            // Normal cases
            let (display_msg, msg_style) = if app.query.len() < 2 {
                let text = if index_empty {
                    "Type to search (live mode — press ^R to build index for speed)".to_string()
                } else {
                    "Type at least 2 characters to search...".to_string()
                };
                (text, Theme::dim())
            } else if app.name_search_active || app.content_search_active {
                ("Searching…".to_string(), Theme::status_searching())
            } else if !app.results.is_empty() {
                // Results exist but all filtered out by search_target
                let target_name = match app.search_target {
                    SearchTarget::Files   => "file",
                    SearchTarget::Folders => "directory",
                    SearchTarget::Both    => "",
                    SearchTarget::Content => "content",
                };
                let hint = if app.active_location.is_some() {
                    format!("No {} results for '{}' — change target (^T) or location (^L)", target_name, app.active_location_label())
                } else {
                    format!("No {} results — try a different target (^T)", target_name)
                };
                (hint, Theme::dim())
            } else {
                ("No results found.".to_string(), Theme::dim())
            };
            f.render_widget(Paragraph::new(Span::styled(display_msg, msg_style)), row0);

            // Show location errors below the message
            if !app.location_errors.is_empty() {
                render_location_errors(f, row1, app);
            }
        }
        return;
    }

    // Always show location errors at the bottom even when there are results
    let (results_area, errors_area) = if !app.location_errors.is_empty() {
        let err_rows = (app.location_errors.len() as u16 + 1).min(list_area.height / 3);
        let results_h = list_area.height.saturating_sub(err_rows);
        (
            Rect::new(list_area.x, list_area.y, list_area.width, results_h),
            Some(Rect::new(list_area.x, list_area.y + results_h, list_area.width, err_rows)),
        )
    } else {
        (list_area, None)
    };
    let list_area = results_area;

    let available_width = list_area.width as usize;
    let prefix_width = 2usize;
    // prefix(2) + loc(12) + gap(2) + match_col(14) + gap(2) = 32
    let max_path_width = available_width
        .saturating_sub(prefix_width + loc_col_w + 2 + match_col_w + 2);

    let pixel_h = list_area.height as usize;
    let total = visible.len();
    let scroll = app.result_scroll.min(total.saturating_sub(1));

    // Accumulate item heights (content=2 lines, name=1) to find the visible end index.
    let mut end = scroll;
    let mut used_px = 0usize;
    while end < total {
        let h = if visible[end].content_line.is_some() { 2 } else { 1 };
        if used_px + h > pixel_h { break; }
        used_px += h;
        end += 1;
    }
    let _visible_height = end - scroll; // item count currently visible (used by caller via visible_height())

    // Indent for line 2 aligns it under the path column.
    let content_indent = prefix_width + loc_col_w + 2 + match_col_w + 2; // = 34
    let preview_width = available_width.saturating_sub(content_indent + 2); // 2 for "│ "

    let items: Vec<ListItem> = visible
        .get(scroll..end)
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(i, result)| {
            let abs_i = i + scroll;
            let is_selected = abs_i == app.selected;
            let prefix = if is_selected { "▶ " } else { "  " };

            // Location column
            let loc_name = app
                .config
                .locations
                .iter()
                .find(|l| l.id == result.file.location_id)
                .map(|l| l.name.as_str())
                .unwrap_or("");
            let loc_display = format!("{:<width$}", loc_name, width = loc_col_w);

            let path = &result.file.display_path;
            let path_display = truncate_path(path, max_path_width);

            let dir_mark = if result.file.is_dir {
                Span::styled("/", Theme::dim())
            } else {
                Span::raw("")
            };

            let result_key = format!("{}:{}", result.file.location_id, result.file.path);
            let is_live = !result.is_indexed || app.live_confirmed_keys.contains(&result_key);
            let src_label = if is_live { "[S]" } else { "[I]" };
            let (badge_text, badge_style) = match result.match_type {
                MatchType::Name if result.file.is_dir => (
                    format!("[directory] {}", src_label),
                    Theme::dir_badge(),
                ),
                MatchType::Name => (
                    format!("[filename]  {}", src_label),
                    Theme::name_badge(),
                ),
                MatchType::Content => (
                    format!("[content]   {}", src_label),
                    Theme::content_badge(),
                ),
            };
            let badge_col = format!("{:<width$}", badge_text, width = match_col_w);

            let base_style = if is_selected { Theme::selected() } else { Theme::normal() };
            let loc_style = if is_selected { Theme::selected() } else { Theme::loc_col() };
            let highlight_style = if is_selected { Theme::highlight_selected() } else { Theme::highlight() };

            // Line 1: location | badge | path
            let mut spans = vec![
                Span::styled(prefix, base_style),
                Span::styled(loc_display, loc_style),
                Span::raw("  "),
                Span::styled(badge_col, badge_style),
                Span::raw("  "),
            ];
            let remapped = remap_path_indices(path, &path_display, &result.match_indices);
            spans.extend(render_path_highlighted(&path_display, &remapped, base_style, highlight_style));
            spans.push(dir_mark);
            let line1 = Line::from(spans);

            // Line 2: content preview (only for content matches)
            if let Some(content) = &result.content_line {
                let linenum_str = match result.content_line_number {
                    Some(n) => format!("[{}] ", n),
                    None    => String::new(),
                };
                let linenum_w = linenum_str.len();
                let preview_w = preview_width.saturating_sub(linenum_w);
                let mut l2 = vec![
                    Span::raw(" ".repeat(content_indent)),
                    Span::styled("│ ", Theme::dim()),
                    Span::styled(linenum_str, Theme::primary()),
                ];
                if preview_w > 0 {
                    l2.extend(content_preview_spans(content, &app.query, preview_w, is_selected));
                }
                ListItem::new(vec![line1, Line::from(l2)])
            } else {
                ListItem::new(line1)
            }
        })
        .collect();

    let relative_selected = if app.selected >= scroll && app.selected < end {
        Some(app.selected - scroll)
    } else {
        None
    };
    let mut state = ListState::default();
    state.select(relative_selected);

    let list = List::new(items)
        .highlight_style(Style::default())
        .highlight_symbol("");

    f.render_stateful_widget(list, list_area, &mut state);

    // Scrollbar — pixel-based to handle mixed 1/2-line items correctly.
    // When content search is off every item is 1 line, so skip the O(n) sum.
    let (total_px, scroll_px) = if app.content_search {
        let t: usize = visible.iter().map(|r| if r.content_line.is_some() { 2 } else { 1 }).sum();
        let s: usize = visible.iter().take(scroll).map(|r| if r.content_line.is_some() { 2 } else { 1 }).sum();
        (t, s)
    } else {
        (total, scroll)
    };
    if total_px > pixel_h && scrollbar_area.width > 0 {
        let track_h = scrollbar_area.height as usize;
        let thumb_h = ((pixel_h * track_h) / total_px).max(1);
        let thumb_top = {
            let max_scroll_px = total_px.saturating_sub(pixel_h);
            if max_scroll_px > 0 { (scroll_px * (track_h - thumb_h)) / max_scroll_px } else { 0 }
        };

        for row in 0..track_h {
            let ch = if row >= thumb_top && row < thumb_top + thumb_h {
                Span::styled("█", Theme::scrollbar_thumb())
            } else {
                Span::styled("│", Theme::scrollbar_track())
            };
            let bar_area = Rect::new(scrollbar_area.x, scrollbar_area.y + row as u16, 1, 1);
            f.render_widget(Paragraph::new(Line::from(vec![ch])), bar_area);
        }
    }

    if let Some(ea) = errors_area {
        render_location_errors(f, ea, app);
    }
}

fn render_results_settings_header(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let dim_style = Theme::dim();
    let key_style = Theme::primary();
    let sep       = Span::styled(" │ ", Style::default().fg(BG4));

    // content search: OFF dim, ON green bold
    let content_val = if app.content_search { "ON" } else { "OFF" };
    let content_val_s = if app.content_search { Theme::on() } else { dim_style };

    // search mode: active value coloured
    let mode_val = app.search_mode.label();
    let mode_val_s = match app.search_mode {
        SearchMode::Index => Theme::normal(),
        SearchMode::Live  => Theme::warning(),
        SearchMode::Both  => Theme::on(),
    };

    // search_target: active value coloured
    let target_val = app.search_target.label();
    let target_val_s = match app.search_target {
        SearchTarget::Files   => Theme::normal(),
        SearchTarget::Folders => Theme::warning(),
        SearchTarget::Both    => Style::default().fg(PURPLE),
        SearchTarget::Content => Style::default().fg(AQUA),
    };

    // active_location: show full name, let ratatui clip if terminal too narrow
    let loc_name = app.active_location_label().to_string();
    let loc_val_s = if app.active_location.is_some() {
        Style::default().fg(PURPLE).add_modifier(Modifier::BOLD)
    } else {
        dim_style
    };

    let line = Line::from(vec![
        Span::styled(" ^S ", key_style),
        Span::styled("search_mode: ", dim_style),
        Span::styled(mode_val, mode_val_s),
        sep.clone(),
        Span::styled("^F ", key_style),
        Span::styled("file_content: ", dim_style),
        Span::styled(content_val, content_val_s),
        sep.clone(),
        Span::styled("^T ", key_style),
        Span::styled("search_target: ", dim_style),
        Span::styled(target_val, target_val_s),
        sep.clone(),
        Span::styled("^L ", key_style),
        Span::styled("location: ", dim_style),
        Span::styled(loc_name, loc_val_s),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_location_errors(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 || app.location_errors.is_empty() {
        return;
    }
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(
        Paragraph::new(Span::styled(
            "  ⚠ Location errors (press ^R to retry):",
            Theme::warning(),
        )),
        header_area,
    );
    for (i, (name, err)) in app.location_errors.iter().enumerate() {
        let row = i as u16 + 1;
        if row >= area.height {
            break;
        }
        let row_area = Rect::new(area.x, area.y + row, area.width, 1);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("    ", Theme::normal()),
                Span::styled(format!("{:<12}", name), Theme::error()),
                Span::styled(format!("  {}", err), Theme::dim()),
            ])),
            row_area,
        );
    }
}

/// Remap match_indices from full display_path positions to truncated path_display positions.
/// Truncation only removes a prefix, so the common suffix (filename + tail) stays intact.
fn remap_path_indices(display_path: &str, path_display: &str, indices: &[usize]) -> Vec<usize> {
    if indices.is_empty() {
        return Vec::new();
    }
    if display_path == path_display {
        return indices.to_vec();
    }
    let dp_chars: Vec<char> = display_path.chars().collect();
    let pd_chars: Vec<char> = path_display.chars().collect();
    let dp_len = dp_chars.len();
    let pd_len = pd_chars.len();

    // Count how many chars match from the end (the preserved suffix)
    let common = dp_chars.iter().rev()
        .zip(pd_chars.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    if common == 0 {
        return Vec::new();
    }

    let dp_suffix_start = dp_len.saturating_sub(common);
    let pd_suffix_start = pd_len.saturating_sub(common);

    indices.iter().filter_map(|&idx| {
        if idx >= dp_suffix_start {
            Some(pd_suffix_start + (idx - dp_suffix_start))
        } else {
            None // index falls in the truncated prefix — skip
        }
    }).collect()
}

fn render_path_highlighted(
    path: &str,
    match_indices: &[usize],
    base_style: Style,
    highlight_style: Style,
) -> Vec<Span<'static>> {
    if match_indices.is_empty() {
        return vec![Span::styled(path.to_string(), base_style)];
    }

    let mut spans = Vec::new();
    let chars: Vec<char> = path.chars().collect();
    let total = chars.len();
    let mut i = 0usize;

    while i < total {
        if match_indices.contains(&i) {
            let mut run = String::new();
            while i < total && match_indices.contains(&i) {
                run.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(run, highlight_style));
        } else {
            let mut run = String::new();
            while i < total && !match_indices.contains(&i) {
                run.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(run, base_style));
        }
    }

    spans
}

/// Truncate a path string to `max_width` chars, removing from the START so the
/// most specific (rightmost) part of the path remains visible.
fn truncate_from_start(path: &str, max_width: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max_width {
        return path.to_string();
    }
    let keep = max_width.saturating_sub(1); // 1 for '…'
    let start = chars.len().saturating_sub(keep);
    format!("…{}", chars[start..].iter().collect::<String>())
}

fn truncate_path(path: &str, max_width: usize) -> String {
    if path.width() <= max_width {
        return path.to_string();
    }
    if let Some(pos) = path.rfind('/') {
        let filename = &path[pos + 1..];
        if filename.width() + 4 <= max_width {
            let prefix_width = max_width - filename.width() - 4;
            let path_start: String = path.chars().take(prefix_width).collect();
            return format!("{}…/{}", path_start, filename);
        }
    }
    let truncated: String = path.chars().take(max_width.saturating_sub(1)).collect();
    format!("{}…", truncated)
}

fn render_progress(f: &mut Frame, area: Rect, app: &App) {
    if let Some(progress) = &app.db_progress {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::progress_border())
            .title(Span::styled(" Indexing ", Theme::progress_border()));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.height == 0 {
            return;
        }

        let row1_area = Rect::new(inner.x, inner.y, inner.width, 1);
        let row2_area = if inner.height >= 2 {
            Some(Rect::new(inner.x, inner.y + 1, inner.width, 1))
        } else {
            None
        };

        if progress.done {
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!(" Done — {} items indexed", progress.current),
                    Theme::success(),
                )),
                row1_area,
            );
            return;
        }

        // Row 1: location name + count
        let row1_text = if progress.location_name.is_empty() {
            " Starting indexing…".to_string()
        } else {
            format!(
                " {} — {} files indexed so far",
                progress.location_name, progress.current
            )
        };
        f.render_widget(
            Paragraph::new(Span::styled(row1_text, Theme::progress_border())),
            row1_area,
        );

        // Row 2: current path/status (start-truncated to fit)
        if let Some(r2) = row2_area {
            let path = &progress.current_path;
            let w = r2.width as usize;
            let prefix = "  ";
            let available = w.saturating_sub(prefix.len());
            let display = if path.is_empty() {
                String::new()
            } else if path.len() > available && available > 4 {
                let start = path.len().saturating_sub(available.saturating_sub(1));
                format!("{}…{}", prefix, &path[start..])
            } else {
                format!("{}{}", prefix, path)
            };
            f.render_widget(
                Paragraph::new(Span::styled(display, Theme::dim())),
                r2,
            );
        }
    }
}


fn render_master_password_modal(f: &mut Frame, area: Rect, app: &App) {
    let modal_width = 52u16.min(area.width.saturating_sub(4));
    let modal_height = 7u16;
    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(x, y, modal_width, modal_height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(Theme::warning())
        .title(Span::styled(" Master Password ", Theme::warning()));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled(
            "Enter sudo password to unlock credentials:",
            Theme::normal(),
        )),
        chunks[0],
    );

    let masked: String = "*".repeat(app.master_password_input.len());
    let pw_line = Line::from(vec![
        Span::styled("Password: ", Theme::normal()),
        Span::styled(masked, Theme::normal()),
        Span::styled("█", Theme::cursor()),
    ]);
    f.render_widget(
        Paragraph::new(pw_line).style(Style::default().bg(BG)),
        chunks[2],
    );

    f.render_widget(
        Paragraph::new(Span::styled("Enter to confirm, Esc to skip", Theme::dim())),
        chunks[3],
    );
}

fn render_settings_popup(f: &mut Frame, area: Rect, app: &App) {
    let modal_width = 82u16.min(area.width.saturating_sub(4));
    let modal_height = area.height.saturating_sub(4).max(22).min(36) as u16;
    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;
    let modal_area = Rect::new(x, y, modal_width, modal_height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(Theme::primary())
        .title(Span::styled(" Menu ", Theme::primary()))
        .title_bottom(Span::styled(
            " ←→ Switch tab  ↑↓ Navigate  Enter/Space Toggle  Esc Close ",
            Theme::dim(),
        ));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    if inner.height < 4 {
        return;
    }

    // Tab header
    let tab_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let sep_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    let content_area = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );

    let tab_labels = ["  Settings  ", "  Locations  ", "  Keys  ", "  Manual  "];
    let mut tab_spans: Vec<Span> = vec![];
    for (i, &label) in tab_labels.iter().enumerate() {
        let style = if i == app.settings_popup_tab {
            if app.settings_header_selected {
                Theme::tab_active()
            } else {
                Theme::header()
            }
        } else {
            Theme::tab_inactive()
        };
        tab_spans.push(Span::styled(label, style));
        tab_spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(tab_spans)), tab_area);
    f.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(inner.width as usize),
            Theme::dim(),
        )),
        sep_area,
    );

    match app.settings_popup_tab {
        0 => render_popup_settings(f, content_area, app),
        1 => render_popup_locations(f, content_area, app),
        2 => render_popup_keys(f, content_area),
        3 => render_popup_help(f, content_area, app),
        _ => {}
    }
}

fn render_popup_settings(f: &mut Frame, area: Rect, app: &App) {
    // Reserve bottom row for the "press i" hint; content area is everything above it.
    let hint_area  = Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1);
    let body_area  = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));

    f.render_widget(
        Paragraph::new(Span::styled(
            "  Press i for setting info",
            Style::default().fg(FG4),
        )),
        hint_area,
    );

    // 21 content rows: 4 section headers, 13 settings, 4 blank separators.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Security header           -> chunks[0]
            Constraint::Length(1), // row 0: require_unlock     -> chunks[1]
            Constraint::Length(1), // blank                     -> chunks[2]
            Constraint::Length(1), // Search header             -> chunks[3]
            Constraint::Length(1), // row 1: fuzzy              -> chunks[4]
            Constraint::Length(1), // row 2: content            -> chunks[5]
            Constraint::Length(1), // row 3: target             -> chunks[6]
            Constraint::Length(1), // row 4: case               -> chunks[7]
            Constraint::Length(1), // row 5: mode               -> chunks[8]
            Constraint::Length(1), // blank                     -> chunks[9]
            Constraint::Length(1), // Display header            -> chunks[10]
            Constraint::Length(1), // row 6: status_bar expand  -> chunks[11]
            Constraint::Length(1), // row 7: editor             -> chunks[12]
            Constraint::Length(1), // row 8: location filter    -> chunks[13]
            Constraint::Length(1), // row 9: status_bar vis     -> chunks[14]
            Constraint::Length(1), // row 10: controls visible  -> chunks[15]
            Constraint::Length(1), // blank                     -> chunks[16]
            Constraint::Length(1), // Index header              -> chunks[17]
            Constraint::Length(1), // row 11: rebuild           -> chunks[18]
            Constraint::Length(1), // row 12: clear index       -> chunks[19]
            Constraint::Min(0),
        ])
        .split(body_area);

    let active = if app.settings_popup_tab == 0 && !app.settings_header_selected {
        app.settings_popup_selected
    } else {
        usize::MAX
    };

    // Section headers and blank rows
    f.render_widget(Paragraph::new(Span::styled("  Security", Theme::header())), chunks[0]);
    f.render_widget(Paragraph::new(""), chunks[2]);
    f.render_widget(Paragraph::new(Span::styled("  Search", Theme::header())), chunks[3]);
    f.render_widget(Paragraph::new(""), chunks[9]);
    f.render_widget(Paragraph::new(Span::styled("  Display", Theme::header())), chunks[10]);
    f.render_widget(Paragraph::new(""), chunks[16]);
    let index_header = format!("  Index  ({} files)", app.index_total_count);
    f.render_widget(Paragraph::new(Span::styled(index_header, Theme::header())), chunks[17]);

    // Setting rows — pill_row(label, shortcut, &[(val, is_active)], selected)
    let has_ssh_plain = !app.config.settings.require_unlock_on_startup
        && app.config.locations.iter().any(|l| l.kind == "ssh" && app.credentials.contains_key(&l.id));
    let encrypt_label = if has_ssh_plain { "Encrypt credentials ⚠" } else { "Encrypt credentials" };
    f.render_widget(Paragraph::new(pill_row(encrypt_label, "", &[
        ("OFF", !app.config.settings.require_unlock_on_startup),
        ("ON",  app.config.settings.require_unlock_on_startup),
    ], active == 0)), chunks[1]);

    f.render_widget(Paragraph::new(pill_row("Fuzzy match", "^Z", &[
        ("OFF", app.config.settings.consecutive_search),
        ("ON",  !app.config.settings.consecutive_search),
    ], active == 1)), chunks[4]);

    f.render_widget(Paragraph::new(pill_row("Search file content", "^F", &[
        ("OFF", !app.content_search),
        ("ON",  app.content_search),
    ], active == 2)), chunks[5]);

    f.render_widget(Paragraph::new(pill_row("Search target", "^T", &[
        ("files",   app.search_target == SearchTarget::Files),
        ("dirs",    app.search_target == SearchTarget::Folders),
        ("all",     app.search_target == SearchTarget::Both),
        ("content", app.search_target == SearchTarget::Content),
    ], active == 3)), chunks[6]);

    f.render_widget(Paragraph::new(pill_row("Case sensitive", "^A", &[
        ("OFF", !app.config.settings.case_sensitive_search),
        ("ON",  app.config.settings.case_sensitive_search),
    ], active == 4)), chunks[7]);

    f.render_widget(Paragraph::new(pill_row("Search mode", "^S", &[
        ("index", app.search_mode == SearchMode::Index),
        ("live",  app.search_mode == SearchMode::Live),
        ("both",  app.search_mode == SearchMode::Both),
    ], active == 5)), chunks[8]);

    f.render_widget(Paragraph::new(pill_row("Status bar", "^V", &[
        ("collapsed", !app.status_bar_expanded),
        ("expanded",  app.status_bar_expanded),
    ], active == 6)), chunks[11]);

    f.render_widget(Paragraph::new(pill_row("Editor", "", &[
        ("nvim", app.config.settings.editor == "nvim"),
        ("nano", app.config.settings.editor == "nano"),
    ], active == 7)), chunks[12]);

    // Location filter row — shows current value; Enter opens the picker popup
    let loc_label = app.active_location_label();
    f.render_widget(Paragraph::new(action_row_value("Location filter", "^L", loc_label, active == 8)), chunks[13]);

    f.render_widget(Paragraph::new(pill_row("Status bar visible", "^H", &[
        ("OFF", !app.status_bar_visible),
        ("ON",  app.status_bar_visible),
    ], active == 9)), chunks[14]);

    f.render_widget(Paragraph::new(pill_row("Controls visible", "^K", &[
        ("OFF", !app.controls_bar_visible),
        ("ON",  app.controls_bar_visible),
    ], active == 10)), chunks[15]);

    // Action rows (11: rebuild, 12: clear) — Enter/Space triggers them
    f.render_widget(Paragraph::new(action_row("Rebuild index", "^R", active == 11)), chunks[18]);
    f.render_widget(Paragraph::new(action_row("Clear entire index", "", active == 12)), chunks[19]);

    // Location picker popup overlay
    if app.show_location_popup {
        render_location_picker_popup(f, area, app);
    }

    // Info popup overlay (shown when user pressed 'i' on a setting)
    if app.settings_info_popup {
        render_settings_info_popup(f, area, app);
    }
}

/// Build an action row that shows a current value as a plain span (e.g. location filter).
fn action_row_value(label: &str, shortcut: &str, value: &str, selected: bool) -> Line<'static> {
    let cursor_style = if selected {
        Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG)
    };
    Line::from(vec![
        Span::styled(if selected { " ▶ " } else { "   " }, cursor_style),
        Span::styled(format!("{:<24}", label), cursor_style),
        Span::styled(format!("{:<10}", shortcut), Style::default().fg(FG4)),
        Span::styled(format!(" {} ", value), Style::default().fg(AQUA).add_modifier(Modifier::BOLD)),
        Span::styled("  Enter ", Style::default().fg(FG4)),
    ])
}

/// Location picker popup — shown when Enter is pressed on the Location filter row.
fn render_location_picker_popup(f: &mut Frame, _area: Rect, app: &App) {
    let enabled: Vec<&str> = std::iter::once("all")
        .chain(
            app.config.locations.iter()
                .filter(|l| l.enabled)
                .map(|l| l.name.as_str()),
        )
        .collect();
    let total = enabled.len();

    // Max 10 visible rows; popup height = border (2) + items capped at 10
    let max_visible: usize = 10;
    let visible = total.min(max_visible);
    let screen = f.area();
    let popup_w = 40u16.min(screen.width.saturating_sub(4));
    let popup_h = (visible as u16 + 2).min(screen.height.saturating_sub(4));
    let px = screen.x + (screen.width.saturating_sub(popup_w)) / 2;
    let py = screen.y + (screen.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(px, py, popup_w, popup_h);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ORANGE))
        .style(Style::default().bg(BG))
        .title(Span::styled(" Location filter ", Style::default().fg(ORANGE).add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(" ↑↓ select  Enter confirm  Esc cancel ", Style::default().fg(FG4)));

    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.height == 0 { return; }

    let scroll = app.location_popup_scroll;
    let inner_h = inner.height as usize;
    for row in 0..inner_h {
        let idx = scroll + row;
        if idx >= total { break; }
        let name = enabled[idx];
        let is_sel = idx == app.location_popup_selected;
        let (cursor, style) = if is_sel {
            (" ▶ ", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))
        } else {
            ("   ", Style::default().fg(FG))
        };
        let cell_area = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(cursor, style),
                Span::styled(name.to_string(), style),
            ])),
            cell_area,
        );
    }
}

/// Build an action row (no pill values — just a label + shortcut + Enter hint).
fn action_row(label: &str, shortcut: &str, selected: bool) -> Line<'static> {
    let cursor_style = if selected {
        Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG)
    };
    Line::from(vec![
        Span::styled(if selected { " ▶ " } else { "   " }, cursor_style),
        Span::styled(format!("{:<24}", label), cursor_style),
        Span::styled(format!("{:<10}", shortcut), Style::default().fg(FG4)),
        Span::styled(" Enter ", Style::default().fg(BG).bg(AQUA).add_modifier(Modifier::BOLD)),
    ])
}

/// Descriptions shown in the 'i' info popup for each settings item (indexed 0–10).
const SETTING_INFO: &[(&str, &str)] = &[
    ("Encrypt credentials", "When ON, SSH passwords are encrypted with a master password\nyou set on first use. When OFF, passwords are stored as plain\ntext in ~/.config/rummage/credentials.json (mode 600 —\nreadable only by you, but unencrypted on disk).\nChanges take effect on next startup."),
    ("Fuzzy match",          "When ON, letters can be non-adjacent (e.g. 'ac' matches 'abc').\nWhen OFF, letters must appear consecutively\n(e.g. 'abc' matches 'abcdef' but not 'axbxc')."),
    ("Search file content",  "When ON, also searches inside file contents (like grep).\nMuch slower than filename-only search.\n^F toggles this during a search session."),
    ("Search target",     "Controls what appears in results:\n  files — files only\n  dirs  — directories only\n  all   — both files and directories"),
    ("Case sensitive",    "When ON, 'Foo' and 'foo' are treated as different.\nWhen OFF, search ignores case."),
    ("Search mode",       "index — searches the pre-built database (fast)\nlive  — scans the filesystem in real time (slower)\nboth  — runs index and live search simultaneously"),
    ("Status bar",        "collapsed — shows a single summary line\nexpanded  — shows one progress row per location\n^V toggles this at any time."),
    ("Result filter",     "Filters the result list by entry type:\n  all     — show all results\n  files   — show only file results\n  dirs    — show only directory results\n  content — show only content match results"),
    ("Editor",            "The editor opened when pressing Enter on a result.\n  nvim — Neovim\n  nano — GNU nano"),
    ("Location filter",   "Filters results to a single location.\n  all — show results from all locations\n  <name> — show only that location's results\n^L cycles through locations at any time.\nPress Enter here to open the location picker."),
    ("Status bar visible","When OFF, the status bar is hidden entirely.\nWhen ON, the status bar is shown (collapsed or expanded).\n^H toggles this at any time."),
    ("Controls visible",  "When ON, shows the settings row at the top of the\nresults panel and the indicators in the search bar.\nWhen OFF, hides both for a cleaner view.\n^K toggles this at any time."),
    ("Rebuild index",     "Scans all enabled locations and rebuilds the search\nindex database from scratch. Run after adding new\nlocations or when results seem stale. Also: ^R."),
    ("Clear index",       "Removes all indexed files from the database without\nrebuilding. Use to free space or force a clean rebuild.\nPer-location clear is available in the Locations tab (c)."),
];

fn render_settings_info_popup(f: &mut Frame, _area: Rect, app: &App) {
    let idx = app.settings_popup_selected;
    let (name, desc) = SETTING_INFO.get(idx).copied().unwrap_or(("", "No description available."));

    let screen = f.area();
    let popup_w = 60u16.min(screen.width.saturating_sub(4));
    let desc_lines: Vec<&str> = desc.lines().collect();
    let popup_h = (3 + desc_lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let px = screen.x + (screen.width.saturating_sub(popup_w)) / 2;
    let py = screen.y + (screen.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(px, py, popup_w, popup_h);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(AQUA))
        .style(Style::default().bg(BG))
        .title(Span::styled(format!(" {} ", name), Style::default().fg(AQUA).add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(" any key to close ", Style::default().fg(FG4)));

    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.height == 0 { return; }

    let mut lines: Vec<Line> = desc_lines.iter()
        .map(|l| Line::from(Span::styled(format!("  {}", l), Style::default().fg(FG))))
        .collect();
    lines.push(Line::raw(""));

    f.render_widget(Paragraph::new(lines), inner);
}

/// Build a settings pill row: cursor + label + shortcut + side-by-side value pills.
/// The active value is highlighted with an orange background.
fn pill_row(label: &str, shortcut: &str, vals: &[(&str, bool)], selected: bool) -> Line<'static> {
    let cursor_style = if selected {
        Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG)
    };

    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(
            if selected { " ▶ " } else { "   " },
            cursor_style,
        ),
        Span::styled(format!("{:<24}", label), cursor_style),
        Span::styled(format!("{:<10}", shortcut), Style::default().fg(FG4)),
    ];

    for &(val, active) in vals {
        if active {
            spans.push(Span::styled(
                format!(" {} ", val),
                Style::default().fg(BG).bg(YELLOW).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} ", val),
                Style::default().fg(FG4),
            ));
        }
        spans.push(Span::styled(" ", Style::default().fg(FG4)));
    }

    Line::from(spans)
}

fn render_popup_locations(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }

    // Layout: list | [saved msg] | hint
    let has_msg = app.location_saved_msg.is_some();
    let bottom_rows = 1u16 + if has_msg { 1 } else { 0 };
    let hint_area = Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1);
    let msg_area  = Rect::new(area.x, area.y + area.height.saturating_sub(2), area.width, 1);
    let list_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(bottom_rows));

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" a", Theme::primary()),
            Span::styled(" Add  ", Theme::dim()),
            Span::styled("e/Enter", Theme::primary()),
            Span::styled(" Edit  ", Theme::dim()),
            Span::styled("d", Theme::primary()),
            Span::styled(" Delete  ", Theme::dim()),
            Span::styled("Space", Theme::primary()),
            Span::styled(" Enable  ", Theme::dim()),
            Span::styled("c", Theme::primary()),
            Span::styled(" Clear index  ", Theme::dim()),
            Span::styled("Shift+↑↓", Theme::primary()),
            Span::styled(" Move", Theme::dim()),
        ])),
        hint_area,
    );

    if has_msg {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" ✓  {}", app.location_saved_msg.as_deref().unwrap_or("")),
                Theme::success(),
            )),
            msg_area,
        );
    }

    if app.config.locations.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No locations configured. Press 'a' to add one.", Theme::dim())),
            list_area,
        );
        return;
    }

    let index_counts = &app.index_location_counts;
    let rebuilding = app.db_progress.as_ref().map(|p| !p.done).unwrap_or(false);
    let rebuild_spinner = if rebuilding {
        const SPINNERS: &[char] = &['|', '/', '-', '\\'];
        let anim_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as usize;
        SPINNERS[(anim_ms / 100) % SPINNERS.len()]
    } else {
        ' '
    };
    // prefix (2) + "[✓] " (4) = 6 chars before the name
    let prefix_w = 6usize;
    let avail_w = list_area.width as usize;
    let indent = " ".repeat(prefix_w);

    let items: Vec<ListItem> = app.config.locations.iter().enumerate().map(|(i, loc)| {
        let is_sel = i == app.settings_popup_selected && app.settings_popup_tab == 1 && !app.settings_header_selected;
        let prefix = if is_sel { "▶ " } else { "  " };
        let enabled = if loc.enabled {
            Span::styled("[✓] ", Theme::success())
        } else {
            Span::styled("[ ] ", Theme::disabled())
        };
        let name_style = if is_sel { Theme::selected() } else { Theme::normal() };
        let kind = if loc.kind == "ssh" {
            format!(" (ssh: {}@{})", loc.ssh_user.as_deref().unwrap_or("user"), loc.ssh_host.as_deref().unwrap_or("?"))
        } else {
            " (local)".to_string()
        };
        let count = index_counts.get(&loc.id).copied().unwrap_or(0);
        let count_str = if rebuilding {
            format!("  [{} rebuilding…]", rebuild_spinner)
        } else if count > 0 {
            format!("  [{} indexed]", count)
        } else {
            "  [not indexed]".to_string()
        };

        // Check if everything fits on one line.
        let tail = format!("  {}{}{}", loc.path, kind, count_str);
        let total_w = prefix_w + loc.name.chars().count() + tail.chars().count();
        if total_w <= avail_w {
            ListItem::new(Line::from(vec![
                Span::styled(prefix, name_style),
                enabled,
                Span::styled(loc.name.clone(), name_style),
                Span::styled(tail, Theme::dim()),
            ]))
        } else {
            // Name on the first line; path/kind/count on a continuation line indented
            // to align with the start of the name.
            let line1 = Line::from(vec![
                Span::styled(prefix, name_style),
                enabled,
                Span::styled(loc.name.clone(), name_style),
            ]);
            let line2 = Line::from(vec![
                Span::styled(format!("{}{}", indent, tail.trim_start()), Theme::dim()),
            ]);
            ListItem::new(ratatui::text::Text::from(vec![line1, line2]))
        }
    }).collect();

    let mut state = ListState::default();
    state.select(Some(app.settings_popup_selected));
    let list = List::new(items)
        .highlight_style(Style::default())
        .highlight_symbol("");
    f.render_stateful_widget(list, list_area, &mut state);
}

fn render_popup_keys(f: &mut Frame, area: Rect) {
    let sections: &[(&str, &[(&str, &str)])] = &[
        ("Search", &[
            ("↑↓ / PgUp/Dn", "Navigate results"),
            ("Enter", "Open selected file"),
            ("Esc", "Clear search"),
            ("Tab", "Open menu"),
            ("^N", "Show file info popup"),
            ("^Q", "Quit"),
        ]),
        ("Search Options", &[
            ("^F", "Toggle content search (grep inside files)"),
            ("^T", "Cycle target: files → dirs → all → content (filters in-place when possible)"),
            ("^S", "Cycle search mode: index → live → both"),
            ("^L", "Cycle location filter: all → each location → all"),
            ("^Z", "Toggle fuzzy matching"),
            ("^A", "Toggle case-sensitive"),
            ("^V", "Toggle expanded status bar (live/both mode only)"),
            ("^H", "Toggle status bar visibility"),
            ("^K", "Toggle controls bar visibility"),
            ("^R", "Rebuild index"),
            ("→ / ^C", "Open file browser at selected result"),
        ]),
        ("Locations (in menu)", &[
            ("a", "Add new location"),
            ("e / Enter", "Edit selected location"),
            ("d", "Delete location"),
            ("Space", "Toggle location enabled/disabled"),
        ]),
    ];

    let mut y = area.y;
    for (section_title, items) in sections.iter() {
        if y >= area.y + area.height {
            break;
        }
        let section_area = Rect::new(area.x, y, area.width, 1);
        f.render_widget(
            Paragraph::new(Span::styled(format!("  {}", section_title), Theme::header())),
            section_area,
        );
        y += 1;
        for (key, desc) in items.iter() {
            if y >= area.y + area.height {
                break;
            }
            let row_area = Rect::new(area.x, y, area.width, 1);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("    {:<18}", key), Theme::primary()),
                    Span::styled(desc.to_string(), Theme::dim()),
                ])),
                row_area,
            );
            y += 1;
        }
        y += 1; // blank between sections
    }
}

fn render_popup_help(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }

    // Reserve 1 col for scrollbar
    let text_w = area.width.saturating_sub(1) as usize;
    let scrollbar_x = area.x + text_w as u16;

    // Pre-wrap so long lines don't overflow the border.
    let all_lines: Vec<String> = crate::help::visual_lines(text_w);
    let total = all_lines.len();
    let visible_h = area.height as usize;
    let scroll = app.settings_popup_selected.min(total.saturating_sub(1));

    let end = (scroll + visible_h).min(total);
    for (i, line) in all_lines[scroll..end].iter().enumerate() {
        let row_area = Rect::new(area.x, area.y + i as u16, text_w as u16, 1);
        let style = if line.starts_with("━") || line.starts_with("─") {
            Theme::dim()
        } else if line.ends_with("(1)") && (line.starts_with("RUMMAGE") || line.starts_with("rum")) {
            Theme::dim()
        } else if !line.starts_with(' ') && !line.is_empty() && line.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            // Section headers: NAME, KEYBOARD SHORTCUTS, WILDCARD SEARCH, etc.
            Theme::header()
        } else if line.starts_with("  ") && !line.starts_with("   ") {
            // Subsection headers and body text at this indent level
            Theme::subsection_header()
        } else {
            Theme::normal()
        };
        f.render_widget(Paragraph::new(Span::styled(line.as_str(), style)), row_area);
    }

    // Scrollbar
    if total > visible_h {
        let track_h = visible_h;
        let thumb_h = ((visible_h * track_h) / total).max(1);
        let max_scroll = total - visible_h;
        let thumb_top = if max_scroll > 0 { (scroll * (track_h - thumb_h)) / max_scroll } else { 0 };
        for row in 0..track_h {
            let ch = if row >= thumb_top && row < thumb_top + thumb_h {
                Span::styled("█", Theme::scrollbar_thumb())
            } else {
                Span::styled("│", Theme::scrollbar_track())
            };
            let bar_area = Rect::new(scrollbar_x, area.y + row as u16, 1, 1);
            f.render_widget(Paragraph::new(Line::from(vec![ch])), bar_area);
        }
    }

    // Hint at bottom right (last visible row)
    if visible_h > 0 {
        let hint = " Enter: open in editor ";
        let hint_x = area.x + (text_w as u16).saturating_sub(hint.len() as u16 + 1);
        let hint_area = Rect::new(hint_x, area.y + area.height.saturating_sub(1), hint.len() as u16 + 1, 1);
        f.render_widget(Paragraph::new(Span::styled(hint, Theme::dim())), hint_area);
    }
}

/// Returns spans for a content preview line, with the first occurrence of `query`
/// highlighted and the visible window centered on that occurrence.
/// If the match is near an edge, extra context is pulled from the other side.
fn content_preview_spans(content: &str, query: &str, max_width: usize, is_selected: bool) -> Vec<Span<'static>> {
    let base = if is_selected { Theme::selected() } else { Theme::dim() };
    let highlight = Theme::highlight();

    if content.is_empty() || max_width == 0 {
        return vec![];
    }

    // Sanitize: replace control characters (tabs, CR, LF, BEL, etc.) with spaces
    // so they don't corrupt the terminal layout or cause unexpected line breaks.
    let sanitized: String = content.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let content = sanitized.as_str();

    let content_chars: Vec<char> = content.chars().collect();
    let content_len = content_chars.len();
    // Window size in chars limited by display width (wide Unicode chars count as 2 columns)
    let win = {
        let mut cols = 0usize;
        let mut cnt = 0usize;
        for &c in &content_chars {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1);
            if cols + cw > max_width { break; }
            cols += cw;
            cnt += 1;
        }
        cnt
    };

    // Case-insensitive char-level search for query
    let query_chars: Vec<char> = query.chars().collect();
    let q_len = query_chars.len();
    let cl: Vec<char> = content_chars.iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let ql: Vec<char> = query_chars.iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();

    let match_pos: Option<(usize, usize)> = if q_len > 0 && q_len <= content_len {
        (0..=content_len.saturating_sub(q_len))
            .find(|&i| cl[i..i + q_len] == ql[..])
            .map(|i| (i, i + q_len))
    } else {
        None
    };

    // Determine window start: center the match, clamp to content bounds.
    // If there aren't enough chars on one side, shift to fill from the other.
    let win_start = if let Some((m_start, m_end)) = match_pos {
        let match_center = m_start + (m_end - m_start) / 2;
        let ideal = match_center.saturating_sub(win / 2);
        if ideal + win > content_len {
            content_len.saturating_sub(win)
        } else {
            ideal
        }
    } else {
        0
    };

    let win_end = (win_start + win).min(content_len);
    // Trim the window by display width from win_start (wide chars may shift the boundary)
    let window: Vec<char> = {
        let mut cols = 0usize;
        content_chars[win_start..win_end].iter().copied()
            .scan(&mut cols, |cols, c| {
                let cw = UnicodeWidthChar::width(c).unwrap_or(1);
                if **cols + cw > max_width { return None; }
                **cols += cw;
                Some(c)
            })
            .collect()
    };

    if let Some((m_start, m_end)) = match_pos {
        let hl_s = m_start.saturating_sub(win_start).min(window.len());
        let hl_e = m_end.saturating_sub(win_start).min(window.len());
        let mut spans = vec![];
        if hl_s > 0 {
            spans.push(Span::styled(window[..hl_s].iter().collect::<String>(), base));
        }
        if hl_e > hl_s {
            spans.push(Span::styled(window[hl_s..hl_e].iter().collect::<String>(), highlight));
        }
        if hl_e < window.len() {
            spans.push(Span::styled(window[hl_e..].iter().collect::<String>(), base));
        }
        spans
    } else {
        vec![Span::styled(window.iter().collect::<String>(), base)]
    }
}
