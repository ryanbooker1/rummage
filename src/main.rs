mod app;
mod cli;
mod config;
mod crypto;
mod db;
mod debug;
mod file_browser;
mod indexer;
mod search;
mod ssh_client;
mod help;
mod ui;

use std::collections::HashMap;
use std::io::{self, stdout};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use clap::Parser;

use app::{
    App, IndexMsg, IndexProgress, InputField, MatchType, NameSearchMsg, Screen, SearchMode,
    SearchMsg, SearchTarget, StatusLevel,
};

/// Returns true when a completed `done` search covers the requested `want`
/// target — meaning the existing result set can be filtered in-place without
/// re-running the search.
fn target_covers(done: SearchTarget, want: SearchTarget) -> bool {
    match (done, want) {
        (a, b) if a == b => true,
        (SearchTarget::Both, SearchTarget::Files)   => true,
        (SearchTarget::Both, SearchTarget::Folders) => true,
        _ => false,
    }
}
use config::{credentials_path, credentials_plain_path, db_path, Config, Location};
use crypto::CredentialStore;

#[tokio::main]
async fn main() -> Result<()> {
    // Restore default SIGPIPE handling so piping output to head/grep/etc. exits
    // cleanly instead of panicking with "failed printing to stdout: Broken pipe".
    #[cfg(unix)]
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }

    // Parse CLI args first so --help / --version / bad-args exit before any I/O.
    let cli_args = cli::Cli::parse();

    // Load config
    let mut config = Config::load().context("Failed to load config")?;
    debug::set_level(config.settings.debug_level);
    debug::log(2, "rummage starting");

    // Open / create DB
    let db_file = db_path();
    {
        let conn = db::open_db(&db_file)?;
        db::create_tables(&conn)?;
    }

    let file_index: Arc<RwLock<Vec<app::IndexedFile>>> = Arc::new(RwLock::new(Vec::new()));

    // CLI mode: load the index synchronously so searches work immediately.
    if cli_args.query.is_some() {
        {
            let conn = db::open_db(&db_file)?;
            let files = db::load_all_files(&conn, &config.locations)?;
            if let Ok(mut idx) = file_index.write() {
                *idx = files;
            }
        }
        // Load credentials
        let credentials = if !config.settings.require_unlock_on_startup {
            crypto::load_plain(&credentials_plain_path()).unwrap_or_default()
        } else {
            HashMap::new()
        };
        cli::run_cli(&cli_args, &config, credentials, file_index).await;
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::process::exit(0);
    }

    // TUI mode: build App immediately (counts seeded from cached config values),
    // then load the full index in the background so the TUI opens instantly.
    let mut app = App::new(config.clone(), file_index.clone());

    {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let file_index_bg = file_index.clone();
        let db_file_bg = db_file.clone();
        let locations_bg = config.locations.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = db::open_db(&db_file_bg) {
                if let Ok(files) = db::load_all_files(&conn, &locations_bg) {
                    if let Ok(mut idx) = file_index_bg.write() {
                        *idx = files;
                    }
                }
            }
            let _ = done_tx.send(());
        });
        app.index_load_done_rx = Some(done_rx);
    }

    // If plain credential storage is enabled, load credentials immediately without prompting
    if !config.settings.require_unlock_on_startup {
        match crypto::load_plain(&credentials_plain_path()) {
            Ok(creds) => {
                app.credentials = creds;
            }
            Err(_) => {} // File doesn't exist yet — that's fine
        }
        // Always consider "unlocked" in non-secure mode (no password protection)
        app.master_password_unlocked = true;
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut app, &mut config, &db_file).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    match result {
        Err(e) if e.to_string() == "quit" => Ok(()),
        other => other,
    }
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    config: &mut Config,
    db_file: &std::path::Path,
) -> Result<()> {
    let mut last_query = String::new();
    let mut content_search_timer: Option<Instant> = None;
    let mut name_search_timer: Option<Instant> = None;
    let mut status_clear_time: Option<Instant> = None;
    let mut trigger_live_phase = false;
    // Background filter: narrows existing results off the render thread.
    let mut filter_rx: Option<tokio::sync::oneshot::Receiver<Vec<app::SearchResult>>> = None;
    loop {
        // Safety clamp: prevent result_scroll from ever exceeding visible results,
        // which would cause visible_height() to return 1 and lock the viewport.
        {
            let vis_len = app.visible_results().len();
            if vis_len == 0 {
                app.result_scroll = 0;
            } else if app.result_scroll >= vis_len {
                app.result_scroll = 0;
            }
        }

        // Sync pending state so the renderer knows a search is imminent
        app.search_pending = name_search_timer.is_some() || content_search_timer.is_some();

        // Render
        terminal.draw(|f| ui::render(f, app))?;

        // Persist the scroll offset that the renderer auto-corrected so that the
        // next event handler sees a consistent scroll/selected relationship.
        // Without this, pane.scroll is never updated when navigating down, causing
        // the viewport to snap when reversing direction.
        if let (Some(fb), Ok(size)) = (&mut app.fb, terminal.size()) {
            let pixel_h = (size.height as usize).saturating_sub(4).max(1);
            for pane in [&mut fb.middle, &mut fb.left, &mut fb.right] {
                let total = pane.entries.len();
                if total == 0 { continue; }
                let s = pane.scroll.min(total.saturating_sub(pixel_h));
                if pane.selected < s {
                    pane.scroll = pane.selected;
                } else if pane.selected >= s + pixel_h {
                    pane.scroll = pane.selected + 1 - pixel_h;
                } else {
                    pane.scroll = s;
                }
            }
        }

        // Check for auto-clear of status message
        if let Some(clear_at) = status_clear_time {
            if Instant::now() >= clear_at {
                app.clear_status();
                status_clear_time = None;
            }
        }

        // Check whether the background index load (deferred from startup) has finished.
        let index_load_just_done = if let Some(rx) = &mut app.index_load_done_rx {
            matches!(rx.try_recv(), Ok(()))
        } else {
            false
        };
        if index_load_just_done {
            app.index_load_done_rx = None;
            app.refresh_index_counts();
            config.index_counts = app.config.index_counts.clone();
            config.save().ok();
            app.config = config.clone();
        }

        // Check for index channel messages — drain into Vec first to avoid borrow issues
        let index_msgs: Vec<IndexMsg> = if let Some(rx) = &mut app.index_rx {
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        } else {
            Vec::new()
        };
        let mut index_done = false;
        for msg in index_msgs {
            match msg {
                IndexMsg::Progress { location_name, current, total, path } => {
                    if let Some(progress) = &mut app.db_progress {
                        progress.current = current;
                        progress.total = total;
                        progress.current_path = path;
                        progress.location_name = location_name;
                        progress.eta_secs = None;
                    }
                }
                IndexMsg::Done => {
                    if let Some(progress) = &mut app.db_progress {
                        progress.done = true;
                    }
                    index_done = true;
                }
                IndexMsg::Error(msg) => {
                    app.set_status(format!("Index error: {}", msg), StatusLevel::Error);
                    status_clear_time = Some(Instant::now() + Duration::from_secs(30));
                    // Extract location name from error message for persistent display
                    let short = if let Some(rest) = msg.strip_prefix("SSH failed for '") {
                        let loc_name = rest.split('\'').next().unwrap_or(&msg).to_string();
                        let err = if msg.contains("key has changed") || msg.contains("man-in-the-middle") {
                            "⚠ HOST KEY MISMATCH — possible attack (see ^N for details)".to_string()
                        } else {
                            msg.split(':').last().unwrap_or("connection failed").trim().to_string()
                        };
                        (loc_name, err)
                    } else {
                        ("unknown".to_string(), msg.chars().take(60).collect())
                    };
                    if !app.location_errors.iter().any(|(n, _)| n == &short.0) {
                        app.location_errors.push(short);
                    }
                }
            }
        }
        if index_done {
            // Reload file index
            let conn = db::open_db(db_file)?;
            let files = db::load_all_files(&conn, &config.locations)?;
            if let Ok(mut idx) = app.file_index.write() {
                *idx = files;
            }
            app.refresh_index_counts();
            config.last_db_build = Some(chrono::Utc::now().to_rfc3339());
            config.index_counts = app.config.index_counts.clone();
            config.save().ok();
            app.config = config.clone();
            app.set_status("Index rebuilt successfully", StatusLevel::Success);
            status_clear_time = Some(Instant::now() + Duration::from_secs(3));
            app.index_rx = None;
            app.db_progress = None;
        }

        // Check for content search channel messages — drain into Vec first
        let search_msgs: Vec<SearchMsg> = if let Some(rx) = &mut app.search_rx {
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        } else {
            Vec::new()
        };
        let mut content_done = false;
        for msg in search_msgs {
            match msg {
                SearchMsg::ContentResult(result) => {
                    // Deduplicate against name results — a file already found
                    // by filename should not appear again as a content match.
                    let key = format!("{}:{}", result.file.location_id, result.file.path);
                    if !app.seen_result_keys.contains(&key) {
                        app.seen_result_keys.insert(key);
                        app.results.push(result);
                        app.refresh_visible_count();
                    }
                }
                SearchMsg::ContentLocationProgress { loc_id, count, current_dir } => {
                    app.location_search_states.insert(loc_id, (count, current_dir));
                }
                SearchMsg::ContentLocationDone { loc_id } => {
                    app.location_search_states.insert(loc_id, (0, "Complete".to_string()));
                }
                SearchMsg::ContentDone => {
                    content_done = true;
                    app.content_search_active = false;
                    if app.search_target == SearchTarget::Content {
                        app.completed_search_target = Some(SearchTarget::Content);
                    }
                    app.location_search_states.clear();
                    app.refresh_visible_count();
                    let name_count = app.results.iter().filter(|r| matches!(r.match_type, MatchType::Name)).count();
                    let content_count = app.results.iter().filter(|r| matches!(r.match_type, MatchType::Content)).count();
                    app.search_status = if content_count > 0 {
                        format!("{} name + {} content results", name_count, content_count)
                    } else {
                        format!("{} results", name_count)
                    };
                    let vis_len = app.visible_count;
                    if vis_len == 0 {
                        app.selected = 0;
                        app.result_scroll = 0;
                    } else if app.selected >= vis_len {
                        app.selected = vis_len - 1;
                        app.result_scroll = 0;
                    }
                }
            }
        }
        if content_done {
            app.search_rx = None;
            app.search_cancel_tx = None;
        }

        // Check for name search channel messages
        let name_msgs: Vec<NameSearchMsg> = if let Some(rx) = &mut app.name_search_rx {
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
                if msgs.len() >= 64 { break; } // cap per-frame to keep UI responsive
            }
            msgs
        } else {
            Vec::new()
        };
        for msg in name_msgs {
            match msg {
                NameSearchMsg::Batch(results) => {
                    for r in results {
                        let key = format!("{}:{}", r.file.location_id, r.file.path);
                        if app.seen_result_keys.contains(&key) {
                            // Already in results from index — record live confirmation (O(1)).
                            // The [S] badge is applied during rendering via live_confirmed_keys.
                            if !r.is_indexed {
                                app.live_confirmed_keys.insert(key);
                            }
                        } else {
                            // If this is a live result not in the index, queue it for DB write
                            if !r.is_indexed {
                                app.pending_index_writes.push(r.file.clone());
                                // Also add to in-memory index
                                if let Ok(mut idx) = app.file_index.write() {
                                    idx.push(r.file.clone());
                                }
                            }
                            app.seen_result_keys.insert(key);
                            // Insert in descending-score order so the list stays sorted
                            // throughout the search and the final sort is a no-op.
                            let pos = app.results.partition_point(|x| x.score >= r.score);
                            app.results.insert(pos, r);
                        }
                    }
                    app.refresh_visible_count();
                    app.search_status = format!("Searching… ({} results)", app.results.len());
                }
                NameSearchMsg::Searching { loc_id, count, current_dir } => {
                    app.location_search_states.insert(loc_id, (count, current_dir.clone()));
                    app.name_search_status = Some(format!("live: {}…", current_dir));
                    app.search_status = format!("Searching… ({} results)", app.results.len());
                }
                NameSearchMsg::IndexProgress { processed, total } => {
                    app.index_search_progress = Some((processed, total));
                }
                NameSearchMsg::Error(msg) => {
                    // Extract location name from "SSH connect failed for 'X'" or similar
                    let loc_name = msg.split('\'').nth(1).unwrap_or("SSH").to_string();
                    let short_err = if msg.contains("key has changed") || msg.contains("man-in-the-middle") {
                        "⚠ HOST KEY MISMATCH — possible attack (see ^N for details)".to_string()
                    } else {
                        msg.split(':').last().unwrap_or(&msg).trim().to_string()
                    };
                    if !app.location_errors.iter().any(|(n, _)| n == &loc_name) {
                        app.location_errors.push((loc_name, short_err));
                    }
                    app.set_status(msg, StatusLevel::Error);
                    status_clear_time = Some(Instant::now() + Duration::from_secs(15));
                }
                NameSearchMsg::Done { loc_id } => {
                    app.name_search_cancel_txs.remove(&loc_id);

                    // Mark this location as complete for smart ^L filtering.
                    // For indexed search (loc_id == ""), mark all scoped enabled locations —
                    // but only when the indexed phase is truly final (not followed by live phase).
                    if loc_id.is_empty() {
                        if !app.both_mode_pending_live {
                            let filter = app.active_location.clone();
                            let locs: Vec<String> = app.config.locations.iter()
                                .filter(|l| l.enabled)
                                .filter(|l| filter.as_ref().map_or(true, |id| &l.id == id))
                                .map(|l| l.id.clone())
                                .collect();
                            for id in locs { app.completed_locations.insert(id); }
                        }
                    } else {
                        app.completed_locations.insert(loc_id);
                    }

                    if app.name_search_pending_sources > 0 {
                        app.name_search_pending_sources -= 1;
                    }
                    if app.name_search_pending_sources == 0 {
                        if app.both_mode_pending_live {
                            app.both_mode_pending_live = false;
                            trigger_live_phase = true;
                        } else {
                            finalize_name_search(app, db_file);
                        }
                    }
                }
            }
        }

        // Apply background filter result if ready
        if let Some(rx) = filter_rx.as_mut() {
            match rx.try_recv() {
                Ok(filtered) => {
                    app.results = filtered;
                    app.seen_result_keys.clear();
                    app.live_confirmed_keys.clear();
                    for r in &app.results {
                        app.seen_result_keys.insert(format!("{}:{}", r.file.location_id, r.file.path));
                    }
                    app.refresh_visible_count();
                    let vis_len = app.visible_count;
                    if vis_len == 0 {
                        app.selected = 0;
                        app.result_scroll = 0;
                    } else if app.selected >= vis_len {
                        app.selected = vis_len - 1;
                        app.result_scroll = 0;
                    }
                    filter_rx = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                Err(_) => { filter_rx = None; }
            }
        }

        // Check for file browser channel messages
        let fb_msgs: Vec<app::FbMsg> = if let Some(rx) = &mut app.fb_rx {
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        } else {
            Vec::new()
        };
        for msg in fb_msgs {
            match msg {
                app::FbMsg::Entries { pane, path, entries } => {
                    if let Some(fb) = &mut app.fb {
                        match pane {
                            app::FbPaneTarget::Left => {
                                // Don't overwrite the virtual locations pane with filesystem data.
                                if fb.left.path == "__locations__" { }
                                else {
                                    // Highlight the entry that matches middle.path's basename
                                    let mid_name = std::path::Path::new(&fb.middle.path)
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    let sel = entries.iter().position(|e| e.name == mid_name).unwrap_or(0);
                                    fb.left = app::FbPane { path, entries, selected: sel, scroll: 0, loading: false };
                                }
                            }
                            app::FbPaneTarget::Middle => {
                                let sel = if let Some(ref focus) = fb.initial_focus.clone() {
                                    entries.iter().position(|e| &e.name == focus).unwrap_or(0)
                                } else {
                                    0
                                };
                                fb.middle = app::FbPane { path, entries, selected: sel, scroll: 0, loading: false };
                                // Trigger right pane load for selected entry
                                trigger_fb_right(fb, &app.config, &app.credentials);
                            }
                            app::FbPaneTarget::Right => {
                                fb.right = app::FbPane { path, entries, selected: 0, scroll: 0, loading: false };
                            }
                        }
                    }
                }
                app::FbMsg::FileContent { path, lines } => {
                    if let Some(fb) = &mut app.fb {
                        if fb.right_content_path == path {
                            fb.right_lines = lines;
                            fb.right_lines_path = path;
                            fb.right_content_loading = false;
                        }
                    }
                }
                app::FbMsg::SshStat { info, .. } => {
                    app.fb_info_ssh_stat = Some(info);
                }
                app::FbMsg::Error(msg) => {
                    if let Some(fb) = &mut app.fb {
                        fb.right_content_loading = false;
                    }
                    app.set_status(format!("File browser: {}", msg), StatusLevel::Error);
                    status_clear_time = Some(Instant::now() + Duration::from_secs(5));
                }
            }
        }

        // Trigger live phase for Both mode (after indexed phase completes)
        if trigger_live_phase {
            trigger_live_phase = false;
            if let Some(tx) = app.name_search_tx.clone() {
                launch_live_searches(app, config, &tx);
                if app.name_search_pending_sources == 0 {
                    // No live locations — finalize now
                    finalize_name_search(app, db_file);
                }
            }
        }

        // Content search debounce
        if let Some(timer) = content_search_timer {
            if Instant::now().duration_since(timer) >= Duration::from_millis(300) {
                content_search_timer = None;
                let content_mode = app.search_target == SearchTarget::Content;
                if (app.content_search || content_mode) && app.query.len() >= 2 {
                    start_content_search(app, config).await;
                }
            }
        }

        // Name search debounce
        if let Some(timer) = name_search_timer {
            if timer.elapsed() >= Duration::from_millis(250) {
                name_search_timer = None;
                if app.query.len() >= 2 {
                    if filter_rx.take().is_some() {
                        // Filter hadn't fired yet — the snapshot was for the old query.
                        // Clear results so we start fresh rather than mixing stale hits
                        // with new ones (stale hits would survive finalize since it
                        // sorts+truncates but does not re-filter).
                        app.results.clear();
                        app.seen_result_keys.clear();
                        app.live_confirmed_keys.clear();
                    }
                    // If filter_rx was already None it already fired and app.results
                    // holds a correctly pre-filtered starting set — keep it as a head start.
                    start_name_search(app, config).await;
                }
            }
        }

        // Poll for keyboard events with 10ms timeout
        if !event::poll(Duration::from_millis(10))? {
            continue;
        }

        let evt = event::read()?;
        let Event::Key(key) = evt else { continue };

        // Global quit — works from any screen or popup
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Err(anyhow::anyhow!("quit"));
        }

        // Global debug overlay toggle (^G)
        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            app.show_debug_overlay = !app.show_debug_overlay;
            continue;
        }


        // F1 shows file info popup (from search results or file browser).
        // Settings popup is opened via Tab from the search screen.
        // Handled per-context below — this comment marks where it was global.

        // Settings popup intercepts all other keys when visible
        if app.show_settings_popup {
            // Delete confirm overlay intercepts keys before anything else in the popup
            if app.delete_confirm.is_some() {
                match key.code {
                    KeyCode::Enter => {
                        // Show spinner/greyed controls immediately before the blocking DB work.
                        app.location_deleting = true;
                        let _ = terminal.draw(|f| ui::render(f, app));

                        if let Some(id) = app.delete_confirm.take() {
                            config.locations.retain(|l| l.id != id);
                            app.config = config.clone();
                            config.save().ok();
                            if let Ok(conn) = db::open_db(db_file) {
                                db::delete_location_files(&conn, &id).ok();
                            }
                            if let Ok(conn) = db::open_db(db_file) {
                                if let Ok(files) = db::load_all_files(&conn, &config.locations) {
                                    if let Ok(mut idx) = app.file_index.write() {
                                        *idx = files;
                                    }
                                    app.refresh_index_counts();
                                }
                            }
                            app.location_deleting = false;
                            app.location_saved_msg = Some("Location deleted".to_string());
                            if app.settings_popup_selected >= config.locations.len() && !config.locations.is_empty() {
                                app.settings_popup_selected = config.locations.len() - 1;
                            }
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                        app.delete_confirm = None;
                    }
                    _ => {}
                }
                continue;
            }

            // Info sub-popup: any key closes it
            if app.settings_info_popup {
                app.settings_info_popup = false;
                continue;
            }

            // Location picker popup intercepts keys before the rest of settings handling
            if app.show_location_popup {
                let enabled_ids: Vec<Option<String>> = std::iter::once(None)
                    .chain(
                        app.config.locations.iter()
                            .filter(|l| l.enabled)
                            .map(|l| Some(l.id.clone())),
                    )
                    .collect();
                let total = enabled_ids.len();
                match key.code {
                    KeyCode::Esc => { app.show_location_popup = false; }
                    KeyCode::Up => {
                        if app.location_popup_selected > 0 {
                            app.location_popup_selected -= 1;
                            if app.location_popup_selected < app.location_popup_scroll {
                                app.location_popup_scroll = app.location_popup_selected;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if app.location_popup_selected + 1 < total {
                            app.location_popup_selected += 1;
                            let visible = 10usize;
                            if app.location_popup_selected >= app.location_popup_scroll + visible {
                                app.location_popup_scroll = app.location_popup_selected + 1 - visible;
                            }
                        }
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        app.active_location = enabled_ids.get(app.location_popup_selected).cloned().flatten();
                        app.show_location_popup = false;
                        apply_location_change(app, db_file, &mut name_search_timer, &mut filter_rx);
                    }
                    _ => {}
                }
                continue;
            }

            // Ctrl shortcuts: jump cursor to the relevant row but let the key
            // fall through to handle_search_input so the action fires normally.
            let ctrl_row: Option<usize> = if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Char('f') => Some(2),  // content search
                    KeyCode::Char('z') => Some(1),  // consecutive/fuzzy
                    KeyCode::Char('a') => Some(4),  // case sensitive
                    KeyCode::Char('t') => Some(3),  // search target
                    KeyCode::Char('s') => Some(5),  // search mode
                    KeyCode::Char('x') => Some(7),  // result filter
                    KeyCode::Char('v') => Some(6),  // status bar expand
                    KeyCode::Char('r') => Some(12), // rebuild
                    KeyCode::Char('l') => Some(9),  // location filter
                    KeyCode::Char('h') => Some(10), // status bar visible
                    KeyCode::Char('k') => Some(11), // controls bar visible
                    _ => None,
                }
            } else {
                None
            };
            if let Some(row) = ctrl_row {
                app.settings_popup_tab = 0;
                app.settings_popup_selected = row;
                app.settings_header_selected = false;
                // Fall through to handle_search_input — no `continue`
            } else {

            // Any keypress in the locations tab clears the saved message.
            if app.settings_popup_tab == 1 {
                app.location_saved_msg = None;
            }

            match key.code {
                KeyCode::Esc => {
                    app.show_settings_popup = false;
                    app.settings_info_popup = false;
                }
                KeyCode::Tab => {
                    // Cycle through tabs: 0 → 1 → 2 → 3 → 0
                    app.settings_popup_tab = (app.settings_popup_tab + 1) % 4;
                    app.settings_popup_selected = 0;
                    app.settings_header_selected = true;
                }
                KeyCode::Left | KeyCode::Right => {
                    if app.settings_header_selected {
                        // Header selected: Left/Right change tabs
                        if key.code == KeyCode::Left {
                            app.settings_popup_tab = app.settings_popup_tab.saturating_sub(1);
                        } else {
                            app.settings_popup_tab = (app.settings_popup_tab + 1).min(3);
                        }
                        app.settings_popup_selected = 0;
                    } else if app.settings_popup_tab == 0 {
                        // Left = previous/off, Right = next/on for the highlighted setting
                        let fwd = key.code == KeyCode::Right;
                        match app.settings_popup_selected {
                            0 => {
                                let new_val = fwd;
                                if new_val != config.settings.require_unlock_on_startup {
                                    let was_on = config.settings.require_unlock_on_startup;
                                    config.settings.require_unlock_on_startup = new_val;
                                    app.config = config.clone();
                                    config.save().ok();
                                    if app.master_password_unlocked && !app.credentials.is_empty() {
                                        if was_on {
                                            // Turning OFF: write plain, delete encrypted
                                            crypto::save_plain(&app.credentials, &credentials_plain_path()).ok();
                                            std::fs::remove_file(credentials_path()).ok();
                                        } else {
                                            // Turning ON: write encrypted, delete plain only if .enc was created
                                            if save_credentials(app, config).is_ok() && credentials_path().exists() {
                                                std::fs::remove_file(credentials_plain_path()).ok();
                                            }
                                        }
                                    }
                                }
                            }
                            1 => {
                                config.settings.consecutive_search = fwd;
                                app.config = config.clone();
                                config.save().ok();
                                if app.query.len() >= 2 {
                                    filter_rx = None;
                                    for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                    app.name_search_cancel_txs.clear();
                                    app.seen_result_keys.clear();
                                    app.live_confirmed_keys.clear();
                                    app.results.clear();
                                    app.reset_search();
                                    app.name_search_active = true;
                                    name_search_timer = Some(Instant::now());
                                }
                            }
                            2 => {
                                app.content_search = fwd;
                                config.settings.content_search = fwd;
                                app.config = config.clone();
                                config.save().ok();
                                if app.content_search && app.query.len() >= 2 {
                                    content_search_timer = Some(Instant::now());
                                } else if !app.content_search {
                                    if let Some(tx) = &app.search_cancel_tx {
                                        tx.send(()).await.ok();
                                    }
                                    app.search_rx = None;
                                    app.search_cancel_tx = None;
                                    app.content_search_active = false;
                                    app.results.retain(|r| matches!(r.match_type, MatchType::Name));
                                }
                            }
                            3 => {
                                app.search_target = if fwd { app.search_target.cycle() } else { app.search_target.cycle_back() };
                                config.settings.search_target = app.search_target.label().to_string();
                                app.config = config.clone();
                                config.save().ok();
                                if app.query.len() >= 2 {
                                    filter_rx = None;
                                    for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                    app.name_search_cancel_txs.clear();
                                    app.seen_result_keys.clear();
                                    app.live_confirmed_keys.clear();
                                    app.results.clear();
                                    app.reset_search();
                                    if app.search_target == SearchTarget::Content {
                                        content_search_timer = Some(Instant::now());
                                    } else {
                                        app.name_search_active = true;
                                        name_search_timer = Some(Instant::now());
                                        if app.content_search {
                                            content_search_timer = Some(Instant::now());
                                        }
                                    }
                                }
                            }
                            4 => {
                                config.settings.case_sensitive_search = fwd;
                                app.config = config.clone();
                                config.save().ok();
                                if app.query.len() >= 2 {
                                    filter_rx = None;
                                    for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                    app.name_search_cancel_txs.clear();
                                    app.seen_result_keys.clear();
                                    app.live_confirmed_keys.clear();
                                    app.results.clear();
                                    app.reset_search();
                                    app.name_search_active = true;
                                    name_search_timer = Some(Instant::now());
                                }
                            }
                            5 => {
                                app.search_mode = if fwd { app.search_mode.cycle() } else { app.search_mode.cycle_back() };
                                config.settings.search_mode = app.search_mode.label().to_string();
                                app.config = config.clone();
                                config.save().ok();
                                if app.query.len() >= 2 {
                                    filter_rx = None;
                                    for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                    app.name_search_cancel_txs.clear();
                                    app.seen_result_keys.clear();
                                    app.live_confirmed_keys.clear();
                                    app.results.clear();
                                    app.reset_search();
                                    app.name_search_active = true;
                                    name_search_timer = Some(Instant::now());
                                }
                            }
                            6 => {
                                app.status_bar_expanded = fwd;
                                config.settings.status_bar_expanded = fwd;
                                app.config = config.clone();
                                config.save().ok();
                            }
                            7 => {
                                config.settings.editor = if fwd { "nano".to_string() } else { "nvim".to_string() };
                                app.config = config.clone();
                                config.save().ok();
                            }
                            8 => {
                                // Open location picker popup on any directional key
                                let enabled_len = app.config.locations.iter().filter(|l| l.enabled).count();
                                if enabled_len > 0 {
                                    // Pre-select current filter position
                                    let enabled_ids: Vec<Option<String>> = std::iter::once(None)
                                        .chain(app.config.locations.iter().filter(|l| l.enabled).map(|l| Some(l.id.clone())))
                                        .collect();
                                    app.location_popup_selected = enabled_ids.iter().position(|id| id == &app.active_location).unwrap_or(0);
                                    app.location_popup_scroll = 0;
                                    app.show_location_popup = true;
                                }
                            }
                            9 => {
                                app.status_bar_visible = fwd;
                                config.settings.status_bar_visible = fwd;
                                app.config = config.clone();
                                config.save().ok();
                            }
                            10 => {
                                app.controls_bar_visible = fwd;
                                config.settings.controls_bar_visible = fwd;
                                app.config = config.clone();
                                config.save().ok();
                            }
                            // 11 = rebuild, 12 = clear: actions triggered only by Enter/Space
                            _ => {}
                        }
                    }
                }
                KeyCode::Up => {
                    if app.settings_popup_tab == 1 && key.modifiers.contains(KeyModifiers::SHIFT) {
                        // Shift+Up: move selected location up in the list
                        let idx = app.settings_popup_selected;
                        if idx > 0 && config.locations.len() > 1 {
                            config.locations.swap(idx, idx - 1);
                            app.settings_popup_selected -= 1;
                            app.config = config.clone();
                            config.save().ok();
                        }
                    } else if app.settings_popup_tab >= 2 {
                        // Keys/Manual: header always selected, Up/Down scroll content
                        app.settings_popup_selected = app.settings_popup_selected.saturating_sub(1);
                    } else if app.settings_header_selected {
                        // Already at the header; nothing above it
                    } else if app.settings_popup_selected > 0 {
                        app.settings_popup_selected -= 1;
                    } else {
                        // At the top row — move focus up to the tab header
                        app.settings_header_selected = true;
                    }
                }
                KeyCode::Down => {
                    if app.settings_popup_tab == 1 && key.modifiers.contains(KeyModifiers::SHIFT) {
                        // Shift+Down: move selected location down in the list
                        let idx = app.settings_popup_selected;
                        if idx + 1 < config.locations.len() {
                            config.locations.swap(idx, idx + 1);
                            app.settings_popup_selected += 1;
                            app.config = config.clone();
                            config.save().ok();
                        }
                    } else if app.settings_popup_tab >= 2 {
                        // Keys/Manual: header always selected, Up/Down scroll content
                        let max = if app.settings_popup_tab == 3 {
                            let sz = terminal.size().unwrap_or_default();
                            let modal_w = 82u16.min(sz.width.saturating_sub(4));
                            let text_w = modal_w.saturating_sub(3) as usize; // borders + scrollbar
                            let term_h = sz.height as usize;
                            let visible_h = term_h.saturating_sub(4).max(22).min(36).saturating_sub(4);
                            help::visual_lines(text_w).len().saturating_sub(visible_h)
                        } else {
                            0
                        };
                        if app.settings_popup_selected < max {
                            app.settings_popup_selected += 1;
                        }
                    } else if app.settings_header_selected {
                        // Move focus from header down to first row
                        app.settings_header_selected = false;
                        app.settings_popup_selected = 0;
                    } else {
                        let max = match app.settings_popup_tab {
                            0 => 12usize,
                            1 => config.locations.len().saturating_sub(1),
                            _ => 0,
                        };
                        if app.settings_popup_selected < max {
                            app.settings_popup_selected += 1;
                        }
                    }
                }
                KeyCode::PageDown if app.settings_popup_tab >= 2 => {
                    let max = if app.settings_popup_tab == 3 {
                        let sz = terminal.size().unwrap_or_default();
                        let modal_w = 82u16.min(sz.width.saturating_sub(4));
                        let text_w = modal_w.saturating_sub(3) as usize;
                        let term_h = sz.height as usize;
                        let visible_h = term_h.saturating_sub(4).max(22).min(36).saturating_sub(4);
                        help::visual_lines(text_w).len().saturating_sub(visible_h)
                    } else {
                        0
                    };
                    app.settings_popup_selected = (app.settings_popup_selected + 20).min(max);
                }
                KeyCode::PageUp if app.settings_popup_tab >= 2 => {
                    app.settings_popup_selected = app.settings_popup_selected.saturating_sub(20);
                }
                KeyCode::Char('i') if app.settings_popup_tab == 0 && !app.settings_header_selected => {
                    app.settings_info_popup = true;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    match app.settings_popup_tab {
                        0 => {
                            match app.settings_popup_selected {
                                0 => {
                                    let was_on = config.settings.require_unlock_on_startup;
                                    config.settings.require_unlock_on_startup = !was_on;
                                    app.config = config.clone();
                                    config.save().ok();
                                    if app.master_password_unlocked && !app.credentials.is_empty() {
                                        if was_on {
                                            crypto::save_plain(&app.credentials, &credentials_plain_path()).ok();
                                            std::fs::remove_file(credentials_path()).ok();
                                        } else {
                                            if save_credentials(app, config).is_ok() && credentials_path().exists() {
                                                std::fs::remove_file(credentials_plain_path()).ok();
                                            }
                                        }
                                    }
                                }
                                1 => {
                                    config.settings.consecutive_search = !config.settings.consecutive_search;
                                    app.config = config.clone();
                                    config.save().ok();
                                    if app.query.len() >= 2 {
                                        filter_rx = None;
                                        for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                        app.name_search_cancel_txs.clear();
                                        app.seen_result_keys.clear();
                                        app.live_confirmed_keys.clear();
                                        app.results.clear();
                                        app.reset_search();
                                        app.name_search_active = true;
                                        name_search_timer = Some(Instant::now());
                                    }
                                }
                                2 => {
                                    app.content_search = !app.content_search;
                                    config.settings.content_search = app.content_search;
                                    app.config = config.clone();
                                    config.save().ok();
                                    if app.content_search && app.query.len() >= 2 {
                                        content_search_timer = Some(Instant::now());
                                    } else if !app.content_search {
                                        if let Some(tx) = &app.search_cancel_tx {
                                            tx.send(()).await.ok();
                                        }
                                        app.search_rx = None;
                                        app.search_cancel_tx = None;
                                        app.content_search_active = false;
                                        app.results.retain(|r| matches!(r.match_type, MatchType::Name));
                                    }
                                }
                                3 => {
                                    app.search_target = app.search_target.cycle();
                                    config.settings.search_target = app.search_target.label().to_string();
                                    app.config = config.clone();
                                    config.save().ok();
                                    let search_active = app.name_search_active || app.content_search_active;
                                    let can_filter = !search_active
                                        && app.completed_search_target
                                            .map_or(false, |done| target_covers(done, app.search_target));
                                    if can_filter {
                                        let vis_len = app.visible_results().len();
                                        if vis_len == 0 { app.selected = 0; app.result_scroll = 0; }
                                        else if app.selected >= vis_len { app.selected = vis_len - 1; }
                                    } else if app.query.len() >= 2 {
                                        filter_rx = None;
                                        for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                        app.name_search_cancel_txs.clear();
                                        app.seen_result_keys.clear();
                                        app.live_confirmed_keys.clear();
                                        app.results.clear();
                                        app.reset_search();
                                        app.completed_search_target = None;
                                        if app.search_target == SearchTarget::Content {
                                            content_search_timer = Some(Instant::now());
                                        } else {
                                            app.name_search_active = true;
                                            name_search_timer = Some(Instant::now());
                                            if app.content_search {
                                                content_search_timer = Some(Instant::now());
                                            }
                                        }
                                    }
                                }
                                4 => {
                                    config.settings.case_sensitive_search = !config.settings.case_sensitive_search;
                                    app.config = config.clone();
                                    config.save().ok();
                                    if app.query.len() >= 2 {
                                        filter_rx = None;
                                        for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                        app.name_search_cancel_txs.clear();
                                        app.seen_result_keys.clear();
                                        app.live_confirmed_keys.clear();
                                        app.results.clear();
                                        app.reset_search();
                                        app.name_search_active = true;
                                        name_search_timer = Some(Instant::now());
                                    }
                                }
                                5 => {
                                    app.search_mode = app.search_mode.cycle();
                                    config.settings.search_mode = app.search_mode.label().to_string();
                                    app.config = config.clone();
                                    config.save().ok();
                                    if app.query.len() >= 2 {
                                        filter_rx = None;
                                        for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                                        app.name_search_cancel_txs.clear();
                                        app.seen_result_keys.clear();
                                        app.live_confirmed_keys.clear();
                                        app.results.clear();
                                        app.reset_search();
                                        app.name_search_active = true;
                                        name_search_timer = Some(Instant::now());
                                    }
                                }
                                6 => {
                                    app.status_bar_expanded = !app.status_bar_expanded;
                                    config.settings.status_bar_expanded = app.status_bar_expanded;
                                    app.config = config.clone();
                                    config.save().ok();
                                }
                                7 => {
                                    config.settings.editor = if config.settings.editor == "nvim" {
                                        "nano".to_string()
                                    } else {
                                        "nvim".to_string()
                                    };
                                    app.config = config.clone();
                                    config.save().ok();
                                }
                                8 => {
                                    // Open location picker popup
                                    let enabled_ids: Vec<Option<String>> = std::iter::once(None)
                                        .chain(app.config.locations.iter().filter(|l| l.enabled).map(|l| Some(l.id.clone())))
                                        .collect();
                                    app.location_popup_selected = enabled_ids.iter().position(|id| id == &app.active_location).unwrap_or(0);
                                    app.location_popup_scroll = 0;
                                    app.show_location_popup = true;
                                }
                                9 => {
                                    app.status_bar_visible = !app.status_bar_visible;
                                    config.settings.status_bar_visible = app.status_bar_visible;
                                    app.config = config.clone();
                                    config.save().ok();
                                }
                                10 => {
                                    app.controls_bar_visible = !app.controls_bar_visible;
                                    config.settings.controls_bar_visible = app.controls_bar_visible;
                                    app.config = config.clone();
                                    config.save().ok();
                                }
                                11 => {
                                    // Rebuild index
                                    app.show_settings_popup = false;
                                    start_rebuild(app, config, db_file).await;
                                }
                                12 => {
                                    // Clear entire index
                                    if let Ok(conn) = db::open_db(db_file) {
                                        if db::clear_all_files(&conn).is_ok() {
                                            if let Ok(mut idx) = app.file_index.write() {
                                                idx.clear();
                                            }
                                            app.refresh_index_counts();
                                            app.results.clear();
                                            app.reset_search();
                                            app.search_status = String::new();
                                            app.set_status("Index cleared", StatusLevel::Success);
                                        } else {
                                            app.set_status("Failed to clear index", StatusLevel::Error);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        1 => {
                            if !config.locations.is_empty() {
                                // Enter edits, Space toggles enabled
                                if matches!(key.code, KeyCode::Enter) {
                                    let loc = &config.locations[app.settings_popup_selected];
                                    let mut form = crate::app::LocationForm::default();
                                    form.fields[0] = loc.name.clone();
                                    form.fields[2] = loc.path.clone();
                                    form.is_ssh = loc.kind == "ssh";
                                    if form.is_ssh {
                                        form.fields[3] = loc.ssh_host.clone().unwrap_or_default();
                                        form.fields[4] = loc.ssh_port.to_string();
                                        form.fields[5] = loc.ssh_user.clone().unwrap_or_default();
                                        form.fields[6] = app.credentials.get(&loc.id).cloned().unwrap_or_default();
                                    }
                                    form.cursor_pos = form.fields[0].chars().count();
                                    app.form = form;
                                    app.show_settings_popup = false;
                                    app.return_to_popup = true;
                                    app.screen = crate::app::Screen::AddLocation { editing_id: Some(config.locations[app.settings_popup_selected].id.clone()) };
                                } else {
                                    // Space toggles
                                    let idx = app.settings_popup_selected;
                                    config.locations[idx].enabled = !config.locations[idx].enabled;
                                    app.config = config.clone();
                                    config.save().ok();
                                }
                            }
                        }
                        3 => {
                            app.pending_help_content = Some(help::CONTENT.to_string());
                            app.show_settings_popup = false;
                        }
                        _ => {}
                    }
                }
                KeyCode::Char('a') if app.settings_popup_tab == 1 => {
                    app.show_settings_popup = false;
                    app.return_to_popup = true;
                    app.screen = crate::app::Screen::AddLocation { editing_id: None };
                    app.form = crate::app::LocationForm::default();
                }
                KeyCode::Char('e') if app.settings_popup_tab == 1 => {
                    if !config.locations.is_empty() {
                        let loc = &config.locations[app.settings_popup_selected];
                        let mut form = crate::app::LocationForm::default();
                        form.fields[0] = loc.name.clone();
                        form.fields[2] = loc.path.clone();
                        form.is_ssh = loc.kind == "ssh";
                        if form.is_ssh {
                            form.fields[3] = loc.ssh_host.clone().unwrap_or_default();
                            form.fields[4] = loc.ssh_port.to_string();
                            form.fields[5] = loc.ssh_user.clone().unwrap_or_default();
                            form.fields[6] = app.credentials.get(&loc.id).cloned().unwrap_or_default();
                        }
                        app.form = form;
                        app.show_settings_popup = false;
                        app.return_to_popup = true;
                        app.screen = crate::app::Screen::AddLocation { editing_id: Some(loc.id.clone()) };
                    }
                }
                KeyCode::Char('d') if app.settings_popup_tab == 1 => {
                    if !config.locations.is_empty() {
                        let id = config.locations[app.settings_popup_selected].id.clone();
                        app.delete_confirm = Some(id);
                    }
                }
                KeyCode::Char('c') if app.settings_popup_tab == 1 => {
                    if !config.locations.is_empty() {
                        let loc_id = config.locations[app.settings_popup_selected].id.clone();
                        let loc_name = config.locations[app.settings_popup_selected].name.clone();
                        if let Ok(conn) = db::open_db(db_file) {
                            if db::delete_location_files(&conn, &loc_id).is_ok() {
                                if let Ok(mut idx) = app.file_index.write() {
                                    idx.retain(|f| f.location_id != loc_id);
                                }
                                app.results.retain(|r| r.file.location_id != loc_id);
                                app.refresh_index_counts();
                                app.refresh_visible_count();
                                let vis_len = app.visible_count;
                                if app.selected >= vis_len && vis_len > 0 {
                                    app.selected = vis_len - 1;
                                } else if vis_len == 0 {
                                    app.selected = 0;
                                    app.result_scroll = 0;
                                }
                                app.search_status = String::new();
                                app.location_saved_msg = Some(format!("Index cleared for '{}'", loc_name));
                            } else {
                                app.location_saved_msg = Some(format!("Failed to clear index for '{}'", loc_name));
                            }
                        }
                    }
                }
                _ => {}
            }

            // If a help topic was requested, open it in the configured editor
            if let Some(content) = app.pending_help_content.take() {
                let tmp_path = std::env::temp_dir().join("booker-help.txt");
                if std::fs::write(&tmp_path, &content).is_ok() {
                    let editor = config.settings.editor.clone();
                    disable_raw_mode()?;
                    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                    let _ = std::process::Command::new(&editor).arg(&tmp_path).status();
                    enable_raw_mode()?;
                    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                    terminal.clear()?;
                }
            }

            continue;
            } // end else (non-Ctrl key in settings popup)
        }

        // Info popup intercepts keys when visible
        if app.show_info_popup {
            let loc_is_ssh = {
                // Check fb_info_result first (file browser), then fall back to search result
                if let Some(r) = &app.fb_info_result {
                    config.locations.iter().find(|l| l.id == r.file.location_id)
                        .map(|l| l.kind == "ssh").unwrap_or(false)
                } else {
                    let visible = app.visible_results();
                    visible.get(app.selected)
                        .and_then(|r| config.locations.iter().find(|l| l.id == r.file.location_id))
                        .map(|l| l.kind == "ssh")
                        .unwrap_or(false)
                }
            };
            let tab_count = if loc_is_ssh { 4 } else { 3 };
            match key.code {
                KeyCode::Esc | KeyCode::Char(' ') | KeyCode::Char('z') | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) || key.code == KeyCode::Esc || key.code == KeyCode::Char(' ') => {
                    app.show_info_popup = false;
                    app.fb_info_result = None;
                    app.fb_info_ssh_stat = None;
                }
                KeyCode::Tab | KeyCode::Right => {
                    app.info_popup_tab = (app.info_popup_tab + 1) % tab_count;
                    app.info_popup_scroll = 0;
                }
                KeyCode::Left => {
                    app.info_popup_tab = app.info_popup_tab.saturating_sub(1);
                    app.info_popup_scroll = 0;
                }
                KeyCode::Up => {
                    app.info_popup_scroll = app.info_popup_scroll.saturating_sub(1);
                }
                KeyCode::Down => {
                    app.info_popup_scroll += 1;
                }
                _ => {}
            }
            continue;
        }

        // File browser intercepts keys when visible
        if app.fb.is_some() {
            // Check if we're in preview-focused mode
            let preview_focused = app.fb.as_ref().map(|fb| fb.preview_focused).unwrap_or(false);

            if preview_focused {
                // ── Preview pane focused: scroll / open / return ──────────────
                match key.code {
                    KeyCode::Esc | KeyCode::Left => {
                        if let Some(fb) = &mut app.fb {
                            fb.preview_focused = false;
                        }
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.fb = None;
                        app.fb_rx = None;
                    }
                    KeyCode::Up => {
                        if let Some(fb) = &mut app.fb {
                            fb.preview_scroll = fb.preview_scroll.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        if let Some(fb) = &mut app.fb {
                            let pixel_h = terminal.size().map(|s| s.height.saturating_sub(4) as usize).unwrap_or(20).max(1);
                            let max = fb.right_lines.len().saturating_sub(pixel_h);
                            fb.preview_scroll = (fb.preview_scroll + 1).min(max);
                        }
                    }
                    KeyCode::PageUp => {
                        if let Some(fb) = &mut app.fb {
                            let page = terminal.size().map(|s| s.height.saturating_sub(4) as usize).unwrap_or(20).max(1);
                            fb.preview_scroll = fb.preview_scroll.saturating_sub(page);
                        }
                    }
                    KeyCode::PageDown => {
                        if let Some(fb) = &mut app.fb {
                            let pixel_h = terminal.size().map(|s| s.height.saturating_sub(4) as usize).unwrap_or(20).max(1);
                            let max = fb.right_lines.len().saturating_sub(pixel_h);
                            fb.preview_scroll = (fb.preview_scroll + pixel_h).min(max);
                        }
                    }
                    KeyCode::Char('w') if key.modifiers == KeyModifiers::NONE => {
                        if let Some(fb) = &mut app.fb {
                            fb.preview_wrap = !fb.preview_wrap;
                            // Reset scroll when toggling wrap so the view isn't lost.
                            fb.preview_scroll = 0;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(fb) = &mut app.fb {
                            if let Some(entry) = fb.middle.entries.get(fb.middle.selected).cloned() {
                                if !entry.is_dir {
                                    let file_path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
                                    let loc_id = fb.location_id.clone();
                                    open_path_in_editor(app, terminal, &mut status_clear_time, &loc_id, &file_path)?;
                                }
                            }
                        }
                    }
                    KeyCode::Char(' ') | KeyCode::Char('n') if key.code == KeyCode::Char(' ') || key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(fb) = app.fb.as_ref() {
                            if let Some(entry) = fb.middle.entries.get(fb.middle.selected) {
                                let full_path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
                                let loc_id = fb.location_id.clone();
                                let loc_name = app.config.locations.iter()
                                    .find(|l| l.id == loc_id)
                                    .map(|l| l.name.clone())
                                    .unwrap_or_default();
                                app.fb_info_result = Some(app::SearchResult {
                                    file: app::IndexedFile {
                                        location_id: loc_id.clone(),
                                        path: full_path.clone(),
                                        display_path: format!("{} — {}", loc_name, full_path.clone()),
                                        is_dir: entry.is_dir,
                                    },
                                    match_type: app::MatchType::Name,
                                    score: 0,
                                    match_indices: Vec::new(),
                                    is_indexed: false,
                                    content_line: None,
                                    content_line_number: None,
                                });
                                app.fb_info_ssh_stat = None;
                                app.show_info_popup = true;
                                app.info_popup_tab = 0;
                                app.info_popup_scroll = 0;
                                if fb.is_ssh {
                                    if let Some(loc) = app.config.locations.iter().find(|l| l.id == loc_id).cloned() {
                                        let password = app.credentials.get(&loc.id).cloned().unwrap_or_default();
                                        let tx = fb.tx.clone();
                                        file_browser::launch_ssh_stat(full_path, loc, password, tx);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }

            // ── Normal (middle-pane) navigation ──────────────────────────────
            match key.code {
                KeyCode::Esc => {
                    app.fb = None;
                    app.fb_rx = None;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.fb = None;
                    app.fb_rx = None;
                }
                KeyCode::Up => {
                    if let Some(fb) = &mut app.fb {
                        if !fb.middle.entries.is_empty() && fb.middle.selected > 0 {
                            fb.middle.selected -= 1;
                            if fb.middle.selected < fb.middle.scroll {
                                fb.middle.scroll = fb.middle.selected;
                            }
                            trigger_fb_right(fb, &app.config, &app.credentials);
                        }
                    }
                }
                KeyCode::Down => {
                    if let Some(fb) = &mut app.fb {
                        let total = fb.middle.entries.len();
                        if total > 0 && fb.middle.selected < total - 1 {
                            fb.middle.selected += 1;
                            trigger_fb_right(fb, &app.config, &app.credentials);
                        }
                    }
                }
                KeyCode::PageUp => {
                    if let Some(fb) = &mut app.fb {
                        let page = terminal.size().map(|s| s.height.saturating_sub(4) as usize).unwrap_or(20).max(1);
                        let offset = fb.middle.selected.saturating_sub(fb.middle.scroll);
                        if fb.middle.scroll == 0 {
                            fb.middle.selected = 0;
                        } else {
                            fb.middle.scroll = fb.middle.scroll.saturating_sub(page);
                            fb.middle.selected = fb.middle.scroll + offset;
                        }
                        trigger_fb_right(fb, &app.config, &app.credentials);
                    }
                }
                KeyCode::PageDown => {
                    if let Some(fb) = &mut app.fb {
                        let pixel_h = terminal.size().map(|s| s.height.saturating_sub(4) as usize).unwrap_or(20).max(1);
                        let total = fb.middle.entries.len();
                        if total > 0 {
                            let offset = fb.middle.selected.saturating_sub(fb.middle.scroll);
                            let max_scroll = total.saturating_sub(pixel_h);
                            let new_scroll = (fb.middle.scroll + pixel_h).min(max_scroll);
                            if new_scroll == fb.middle.scroll {
                                // Already at max scroll — jump to last entry.
                                fb.middle.selected = total - 1;
                            } else {
                                // Scroll as far as possible, cursor keeps same offset.
                                fb.middle.scroll = new_scroll;
                                fb.middle.selected = (new_scroll + offset).min(total - 1);
                            }
                            trigger_fb_right(fb, &app.config, &app.credentials);
                        }
                    }
                }
                KeyCode::Char('w') if key.modifiers == KeyModifiers::NONE => {
                    if let Some(fb) = &mut app.fb {
                        fb.middle_wrap = !fb.middle_wrap;
                    }
                }
                KeyCode::Left => {
                    // Check whether we should jump to the locations picker.
                    let go_to_loc_root = if let Some(fb) = &app.fb {
                        if fb.at_location_root {
                            false // already there
                        } else {
                            let loc_base = app.config.locations.iter()
                                .find(|l| l.id == fb.location_id)
                                .map(|l| l.path.trim_end_matches('/').to_string())
                                .unwrap_or_default();
                            fb.middle.path.trim_end_matches('/') == loc_base
                        }
                    } else { false };

                    let at_loc_root_now = app.fb.as_ref().map(|fb| fb.at_location_root).unwrap_or(false);
                    if at_loc_root_now {
                        // Left at the locations picker → close the file browser entirely.
                        app.fb = None;
                        app.fb_rx = None;
                    } else if go_to_loc_root {
                        open_location_root_browser(app);
                    } else if let Some(fb) = &mut app.fb {
                        {
                        let new_middle_path = std::path::Path::new(&fb.middle.path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .filter(|p| !p.is_empty())
                            .unwrap_or_else(|| "/".to_string());
                        if new_middle_path == fb.middle.path {
                            // Already at filesystem root
                        } else {
                            // Reuse left pane as new middle (already loaded)
                            if !fb.left.loading {
                                let old_mid_name = std::path::Path::new(&fb.middle.path)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let sel = fb.left.entries.iter()
                                    .position(|e| e.name == old_mid_name).unwrap_or(0);
                                let new_mid = app::FbPane {
                                    path: fb.left.path.clone(),
                                    entries: fb.left.entries.clone(),
                                    selected: sel,
                                    scroll: 0,
                                    loading: false,
                                };
                                fb.middle = new_mid;
                            } else {
                                fb.middle = app::FbPane::loading(new_middle_path.clone());
                                let tx = fb.tx.clone();
                                let loc_id = fb.location_id.clone();
                                let is_ssh = fb.is_ssh;
                                let loc = app.config.locations.iter().find(|l| l.id == loc_id).cloned();
                                let password = app.credentials.get(&loc_id).cloned().unwrap_or_default();
                                if is_ssh {
                                    if let Some(loc) = loc {
                                        file_browser::launch_ssh_load(app::FbPaneTarget::Middle, new_middle_path.clone(), loc, password, tx);
                                    }
                                } else {
                                    file_browser::launch_local_load(app::FbPaneTarget::Middle, new_middle_path.clone(), tx);
                                }
                            }
                            // Load new left pane (parent of new middle), unless we're now
                            // at the location root — in that case show the locations list.
                            let loc_base_check = {
                                let loc_id = fb.location_id.clone();
                                app.config.locations.iter()
                                    .find(|l| l.id == loc_id)
                                    .map(|l| l.path.trim_end_matches('/').to_string())
                                    .unwrap_or_default()
                            };
                            if fb.middle.path.trim_end_matches('/') == loc_base_check {
                                let (loc_entries, loc_ids): (Vec<_>, Vec<_>) = app.config.locations.iter()
                                    .filter(|l| l.enabled)
                                    .map(|l| (
                                        app::FbEntry { name: l.name.clone(), is_dir: true, size: 0, modified: None },
                                        l.id.clone()
                                    ))
                                    .unzip();
                                let sel = loc_ids.iter().position(|id| *id == fb.location_id).unwrap_or(0);
                                fb.left = app::FbPane {
                                    path: "__locations__".to_string(),
                                    entries: loc_entries,
                                    selected: sel,
                                    scroll: 0,
                                    loading: false,
                                };
                                fb.location_entry_ids = loc_ids;
                            } else {
                                let new_left_path = std::path::Path::new(&fb.middle.path)
                                    .parent()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .filter(|p| !p.is_empty())
                                    .unwrap_or_else(|| "/".to_string());
                                if new_left_path == fb.middle.path {
                                    // Middle is already at filesystem root; show a static "/" stub
                                    fb.left = app::FbPane {
                                        path: "/".to_string(),
                                        entries: vec![app::FbEntry {
                                            name: "/".to_string(),
                                            is_dir: true,
                                            size: 0,
                                            modified: None,
                                        }],
                                        selected: 0,
                                        scroll: 0,
                                        loading: false,
                                    };
                                } else {
                                    fb.left = app::FbPane::loading(new_left_path.clone());
                                    let tx = fb.tx.clone();
                                    let loc_id = fb.location_id.clone();
                                    let is_ssh = fb.is_ssh;
                                    let loc = app.config.locations.iter().find(|l| l.id == loc_id).cloned();
                                    let password = app.credentials.get(&loc_id).cloned().unwrap_or_default();
                                    if is_ssh {
                                        if let Some(loc) = loc {
                                            file_browser::launch_ssh_load(app::FbPaneTarget::Left, new_left_path, loc, password, tx);
                                        }
                                    } else {
                                        file_browser::launch_local_load(app::FbPaneTarget::Left, new_left_path, tx);
                                    }
                                }
                            }
                            // Reload right pane for new middle selection
                            trigger_fb_right(fb, &app.config, &app.credentials);
                        }
                        } // end else (not at filesystem root)
                    } // end else if (not at_location_root)
                }
                KeyCode::Right => {
                    if app.fb.as_ref().map(|fb| fb.at_location_root).unwrap_or(false) {
                        enter_selected_location(app);
                    } else if let Some(fb) = &mut app.fb {
                        if let Some(entry) = fb.middle.entries.get(fb.middle.selected).cloned() {
                            if entry.is_dir {
                                let new_path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
                                // New left = current middle
                                let new_left = app::FbPane {
                                    path: fb.middle.path.clone(),
                                    entries: fb.middle.entries.clone(),
                                    selected: fb.middle.selected,
                                    scroll: fb.middle.scroll,
                                    loading: false,
                                };
                                // New middle = right pane if already loaded and correct path
                                let new_mid = if !fb.right.loading && fb.right.path == new_path {
                                    let mut m = fb.right.clone();
                                    m.selected = 0;
                                    m.scroll = 0;
                                    m
                                } else {
                                    app::FbPane::loading(new_path.clone())
                                };
                                let needs_load = new_mid.loading;
                                fb.left = new_left;
                                fb.middle = new_mid;
                                // If we just navigated into the location root, replace the
                                // left pane with the virtual locations list.
                                {
                                    let loc_base_check = {
                                        let loc_id = fb.location_id.clone();
                                        app.config.locations.iter()
                                            .find(|l| l.id == loc_id)
                                            .map(|l| l.path.trim_end_matches('/').to_string())
                                            .unwrap_or_default()
                                    };
                                    if new_path.trim_end_matches('/') == loc_base_check {
                                        let (loc_entries, loc_ids): (Vec<_>, Vec<_>) = app.config.locations.iter()
                                            .filter(|l| l.enabled)
                                            .map(|l| (
                                                app::FbEntry { name: l.name.clone(), is_dir: true, size: 0, modified: None },
                                                l.id.clone()
                                            ))
                                            .unzip();
                                        let sel = loc_ids.iter().position(|id| *id == fb.location_id).unwrap_or(0);
                                        fb.left = app::FbPane {
                                            path: "__locations__".to_string(),
                                            entries: loc_entries,
                                            selected: sel,
                                            scroll: 0,
                                            loading: false,
                                        };
                                        fb.location_entry_ids = loc_ids;
                                    }
                                }
                                if needs_load {
                                    let tx = fb.tx.clone();
                                    let loc_id = fb.location_id.clone();
                                    let is_ssh = fb.is_ssh;
                                    let loc = app.config.locations.iter().find(|l| l.id == loc_id).cloned();
                                    let password = app.credentials.get(&loc_id).cloned().unwrap_or_default();
                                    if is_ssh {
                                        if let Some(loc) = loc {
                                            file_browser::launch_ssh_load(app::FbPaneTarget::Middle, new_path, loc, password, tx);
                                        }
                                    } else {
                                        file_browser::launch_local_load(app::FbPaneTarget::Middle, new_path, tx);
                                    }
                                }
                                // Reload right pane
                                trigger_fb_right(fb, &app.config, &app.credentials);
                            } else {
                                // Right on a file → focus the preview pane
                                if !fb.right_lines.is_empty() {
                                    fb.preview_focused = true;
                                    fb.preview_scroll = 0;
                                }
                            }
                        }
                    } // end else if (not at_location_root)
                }
                KeyCode::Enter => {
                    if app.fb.as_ref().map(|fb| fb.at_location_root).unwrap_or(false) {
                        enter_selected_location(app);
                    } else if let Some(fb) = &mut app.fb {
                        if let Some(entry) = fb.middle.entries.get(fb.middle.selected).cloned() {
                            if entry.is_dir {
                                let new_path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
                                let new_left = app::FbPane {
                                    path: fb.middle.path.clone(),
                                    entries: fb.middle.entries.clone(),
                                    selected: fb.middle.selected,
                                    scroll: fb.middle.scroll,
                                    loading: false,
                                };
                                let new_mid = if !fb.right.loading && fb.right.path == new_path {
                                    let mut m = fb.right.clone();
                                    m.selected = 0;
                                    m.scroll = 0;
                                    m
                                } else {
                                    app::FbPane::loading(new_path.clone())
                                };
                                let needs_load = new_mid.loading;
                                fb.left = new_left;
                                fb.middle = new_mid;
                                // Same as Right: show locations list when at location root.
                                {
                                    let loc_base_check = {
                                        let loc_id = fb.location_id.clone();
                                        app.config.locations.iter()
                                            .find(|l| l.id == loc_id)
                                            .map(|l| l.path.trim_end_matches('/').to_string())
                                            .unwrap_or_default()
                                    };
                                    if new_path.trim_end_matches('/') == loc_base_check {
                                        let (loc_entries, loc_ids): (Vec<_>, Vec<_>) = app.config.locations.iter()
                                            .filter(|l| l.enabled)
                                            .map(|l| (
                                                app::FbEntry { name: l.name.clone(), is_dir: true, size: 0, modified: None },
                                                l.id.clone()
                                            ))
                                            .unzip();
                                        let sel = loc_ids.iter().position(|id| *id == fb.location_id).unwrap_or(0);
                                        fb.left = app::FbPane {
                                            path: "__locations__".to_string(),
                                            entries: loc_entries,
                                            selected: sel,
                                            scroll: 0,
                                            loading: false,
                                        };
                                        fb.location_entry_ids = loc_ids;
                                    }
                                }
                                if needs_load {
                                    let tx = fb.tx.clone();
                                    let loc_id = fb.location_id.clone();
                                    let is_ssh = fb.is_ssh;
                                    let loc = app.config.locations.iter().find(|l| l.id == loc_id).cloned();
                                    let password = app.credentials.get(&loc_id).cloned().unwrap_or_default();
                                    if is_ssh {
                                        if let Some(loc) = loc {
                                            file_browser::launch_ssh_load(app::FbPaneTarget::Middle, new_path, loc, password, tx);
                                        }
                                    } else {
                                        file_browser::launch_local_load(app::FbPaneTarget::Middle, new_path, tx);
                                    }
                                }
                                trigger_fb_right(fb, &app.config, &app.credentials);
                            } else {
                                // Enter on a file → open in editor directly
                                let file_path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
                                let loc_id = fb.location_id.clone();
                                open_path_in_editor(app, terminal, &mut status_clear_time, &loc_id, &file_path)?;
                            }
                        }
                    }
                }
                KeyCode::Char(' ') | KeyCode::Char('n') if key.code == KeyCode::Char(' ') || key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(fb) = app.fb.as_ref() {
                        if let Some(entry) = fb.middle.entries.get(fb.middle.selected) {
                            let full_path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
                            let loc_id = fb.location_id.clone();
                            let loc_name = app.config.locations.iter()
                                .find(|l| l.id == loc_id)
                                .map(|l| l.name.clone())
                                .unwrap_or_default();
                            app.fb_info_result = Some(app::SearchResult {
                                file: app::IndexedFile {
                                    location_id: loc_id.clone(),
                                    path: full_path.clone(),
                                    display_path: format!("{} — {}", loc_name, full_path.clone()),
                                    is_dir: entry.is_dir,
                                },
                                match_type: app::MatchType::Name,
                                score: 0,
                                match_indices: Vec::new(),
                                is_indexed: false,
                                content_line: None,
                                content_line_number: None,
                            });
                            app.fb_info_ssh_stat = None;
                            app.show_info_popup = true;
                            app.info_popup_tab = 0;
                            app.info_popup_scroll = 0;
                            if fb.is_ssh {
                                if let Some(loc) = app.config.locations.iter().find(|l| l.id == loc_id).cloned() {
                                    let password = app.credentials.get(&loc.id).cloned().unwrap_or_default();
                                    let tx = fb.tx.clone();
                                    file_browser::launch_ssh_stat(full_path, loc, password, tx);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            continue;
        }

        // Master password prompt takes priority
        if app.show_master_password_prompt {
            match key.code {
                KeyCode::Char(c) if key.modifiers == KeyModifiers::NONE => {
                    app.master_password_input.push(c);
                }
                KeyCode::Char(c) if key.modifiers == KeyModifiers::SHIFT => {
                    app.master_password_input.push(c.to_ascii_uppercase());
                }
                KeyCode::Backspace => {
                    app.master_password_input.pop();
                }
                KeyCode::Enter => {
                    // Rate limiting: enforce lock-out after repeated failures.
                    if let Some(until) = app.master_password_locked_until {
                        if Instant::now() < until {
                            let secs = (until - Instant::now()).as_secs() + 1;
                            app.set_status(
                                format!("Too many failed attempts — wait {}s before retrying", secs),
                                StatusLevel::Error,
                            );
                            status_clear_time = Some(until);
                            continue;
                        }
                    }
                    let pw = app.master_password_input.clone();
                    match try_unlock_credentials(&pw, config) {
                        Ok(creds) => {
                            app.credentials = creds;
                            app.master_password_unlocked = true;
                            app.master_password = Some(pw.clone());
                            app.show_master_password_prompt = false;
                            app.master_password_input.clear();
                            app.master_password_failed_attempts = 0;
                            app.master_password_locked_until = None;
                            // Verify sudo password — warn but don't block if sudo fails
                            if !crypto::verify_sudo_password(&pw) {
                                app.set_status(
                                    "Credentials unlocked (sudo verification failed — sudo may be misconfigured)",
                                    StatusLevel::Warning,
                                );
                            } else {
                                app.set_status("Credentials unlocked", StatusLevel::Success);
                            }
                            status_clear_time = Some(Instant::now() + Duration::from_secs(2));
                        }
                        Err(e) => {
                            use zeroize::Zeroize;
                            app.master_password_input.zeroize();
                            app.master_password_failed_attempts += 1;
                            // Exponential back-off: 1s, 2s, 4s … capped at 32s.
                            let delay = Duration::from_secs(
                                1u64 << app.master_password_failed_attempts.min(5),
                            );
                            app.master_password_locked_until = Some(Instant::now() + delay);
                            app.set_status(
                                format!("Failed to unlock: {}", e),
                                StatusLevel::Error,
                            );
                            status_clear_time = Some(Instant::now() + Duration::from_secs(3));
                        }
                    }
                }
                KeyCode::Esc => {
                    app.show_master_password_prompt = false;
                    app.master_password_input.clear();
                    app.set_status(
                        "Skipped credential unlock — SSH locations unavailable",
                        StatusLevel::Warning,
                    );
                    status_clear_time = Some(Instant::now() + Duration::from_secs(3));
                }
                _ => {}
            }
            continue;
        }

        // Delete confirm overlay
        if app.delete_confirm.is_some() {
            match key.code {
                KeyCode::Enter => {
                    if let Some(id) = app.delete_confirm.take() {
                        config.locations.retain(|l| l.id != id);
                        app.config = config.clone();
                        config.save().ok();
                        // Also remove from DB
                        if let Ok(conn) = db::open_db(db_file) {
                            db::delete_location_files(&conn, &id).ok();
                        }
                        // Reload index
                        if let Ok(conn) = db::open_db(db_file) {
                            if let Ok(files) = db::load_all_files(&conn, &config.locations) {
                                if let Ok(mut idx) = app.file_index.write() {
                                    *idx = files;
                                }
                                app.refresh_index_counts();
                            }
                        }
                        app.location_saved_msg = Some("Location deleted".to_string());
                        if app.config_selected >= config.locations.len() && !config.locations.is_empty() {
                            app.config_selected = config.locations.len() - 1;
                        }
                    }
                }
                KeyCode::Esc => {
                    app.delete_confirm = None;
                }
                _ => {}
            }
            continue;
        }

        // Form input for add/edit location
        if let Screen::AddLocation { editing_id } = app.screen.clone() {
            handle_form_input(app, key.code, key.modifiers, &editing_id, config, db_file, terminal).await?;
            continue;
        }

        // Main screen and config screen handling
        match &app.screen.clone() {
            Screen::Search => {
                handle_search_input(
                    app,
                    key.code,
                    key.modifiers,
                    config,
                    db_file,
                    terminal,
                    &mut last_query,
                    &mut content_search_timer,
                    &mut name_search_timer,
                    &mut status_clear_time,
                    &mut filter_rx,
                )
                .await?;

                // If we should quit
                if matches!(app.screen, Screen::Search) {
                    // check quit flag (handled by returning Ok inside function)
                }
            }
            Screen::Config => {
                handle_config_input(app, key.code, key.modifiers, config, db_file).await?;
            }
            Screen::Settings => {
                handle_settings_input(app, key.code, config);
            }
            Screen::AddLocation { .. } => {}
        }

        // Update name search if query changed
        if app.query != last_query {
            let old_query = std::mem::replace(&mut last_query, app.query.clone());
            // Cancel ongoing searches immediately
            for tx in app.name_search_cancel_txs.values() {
                tx.try_send(()).ok();
            }
            app.name_search_rx = None;
            app.name_search_cancel_txs.clear();
            app.name_search_pending_sources = 0;
            app.name_search_status = None;
            if let Some(tx) = &app.search_cancel_tx {
                tx.send(()).await.ok();
            }
            app.search_rx = None;
            app.search_cancel_tx = None;
            app.content_search_active = false;

            if app.query.len() >= 2 {
                // Broadening = removing characters (old query starts with new query).
                // Every existing result still matches a broader query, so keep them all.
                // Narrowing = typing more characters: re-filter to drop non-matching results.
                let broadening = old_query.len() > app.query.len()
                    && old_query.starts_with(app.query.as_str());

                if !broadening {
                    // Run filtering off the render thread so large result sets
                    // don't cause a visible freeze. Existing results stay on screen
                    // until the filter result arrives (usually within one frame).
                    let snapshot = app.results.clone();
                    let q = app.query.clone();
                    let consecutive = config.settings.consecutive_search;
                    let case_sensitive = config.settings.case_sensitive_search;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    filter_rx = Some(rx);
                    tokio::task::spawn_blocking(move || {
                        let filtered = search::refilter_results(&snapshot, &q, consecutive, case_sensitive);
                        tx.send(filtered).ok();
                    });
                    // seen_result_keys will be rebuilt when the filter result arrives
                } else {
                    // Broadening: all existing results still match, just rebuild keys.
                    app.seen_result_keys.clear();
                    app.live_confirmed_keys.clear();
                    for r in &app.results {
                        app.seen_result_keys.insert(format!("{}:{}", r.file.location_id, r.file.path));
                    }
                }
                app.refresh_visible_count();
                let vis_len = app.visible_count;
                if vis_len == 0 {
                    app.selected = 0;
                    app.result_scroll = 0;
                } else if app.selected >= vis_len {
                    app.selected = vis_len - 1;
                    app.result_scroll = 0;
                }
                app.search_status = format!("Searching… ({} results)", app.results.len());
                // Content search is a live scan — it cannot use the index.
                // Block it when mode is Index-only; the UI will show a warning.
                let content_needs_live = app.content_search
                    || app.search_target == SearchTarget::Content;
                let content_blocked = content_needs_live
                    && matches!(app.search_mode, SearchMode::Index);

                if app.search_target == SearchTarget::Content {
                    // Content-only mode: skip name search entirely.
                    app.name_search_active = false;
                    if !content_blocked {
                        content_search_timer = Some(Instant::now());
                    }
                } else {
                    app.name_search_active = true;
                    name_search_timer = Some(Instant::now());
                    if app.content_search && !content_blocked {
                        content_search_timer = Some(Instant::now());
                    }
                }
            } else {
                filter_rx = None;
                app.results.clear();
                app.seen_result_keys.clear();
                app.live_confirmed_keys.clear();
                app.location_search_states.clear();
                app.completed_locations.clear();
                app.reset_search();
                app.name_search_active = false;
                app.search_stopped = false;
                app.index_search_progress = None;
                let idx_count = app.file_index.read().map(|i| i.len()).unwrap_or(0);
                if idx_count > 0 {
                    app.search_status = format!("{} files indexed — type to search", idx_count);
                } else {
                    app.search_status = "No index — type to search (live mode)".to_string();
                }
            }
        }
    }
}

async fn handle_search_input(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    config: &mut Config,
    db_file: &std::path::Path,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    last_query: &mut String,
    content_search_timer: &mut Option<Instant>,
    name_search_timer: &mut Option<Instant>,
    status_clear_time: &mut Option<Instant>,
    filter_rx: &mut Option<tokio::sync::oneshot::Receiver<Vec<app::SearchResult>>>,
) -> Result<bool> {
    match code {
        KeyCode::Char('q') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Quit
            return Err(anyhow::anyhow!("quit"));
        }
        KeyCode::Char('f') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Toggle content search (Ctrl+F)
            app.content_search = !app.content_search;
            config.settings.content_search = app.content_search;
            app.config = config.clone();
            config.save().ok();
            if app.content_search && app.query.len() >= 2 {
                *content_search_timer = Some(Instant::now());
            } else {
                // Cancel content search
                if let Some(tx) = &app.search_cancel_tx {
                    tx.send(()).await.ok();
                }
                app.search_rx = None;
                app.search_cancel_tx = None;
                app.content_search_active = false;
                app.results.retain(|r| matches!(r.match_type, MatchType::Name));
                // Restart name search cleanly so we don't stay stuck in a half-finished state
                if app.query.len() >= 2 {
                    for tx in app.name_search_cancel_txs.values() {
                        tx.try_send(()).ok();
                    }
                    app.name_search_rx = None;
                    app.name_search_cancel_txs.clear();
                    app.name_search_pending_sources = 0;
                    app.seen_result_keys.clear();
                    app.live_confirmed_keys.clear();
                    app.results.clear();
                    app.reset_search();
                    *name_search_timer = Some(Instant::now());
                }
            }
        }
        KeyCode::Char('t') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Cycle search target (Ctrl+T): Files → Dirs → Both → Content → Files
            app.search_target = app.search_target.cycle();
            config.settings.search_target = app.search_target.label().to_string();
            app.config = config.clone();
            config.save().ok();
            let search_active = app.name_search_active || app.content_search_active;
            let can_filter = !search_active
                && app.completed_search_target
                    .map_or(false, |done| target_covers(done, app.search_target));
            if can_filter {
                // Existing results already cover the new target — just re-filter in place.
                let vis_len = app.visible_results().len();
                if vis_len == 0 {
                    app.selected = 0;
                    app.result_scroll = 0;
                } else if app.selected >= vis_len {
                    app.selected = vis_len - 1;
                    app.result_scroll = 0;
                }
            } else {
                // Cancel running searches and restart with the new target.
                if let Some(tx) = &app.search_cancel_tx {
                    tx.send(()).await.ok();
                }
                app.search_rx = None;
                app.search_cancel_tx = None;
                app.content_search_active = false;
                app.location_search_states.clear();
                app.completed_search_target = None;
                if app.query.len() >= 2 {
                    for tx in app.name_search_cancel_txs.values() {
                        tx.try_send(()).ok();
                    }
                    app.name_search_rx = None;
                    app.name_search_cancel_txs.clear();
                    app.name_search_pending_sources = 0;
                    app.seen_result_keys.clear();
                    app.live_confirmed_keys.clear();
                    app.results.clear();
                    app.reset_search();
                    if app.search_target == SearchTarget::Content {
                        *content_search_timer = Some(Instant::now());
                    } else {
                        app.name_search_active = true;
                        *name_search_timer = Some(Instant::now());
                        if app.content_search {
                            *content_search_timer = Some(Instant::now());
                        }
                    }
                }
            }
        }
        KeyCode::Char('z') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Toggle consecutive search (Ctrl+Z / fuzzy)
            config.settings.consecutive_search = !config.settings.consecutive_search;
            app.config = config.clone();
            config.save().ok();
            if app.query.len() >= 2 {
                for tx in app.name_search_cancel_txs.values() {
                    tx.try_send(()).ok();
                }
                app.name_search_rx = None;
                app.name_search_cancel_txs.clear();
                app.name_search_pending_sources = 0;
                app.seen_result_keys.clear();
                app.live_confirmed_keys.clear();
                app.results.clear();
                app.reset_search();
                app.name_search_active = true;
                *name_search_timer = Some(Instant::now());
            }
        }
KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.search_mode = app.search_mode.cycle();
            // Save to config
            config.settings.search_mode = app.search_mode.label().to_string();
            config.save().ok();
            app.config = config.clone();
            // Restart search if active
            if app.query.len() >= 2 {
                for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
                app.name_search_rx = None;
                app.name_search_tx = None;
                app.name_search_cancel_txs.clear();
                app.name_search_pending_sources = 0;
                app.seen_result_keys.clear();
                app.live_confirmed_keys.clear();
                app.results.clear();
                app.reset_search();
                app.name_search_active = true;
                *name_search_timer = Some(Instant::now());
            }
        }
        KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.status_bar_expanded = !app.status_bar_expanded;
            config.settings.status_bar_expanded = app.status_bar_expanded;
            config.save().ok();
            app.config = config.clone();
        }
        KeyCode::Char('h') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.status_bar_visible = !app.status_bar_visible;
            config.settings.status_bar_visible = app.status_bar_visible;
            config.save().ok();
            app.config = config.clone();
        }
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.controls_bar_visible = !app.controls_bar_visible;
            config.settings.controls_bar_visible = app.controls_bar_visible;
            config.save().ok();
            app.config = config.clone();
        }
        KeyCode::Char('n') if modifiers.contains(KeyModifiers::CONTROL) => {
            let has_selection = !app.visible_results().is_empty();
            if has_selection {
                app.show_info_popup = true;
                app.info_popup_tab = 0;
                app.info_popup_scroll = 0;
            }
        }
        KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Cycle location filter (Ctrl+L): All → location[0] → location[1] → … → All
            // Smart: filter-only when target already complete; keep running task when possible.
            app.cycle_active_location();
            apply_location_change(app, db_file, name_search_timer, filter_rx);
        }
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Toggle case-sensitive search (Ctrl+A)
            config.settings.case_sensitive_search = !config.settings.case_sensitive_search;
            app.config = config.clone();
            config.save().ok();
            if app.query.len() >= 2 {
                for tx in app.name_search_cancel_txs.values() {
                    tx.try_send(()).ok();
                }
                app.name_search_rx = None;
                app.name_search_cancel_txs.clear();
                app.name_search_pending_sources = 0;
                app.seen_result_keys.clear();
                app.live_confirmed_keys.clear();
                app.results.clear();
                app.reset_search();
                app.name_search_active = true;
                *name_search_timer = Some(Instant::now());
            }
        }
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Open file browser: at the selected result if one exists, else at location root
            if let Some(result) = app.visible_results().get(app.selected).map(|r| (*r).clone()) {
                if let Some(loc) = app.config.locations.iter().find(|l| l.id == result.file.location_id).cloned() {
                    let (middle_path, focus_name) = if result.file.is_dir {
                        (result.file.path.clone(), None)
                    } else {
                        let parent = std::path::Path::new(&result.file.path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| "/".to_string());
                        let name = std::path::Path::new(&result.file.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string());
                        (parent, name)
                    };
                    let loc_base = loc.path.trim_end_matches('/').to_string();
                    let at_loc_root = middle_path.trim_end_matches('/') == loc_base;

                    let is_ssh = loc.kind == "ssh";
                    let (tx, rx) = tokio::sync::mpsc::channel::<app::FbMsg>(64);
                    let password = app.credentials.get(&result.file.location_id).cloned().unwrap_or_default();

                    let (left_pane, location_entry_ids) = if at_loc_root {
                        let (entries, ids): (Vec<_>, Vec<_>) = app.config.locations.iter()
                            .filter(|l| l.enabled)
                            .map(|l| (
                                app::FbEntry { name: l.name.clone(), is_dir: true, size: 0, modified: None },
                                l.id.clone()
                            ))
                            .unzip();
                        let sel = ids.iter().position(|id| *id == result.file.location_id).unwrap_or(0);
                        let pane = app::FbPane {
                            path: "__locations__".to_string(),
                            entries,
                            selected: sel,
                            scroll: 0,
                            loading: false,
                        };
                        (pane, ids)
                    } else {
                        let left_path = std::path::Path::new(&middle_path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .filter(|p| !p.is_empty())
                            .unwrap_or_else(|| "/".to_string());
                        (app::FbPane::loading(left_path), Vec::new())
                    };
                    let left_path_for_load = if at_loc_root { None } else {
                        Some(left_pane.path.clone())
                    };

                    let fb = app::FileBrowser {
                        location_id: result.file.location_id.clone(),
                        is_ssh,
                        initial_focus: focus_name,
                        left: left_pane,
                        middle: app::FbPane::loading(middle_path.clone()),
                        right: app::FbPane::empty(String::new()),
                        tx: tx.clone(),
                        right_lines: Vec::new(),
                        right_content_loading: false,
                        right_content_path: String::new(),
                        right_lines_path: String::new(),
                        preview_focused: false,
                        preview_scroll: 0,
                        at_location_root: false,
                        location_entry_ids,
                        preview_wrap: false,
                        middle_wrap: false,
                    };
                    app.fb = Some(fb);
                    app.fb_rx = Some(rx);

                    if is_ssh {
                        if let Some(lp) = left_path_for_load {
                            file_browser::launch_ssh_load(app::FbPaneTarget::Left, lp, loc.clone(), password.clone(), tx.clone());
                        }
                        file_browser::launch_ssh_load(app::FbPaneTarget::Middle, middle_path, loc, password, tx);
                    } else {
                        if let Some(lp) = left_path_for_load {
                            file_browser::launch_local_load(app::FbPaneTarget::Left, lp, tx.clone());
                        }
                        file_browser::launch_local_load(app::FbPaneTarget::Middle, middle_path, tx);
                    }
                }
            } else {
                // No results selected: open at the locations root
                open_location_root_browser(app);
            }
        }
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            if app.index_rx.is_some() {
                // Cancel in-progress rebuild by dropping the receiver
                app.index_rx = None;
                app.db_progress = None;
                app.set_status("Index rebuild cancelled.", StatusLevel::Warning);
            } else {
                start_rebuild(app, config, db_file).await;
            }
        }
        KeyCode::Tab => {
            app.show_settings_popup = true;
            app.settings_popup_tab = 0; // Settings tab
            app.settings_popup_selected = 0;
            app.settings_header_selected = true;
        }
        KeyCode::Esc => {
            let searching = app.name_search_active || app.content_search_active;
            if searching {
                // First Esc: stop the search but keep results
                for tx in app.name_search_cancel_txs.values() {
                    tx.try_send(()).ok();
                }
                app.name_search_rx = None;
                app.name_search_cancel_txs.clear();
                app.name_search_pending_sources = 0;
                app.name_search_active = false;
                app.name_search_status = None;
                if let Some(tx) = &app.search_cancel_tx {
                    tx.try_send(()).ok();
                }
                app.search_rx = None;
                app.search_cancel_tx = None;
                app.content_search_active = false;
                app.search_status = String::new();
                app.search_stopped = true;
                app.index_search_progress = None;
                app.location_search_states.clear();
                app.both_mode_pending_live = false;
                // Flush any files discovered before the cancel to the DB.
                flush_pending_index_writes(app, db_file);
            } else {
                // Second Esc (or Esc with no active search): clear everything
                flush_pending_index_writes(app, db_file);
                app.query.clear();
                app.cursor_pos = 0;
                app.results.clear();
                app.reset_search();
                *last_query = String::new();
                app.search_status = String::new();
                app.seen_result_keys.clear();
                app.live_confirmed_keys.clear();
                app.search_stopped = false;
                app.index_search_progress = None;
            }
        }
        KeyCode::Up => {
            let vh = visible_height(app, terminal);
            app.scroll_up(vh);
        }
        KeyCode::Down => {
            let vh = visible_height(app, terminal);
            let total = app.visible_results().len();
            app.scroll_down(vh, total);
        }
        KeyCode::PageDown => {
            let page_size = visible_height(app, terminal);
            let total = app.visible_results().len();
            app.page_down(page_size, total);
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos -= 1;
            }
        }
        KeyCode::Right if app.cursor_pos < app.query.chars().count() => {
            app.cursor_pos += 1;
        }
        KeyCode::Home => {
            app.cursor_pos = 0;
        }
        KeyCode::End => {
            app.cursor_pos = app.query.chars().count();
        }
        KeyCode::Right => {
            // cursor is at end — open file browser at the selected result
            if let Some(result) = app.visible_results().get(app.selected).map(|r| (*r).clone()) {
                if let Some(loc) = app.config.locations.iter().find(|l| l.id == result.file.location_id).cloned() {
                    let (middle_path, focus_name) = if result.file.is_dir {
                        (result.file.path.clone(), None)
                    } else {
                        let parent = std::path::Path::new(&result.file.path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| "/".to_string());
                        let name = std::path::Path::new(&result.file.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string());
                        (parent, name)
                    };
                    let loc_base = loc.path.trim_end_matches('/').to_string();
                    let at_loc_root = middle_path.trim_end_matches('/') == loc_base;

                    let is_ssh = loc.kind == "ssh";
                    let (tx, rx) = tokio::sync::mpsc::channel::<app::FbMsg>(64);
                    let password = app.credentials.get(&result.file.location_id).cloned().unwrap_or_default();

                    let (left_pane, location_entry_ids) = if at_loc_root {
                        let (entries, ids): (Vec<_>, Vec<_>) = app.config.locations.iter()
                            .filter(|l| l.enabled)
                            .map(|l| (
                                app::FbEntry { name: l.name.clone(), is_dir: true, size: 0, modified: None },
                                l.id.clone()
                            ))
                            .unzip();
                        let sel = ids.iter().position(|id| *id == result.file.location_id).unwrap_or(0);
                        let pane = app::FbPane {
                            path: "__locations__".to_string(),
                            entries,
                            selected: sel,
                            scroll: 0,
                            loading: false,
                        };
                        (pane, ids)
                    } else {
                        let left_path = std::path::Path::new(&middle_path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .filter(|p| !p.is_empty())
                            .unwrap_or_else(|| "/".to_string());
                        (app::FbPane::loading(left_path), Vec::new())
                    };
                    let left_path_for_load = if at_loc_root { None } else {
                        Some(left_pane.path.clone())
                    };

                    let fb = app::FileBrowser {
                        location_id: result.file.location_id.clone(),
                        is_ssh,
                        initial_focus: focus_name,
                        left: left_pane,
                        middle: app::FbPane::loading(middle_path.clone()),
                        right: app::FbPane::empty(String::new()),
                        tx: tx.clone(),
                        right_lines: Vec::new(),
                        right_content_loading: false,
                        right_content_path: String::new(),
                        right_lines_path: String::new(),
                        preview_focused: false,
                        preview_scroll: 0,
                        at_location_root: false,
                        location_entry_ids,
                        preview_wrap: false,
                        middle_wrap: false,
                    };
                    app.fb = Some(fb);
                    app.fb_rx = Some(rx);

                    if is_ssh {
                        if let Some(lp) = left_path_for_load {
                            file_browser::launch_ssh_load(app::FbPaneTarget::Left, lp, loc.clone(), password.clone(), tx.clone());
                        }
                        file_browser::launch_ssh_load(app::FbPaneTarget::Middle, middle_path, loc, password, tx);
                    } else {
                        if let Some(lp) = left_path_for_load {
                            file_browser::launch_local_load(app::FbPaneTarget::Left, lp, tx.clone());
                        }
                        file_browser::launch_local_load(app::FbPaneTarget::Middle, middle_path, tx);
                    }
                }
            } else {
                // No results: open the locations picker
                open_location_root_browser(app);
            }
        }
        KeyCode::PageUp => {
            let page_size = visible_height(app, terminal);
            app.page_up(page_size);
        }
        KeyCode::Enter => {
            open_selected(app, terminal, status_clear_time)?;
        }
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                app.cursor_pos -= 1;
                let byte_pos = app.query.char_indices().nth(app.cursor_pos).map(|(i, _)| i).unwrap_or(0);
                app.query.remove(byte_pos);
            }
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.query.chars().count() {
                let byte_pos = app.query.char_indices().nth(app.cursor_pos).map(|(i, _)| i).unwrap_or(0);
                app.query.remove(byte_pos);
            }
        }
        KeyCode::Char(c)
            if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
        {
            let byte_pos = app.query.char_indices().nth(app.cursor_pos).map(|(i, _)| i).unwrap_or(app.query.len());
            app.query.insert(byte_pos, c);
            app.cursor_pos += 1;
        }
        _ => {}
    }
    Ok(false)
}

async fn handle_config_input(
    app: &mut App,
    code: KeyCode,
    _modifiers: KeyModifiers,
    config: &mut Config,
    _db_file: &std::path::Path,
) -> Result<()> {
    match code {
        KeyCode::Tab | KeyCode::Esc => {
            app.screen = Screen::Search;
        }
        KeyCode::Up => {
            app.config_scroll_up();
        }
        KeyCode::Down => {
            app.config_scroll_down();
        }
        KeyCode::Char('a') => {
            app.form = app::LocationForm::default();
            app.screen = Screen::AddLocation { editing_id: None };
        }
        KeyCode::Char('e') | KeyCode::Enter => {
            if let Some(loc) = config.locations.get(app.config_selected) {
                let mut form = app::LocationForm::default();
                form.fields[InputField::Name.index()] = loc.name.clone();
                form.fields[InputField::Path.index()] = loc.path.clone();
                form.is_ssh = loc.kind == "ssh";
                if form.is_ssh {
                    form.fields[InputField::SshHost.index()] =
                        loc.ssh_host.clone().unwrap_or_default();
                    form.fields[InputField::SshPort.index()] = loc.ssh_port.to_string();
                    form.fields[InputField::SshUser.index()] =
                        loc.ssh_user.clone().unwrap_or_default();
                    // Password loaded from credentials if unlocked
                    form.fields[InputField::SshPassword.index()] = app
                        .credentials
                        .get(&loc.id)
                        .cloned()
                        .unwrap_or_default();
                }
                let id = loc.id.clone();
                app.form = form;
                app.screen = Screen::AddLocation { editing_id: Some(id) };
            }
        }
        KeyCode::Char('d') => {
            if let Some(loc) = config.locations.get(app.config_selected) {
                app.delete_confirm = Some(loc.id.clone());
            }
        }
        KeyCode::Char(' ') => {
            if let Some(loc) = config.locations.get_mut(app.config_selected) {
                loc.enabled = !loc.enabled;
                app.config = config.clone();
                config.save().ok();
            }
        }
        KeyCode::Char('s') => {
            app.screen = Screen::Settings;
        }
        _ => {}
    }
    Ok(())
}

fn handle_settings_input(
    app: &mut App,
    code: KeyCode,
    config: &mut Config,
) {
    match code {
        KeyCode::Tab | KeyCode::Esc => {
            app.screen = Screen::Config;
        }
        KeyCode::Up => {
            if app.settings_popup_selected > 0 {
                app.settings_popup_selected -= 1;
            }
        }
        KeyCode::Down => {
            if app.settings_popup_selected < 1 {
                app.settings_popup_selected += 1;
            }
        }
        KeyCode::Char(' ') => {
            match app.settings_popup_selected {
                0 => {
                    let was_on = config.settings.require_unlock_on_startup;
                    config.settings.require_unlock_on_startup = !was_on;
                    app.config = config.clone();
                    config.save().ok();
                    if app.master_password_unlocked && !app.credentials.is_empty() {
                        if was_on {
                            crypto::save_plain(&app.credentials, &credentials_plain_path()).ok();
                            std::fs::remove_file(credentials_path()).ok();
                        } else {
                            if save_credentials(app, config).is_ok() && credentials_path().exists() {
                                std::fs::remove_file(credentials_plain_path()).ok();
                            }
                        }
                    }
                }
                1 => {
                    config.settings.consecutive_search = !config.settings.consecutive_search;
                    app.config = config.clone();
                    config.save().ok();
                }
                _ => {}
            }
        }
        _ => {}
    }
}

async fn handle_form_input(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    editing_id: &Option<String>,
    config: &mut Config,
    db_file: &std::path::Path,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    // Handle the plain-text SSH warning popup before anything else.
    if app.ssh_plain_warn {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.config.settings.require_unlock_on_startup = true;
                config.settings.require_unlock_on_startup = true;
                config.save().ok();
                app.form.is_ssh = true;
                app.ssh_plain_warn = false;
            }
            // N, Enter, or Esc all dismiss; Esc cancels the switch, N/Enter proceed plain.
            KeyCode::Esc => {
                app.ssh_plain_warn = false;
                // Don't switch to SSH.
            }
            _ => {
                // N, Enter, or any other key → proceed without encryption.
                app.form.is_ssh = true;
                app.ssh_plain_warn = false;
            }
        }
        return Ok(());
    }

    let field_count = app.form.field_count();
    let focused = app.form.focused_field;
    let current_field = app.form.current_field();
    let storage = current_field.index();

    match code {
        KeyCode::Esc => {
            app.clear_status();
            if app.return_to_popup {
                app.return_to_popup = false;
                app.show_settings_popup = true;
                app.settings_popup_tab = 1;
                app.screen = Screen::Search;
            } else {
                app.screen = Screen::Config;
            }
        }
        KeyCode::Tab | KeyCode::Down => {
            app.form.focused_field = (focused + 1) % field_count;
            app.form.cursor_pos = app.form.fields[app.form.current_field().index()].chars().count();
            app.clear_status();
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.form.focused_field = if focused == 0 {
                field_count - 1
            } else {
                focused - 1
            };
            app.form.cursor_pos = app.form.fields[app.form.current_field().index()].chars().count();
            app.clear_status();
        }
        KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            if current_field == InputField::Type =>
        {
            let switching_to_ssh = !app.form.is_ssh;
            if switching_to_ssh && !app.config.settings.require_unlock_on_startup {
                app.ssh_plain_warn = true;
            } else {
                app.form.is_ssh = !app.form.is_ssh;
            }
            app.clear_status();
        }
        KeyCode::Left => {
            if app.form.cursor_pos > 0 {
                app.form.cursor_pos -= 1;
            }
        }
        KeyCode::Right => {
            let len = app.form.fields[storage].chars().count();
            if app.form.cursor_pos < len {
                app.form.cursor_pos += 1;
            }
        }
        KeyCode::Home => {
            app.form.cursor_pos = 0;
        }
        KeyCode::End => {
            app.form.cursor_pos = app.form.fields[storage].chars().count();
        }
        KeyCode::Char('z') if app.form.is_ssh && modifiers.contains(KeyModifiers::CONTROL) => {
            if app.form.is_ssh {
                let host = app.form.fields[InputField::SshHost.index()].trim().to_string();
                let port_str = app.form.fields[InputField::SshPort.index()].trim().to_string();
                let user = app.form.fields[InputField::SshUser.index()].trim().to_string();
                let pw = app.form.fields[InputField::SshPassword.index()].clone();
                let path = app.form.fields[InputField::Path.index()].trim().to_string();
                if host.is_empty() {
                    app.set_status("SSH Host is required for test", StatusLevel::Error);
                } else {
                    let port: u16 = port_str.parse().unwrap_or(22);
                    let loc = Location {
                        id: String::new(),
                        name: String::new(),
                        kind: "ssh".to_string(),
                        path,
                        ssh_host: Some(host.clone()),
                        ssh_port: port,
                        ssh_user: Some(user),
                        enabled: true,
                    };
                    app.set_status(format!("Testing SSH connection to {}…", host), StatusLevel::Info);
                    let _ = terminal.draw(|f| ui::render(f, app));
                    let result = tokio::task::spawn_blocking(move || {
                        let client = ssh_client::SshClient::connect(&loc, &pw)?;
                        // Check path if one was provided
                        let path_result = if loc.path.trim().is_empty() {
                            None
                        } else {
                            let cmd = format!("test -d {} && echo dir || (test -e {} && echo exists || echo missing)",
                                ssh_client::shell_escape(&loc.path), ssh_client::shell_escape(&loc.path));
                            match client.exec(&cmd) {
                                Ok(out) => Some(Ok(out.trim().to_string())),
                                Err(e) => Some(Err(e.to_string())),
                            }
                        };
                        Ok::<_, anyhow::Error>((path_result,))
                    }).await;
                    match result {
                        Ok(Ok((path_result,))) => {
                            match path_result {
                                None => {
                                    app.set_status("Connected successfully. No path configured to check.", StatusLevel::Success);
                                }
                                Some(Ok(ref s)) if s == "dir" => {
                                    app.set_status("Connected successfully. Path exists and is a directory.", StatusLevel::Success);
                                }
                                Some(Ok(ref s)) if s == "exists" => {
                                    app.set_status("Connected successfully. Path exists but is not a directory.", StatusLevel::Warning);
                                }
                                Some(Ok(_)) => {
                                    app.set_status("Connected successfully, but path does not exist on the remote host.", StatusLevel::Warning);
                                }
                                Some(Err(e)) => {
                                    app.set_status(format!("Connected successfully, but could not check path: {}", e), StatusLevel::Warning);
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            app.set_status(format!("SSH connection failed: {}", e), StatusLevel::Error);
                        }
                        Err(e) => {
                            app.set_status(format!("SSH test error: {}", e), StatusLevel::Error);
                        }
                    }
                }
            }
        }
        KeyCode::Enter => {
            save_location_form(app, editing_id, config, db_file).await?;
        }
        KeyCode::Backspace => {
            if current_field != InputField::Type && app.form.cursor_pos > 0 {
                let val = &mut app.form.fields[storage];
                let byte_pos = val.char_indices()
                    .nth(app.form.cursor_pos - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                val.remove(byte_pos);
                app.form.cursor_pos -= 1;
                app.clear_status();
            }
        }
        KeyCode::Delete => {
            if current_field != InputField::Type {
                let val = &mut app.form.fields[storage];
                let len = val.chars().count();
                if app.form.cursor_pos < len {
                    let byte_pos = val.char_indices()
                        .nth(app.form.cursor_pos)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    val.remove(byte_pos);
                    app.clear_status();
                }
            }
        }
        KeyCode::Char(c) => {
            if current_field == InputField::Type {
                // Space already handled above
            } else {
                let c = if modifiers.contains(KeyModifiers::SHIFT) {
                    c.to_ascii_uppercase()
                } else {
                    c
                };
                let val = &mut app.form.fields[storage];
                let byte_pos = val.char_indices()
                    .nth(app.form.cursor_pos)
                    .map(|(i, _)| i)
                    .unwrap_or(val.len());
                val.insert(byte_pos, c);
                app.form.cursor_pos += 1;
                app.clear_status();
            }
        }
        _ => {}
    }
    Ok(())
}

async fn save_location_form(
    app: &mut App,
    editing_id: &Option<String>,
    config: &mut Config,
    _db_file: &std::path::Path,
) -> Result<()> {
    let path = app.form.fields[InputField::Path.index()].trim().to_string();
    let name = {
        let raw = app.form.fields[InputField::Name.index()].trim().to_string();
        if raw.is_empty() {
            // Auto-derive from path
            if app.form.is_ssh {
                let host = app.form.fields[InputField::SshHost.index()].trim().to_string();
                let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or(&path);
                if host.is_empty() {
                    path.clone()
                } else {
                    format!("{}:{}", host, last)
                }
            } else {
                std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path)
                    .to_string()
            }
        } else {
            raw
        }
    };

    if path.is_empty() {
        app.set_status("Path is required", StatusLevel::Error);
        return Ok(());
    }

    // Path safety hint — warn once if the path looks like it contains credentials.
    const SENSITIVE_DIRS: &[&str] = &[".ssh", ".gnupg", ".aws", ".kube", ".password-store"];
    let path_lc = path.to_lowercase();
    if SENSITIVE_DIRS.iter().any(|d| path_lc.contains(d)) && !app.form.path_warn_shown {
        app.form.path_warn_shown = true;
        app.set_status(
            "⚠ This path may contain sensitive files — filenames will be indexed. Press Enter again to confirm.",
            StatusLevel::Warning,
        );
        return Ok(());
    }

    let kind = if app.form.is_ssh { "ssh" } else { "local" };

    if kind == "local" {
        let p = std::path::Path::new(&path);
        if !p.exists() {
            app.set_status(format!("Path does not exist: {}", path), StatusLevel::Error);
            return Ok(());
        }
        if !p.is_dir() {
            app.set_status(format!("Path is not a directory: {}", path), StatusLevel::Error);
            return Ok(());
        }
    }

    let (ssh_host, ssh_port, ssh_user, ssh_password) = if app.form.is_ssh {
        let host = app.form.fields[InputField::SshHost.index()].trim().to_string();
        let port_str = app.form.fields[InputField::SshPort.index()].trim().to_string();
        let user = app.form.fields[InputField::SshUser.index()].trim().to_string();
        let pw = app.form.fields[InputField::SshPassword.index()].clone();

        if host.is_empty() {
            app.set_status("SSH Host is required for SSH locations", StatusLevel::Error);
            return Ok(());
        }
        let port: u16 = port_str.parse().unwrap_or(22);
        (Some(host), port, Some(user), pw)
    } else {
        (None, 22u16, None, String::new())
    };

    let had_ssh_password = app.form.is_ssh && !ssh_password.is_empty();

    if let Some(id) = editing_id {
        // Update existing
        if let Some(loc) = config.locations.iter_mut().find(|l| &l.id == id) {
            loc.name = name;
            loc.path = path;
            loc.kind = kind.to_string();
            loc.ssh_host = ssh_host;
            loc.ssh_port = ssh_port;
            loc.ssh_user = ssh_user;
        }
        // Update credentials if SSH
        if had_ssh_password {
            app.credentials.insert(id.clone(), ssh_password);
            save_credentials(app, config)?;
        }
    } else {
        // New location
        let id = Config::generate_location_id();
        let loc = Location {
            id: id.clone(),
            name,
            kind: kind.to_string(),
            path,
            ssh_host,
            ssh_port,
            ssh_user,
            enabled: true,
        };
        config.locations.push(loc);

        if had_ssh_password {
            app.credentials.insert(id, ssh_password);
            save_credentials(app, config)?;
        }
    }

    app.config = config.clone();
    config.save()?;
    if app.return_to_popup {
        app.return_to_popup = false;
        app.show_settings_popup = true;
        app.settings_popup_tab = 1;
        app.screen = Screen::Search;
    } else {
        app.screen = Screen::Config;
    }
    let plain_cred = had_ssh_password
        && !app.config.settings.require_unlock_on_startup;
    app.location_saved_msg = Some(if plain_cred {
        "Saved — ⚠ password stored as plain text (open Settings → Encrypt credentials to protect it)".to_string()
    } else {
        "Location saved — press ^R to rebuild index".to_string()
    });

    Ok(())
}


fn save_credentials(app: &mut App, config: &mut Config) -> Result<()> {
    if !app.master_password_unlocked {
        return Ok(());
    }

    if !config.settings.require_unlock_on_startup {
        // Plain storage
        crypto::save_plain(&app.credentials, &credentials_plain_path())?;
    } else if let Some(ref pw) = app.master_password.clone() {
        // Encrypted storage
        let salt = if let Some(salt_b64) = &config.credential_salt {
            CredentialStore::salt_from_base64(salt_b64)?
        } else {
            let salt = CredentialStore::generate_salt();
            config.credential_salt = Some(CredentialStore::salt_to_base64(&salt));
            config.save().ok();
            salt
        };
        let ciphertext = CredentialStore::encrypt(pw, &salt, &app.credentials)?;
        let cred_path = credentials_path();
        if let Some(parent) = cred_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&cred_path, &ciphertext)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cred_path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn try_unlock_credentials(
    master_pw: &str,
    config: &Config,
) -> Result<HashMap<String, String>> {
    let cred_path = credentials_path();
    if !cred_path.exists() {
        return Ok(HashMap::new());
    }

    let salt_b64 = config
        .credential_salt
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No credential salt in config"))?;
    let salt = CredentialStore::salt_from_base64(salt_b64)?;

    let ciphertext = std::fs::read_to_string(&cred_path)
        .context("Failed to read credentials file")?;
    let ciphertext = ciphertext.trim();

    let credentials = CredentialStore::decrypt(master_pw, &salt, ciphertext)?;
    Ok(credentials)
}

async fn start_rebuild(
    app: &mut App,
    config: &Config,
    db_file: &std::path::Path,
) {
    app.location_errors.clear();

    let (tx, rx) = mpsc::channel::<IndexMsg>(256);
    app.index_rx = Some(rx);
    app.db_progress = Some(IndexProgress {
        total: 0,
        current: 0,
        current_path: String::new(),
        location_name: String::new(),
        eta_secs: None,
        done: false,
    });

    let locations = config.locations.clone();
    let credentials = app.credentials.clone();
    let db_path = db_file.to_path_buf();

    indexer::rebuild_index(locations, credentials, db_path, tx).await;
}

async fn start_name_search(app: &mut App, config: &Config) {
    // Content-only mode has no name search phase.
    if app.search_target == SearchTarget::Content {
        return;
    }
    debug::log(2, &format!("start_name_search query={:?} mode={} target={}", app.query, app.search_mode.label(), app.search_target.label()));
    app.search_stopped = false;
    app.both_mode_pending_live = false;
    app.location_search_states.clear();
    app.index_search_progress = None;
    let (tx, rx) = mpsc::channel::<NameSearchMsg>(512);
    app.name_search_rx = Some(rx);
    app.name_search_tx = Some(tx.clone());
    app.name_search_cancel_txs.clear();
    app.name_search_pending_sources = 0;
    app.completed_locations.clear();
    app.name_search_active = true;

    let query = app.query.clone();
    let target = app.search_target;
    let consecutive = config.settings.consecutive_search;
    let case_sensitive = config.settings.case_sensitive_search;
    let index_empty = app.file_index.read().map(|i| i.is_empty()).unwrap_or(true);
    let loc_filter = app.active_location.clone();
    let search_mode = app.search_mode;

    // 1) Indexed search — runs in Index or Both mode (if index exists)
    let run_indexed = matches!(search_mode, SearchMode::Index | SearchMode::Both) && !index_empty;
    if run_indexed {
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        app.name_search_cancel_txs.insert(String::new(), cancel_tx);
        app.name_search_pending_sources += 1;
        search::search_names_indexed_async(
            query.clone(), app.file_index.clone(), loc_filter.clone(),
            target, consecutive, case_sensitive,
            tx.clone(), cancel_rx,
        );
    }

    // In Both mode, live searches start AFTER indexed completes (sequential).
    // In Live mode, live searches start immediately.
    if matches!(search_mode, SearchMode::Both) && run_indexed {
        // Live phase will be triggered when indexed Done arrives
        app.both_mode_pending_live = true;
    } else if matches!(search_mode, SearchMode::Live | SearchMode::Both) {
        // Both with empty index, or Live mode: start live immediately
        launch_live_searches(app, config, &tx);
    }

    if app.name_search_pending_sources == 0 {
        app.name_search_active = false;
        app.search_status = "No locations configured".to_string();
    }
    // tx dropped here; tasks hold their own clones
}

fn launch_live_searches(
    app: &mut App,
    config: &Config,
    tx: &tokio::sync::mpsc::Sender<NameSearchMsg>,
) {
    let query = app.query.clone();
    let target = app.search_target;
    let consecutive = config.settings.consecutive_search;
    let case_sensitive = config.settings.case_sensitive_search;
    let loc_filter = app.active_location.clone();

    let local_locs: Vec<_> = config.locations.iter()
        .filter(|l| l.enabled && l.kind == "local")
        .filter(|l| loc_filter.as_ref().map_or(true, |id| &l.id == id))
        .cloned()
        .collect();
    for loc in local_locs {
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        app.name_search_cancel_txs.insert(loc.id.clone(), cancel_tx);
        app.name_search_pending_sources += 1;
        search::search_names_live_async(
            query.clone(), vec![loc], target, consecutive, case_sensitive,
            tx.clone(), cancel_rx,
        );
    }

    let ssh_locs: Vec<_> = config.locations.iter()
        .filter(|l| l.enabled && l.kind == "ssh")
        .filter(|l| loc_filter.as_ref().map_or(true, |id| &l.id == id))
        .cloned()
        .collect();
    for loc in ssh_locs {
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        app.name_search_cancel_txs.insert(loc.id.clone(), cancel_tx);
        app.name_search_pending_sources += 1;
        search::search_names_ssh_async(
            query.clone(), loc, app.credentials.clone(),
            target, consecutive, case_sensitive,
            tx.clone(), cancel_rx,
        );
    }
}

fn flush_pending_index_writes(app: &mut App, db_file: &std::path::Path) {
    if app.pending_index_writes.is_empty() {
        return;
    }
    let writes = std::mem::take(&mut app.pending_index_writes);
    let db_path_clone = db_file.to_path_buf();
    let mut files_by_loc: std::collections::HashMap<String, (Vec<String>, Vec<String>)> = std::collections::HashMap::new();
    for f in writes {
        let entry = files_by_loc.entry(f.location_id).or_default();
        if f.is_dir { entry.1.push(f.path); } else { entry.0.push(f.path); }
    }
    tokio::task::spawn_blocking(move || {
        if let Ok(conn) = db::open_db(&db_path_clone) {
            for (loc_id, (files, dirs)) in files_by_loc {
                if !files.is_empty() { db::batch_upsert_files(&conn, &loc_id, &files, false).ok(); }
                if !dirs.is_empty() { db::batch_upsert_files(&conn, &loc_id, &dirs, true).ok(); }
            }
        }
    });
    app.refresh_index_counts();
}

/// Smart location-change handler for ^L and the location-picker popup.
///
/// After `app.active_location` has been updated, this function decides:
///   • Filter-only — target already complete; just re-filter visible results.
///   • Keep running — target task is in progress; cancel tasks for other locs.
///   • Restart — target not started/not complete; full restart for target only.
fn apply_location_change(
    app: &mut App,
    db_file: &std::path::Path,
    name_search_timer: &mut Option<Instant>,
    filter_rx: &mut Option<tokio::sync::oneshot::Receiver<Vec<app::SearchResult>>>,
) {
    if app.query.len() < 2 {
        // No active search — just re-filter visible results.
        app.refresh_visible_count();
        let vis = app.visible_count;
        if vis == 0 { app.selected = 0; app.result_scroll = 0; }
        else if app.selected >= vis { app.selected = vis - 1; app.result_scroll = 0; }
        return;
    }

    let new_loc = app.active_location.clone();
    let enabled_locs: Vec<String> = app.config.locations.iter()
        .filter(|l| l.enabled)
        .map(|l| l.id.clone())
        .collect();

    // Is the requested target fully searched already?
    let target_complete = match &new_loc {
        None => enabled_locs.iter().all(|id| app.completed_locations.contains(id)),
        Some(loc_id) => app.completed_locations.contains(loc_id),
    };

    // Is the requested target currently being searched (task running)?
    // The indexed task (key "") covers all scoped locs while it runs.
    let target_running = match &new_loc {
        None => enabled_locs.iter().any(|id| app.name_search_cancel_txs.contains_key(id.as_str())),
        Some(loc_id) => {
            app.name_search_cancel_txs.contains_key(loc_id.as_str())
                || app.name_search_cancel_txs.contains_key("")
        }
    };

    if target_complete && !app.name_search_active {
        // All results are in — just re-filter.
        app.refresh_visible_count();
        let vis = app.visible_count;
        if vis == 0 { app.selected = 0; app.result_scroll = 0; }
        else if app.selected >= vis { app.selected = vis - 1; app.result_scroll = 0; }
        return;
    }

    if target_complete {
        // Target done but other tasks still running — cancel the others.
        let ids_to_cancel: Vec<String> = match &new_loc {
            None => vec![],
            Some(loc_id) => app.name_search_cancel_txs.keys()
                .filter(|k| *k != loc_id)
                .cloned().collect(),
        };
        let n = ids_to_cancel.len();
        for id in &ids_to_cancel {
            if let Some(tx) = app.name_search_cancel_txs.remove(id) { tx.try_send(()).ok(); }
        }
        app.name_search_pending_sources = app.name_search_pending_sources.saturating_sub(n as u8);
        if app.name_search_active && app.name_search_pending_sources == 0 {
            finalize_name_search(app, db_file);
        } else {
            app.refresh_visible_count();
            let vis = app.visible_count;
            if vis == 0 { app.selected = 0; app.result_scroll = 0; }
            else if app.selected >= vis { app.selected = vis - 1; app.result_scroll = 0; }
        }
        return;
    }

    if target_running {
        // Target task is in progress — cancel tasks for other locs (keep indexed "" and target).
        let ids_to_cancel: Vec<String> = match &new_loc {
            None => vec![],
            Some(loc_id) => app.name_search_cancel_txs.keys()
                .filter(|k| !k.is_empty() && *k != loc_id.as_str())
                .cloned().collect(),
        };
        let n = ids_to_cancel.len();
        for id in &ids_to_cancel {
            if let Some(tx) = app.name_search_cancel_txs.remove(id) { tx.try_send(()).ok(); }
        }
        app.name_search_pending_sources = app.name_search_pending_sources.saturating_sub(n as u8);
        *filter_rx = None;
        app.refresh_visible_count();
        let vis = app.visible_count;
        if vis == 0 { app.selected = 0; app.result_scroll = 0; }
        else if app.selected >= vis { app.selected = vis - 1; app.result_scroll = 0; }
        return;
    }

    // Target not complete and not running — restart search scoped to this location.
    *filter_rx = None;
    for tx in app.name_search_cancel_txs.values() { tx.try_send(()).ok(); }
    app.name_search_rx = None;
    app.name_search_tx = None;
    app.name_search_cancel_txs.clear();
    app.name_search_pending_sources = 0;
    app.seen_result_keys.clear();
    app.live_confirmed_keys.clear();
    app.results.clear();
    app.reset_search();
    app.name_search_active = true;
    *name_search_timer = Some(Instant::now());
}

fn finalize_name_search(app: &mut App, db_file: &std::path::Path) {
    debug::log(2, &format!("finalize_name_search results={}", app.results.len()));
    // All sources finished — fair sort + truncate on NAME results only.
    // Content results (score=0) must be preserved separately so they
    // are not sorted to the bottom and then truncated away.
    let content_results: Vec<_> = app.results
        .iter()
        .filter(|r| matches!(r.match_type, MatchType::Content))
        .cloned()
        .collect();
    app.results.retain(|r| matches!(r.match_type, MatchType::Name));

    // Results are already sorted by score (maintained via binary insertion during search).
    // Re-append content results that arrived before name search finished.
    app.results.extend(content_results);

    // Clamp selected to visible (filtered) result count.
    app.refresh_visible_count();
    let vis_len = app.visible_count;
    if vis_len == 0 {
        app.selected = 0;
        app.result_scroll = 0;
    } else if app.selected >= vis_len {
        app.selected = vis_len - 1;
        app.result_scroll = 0;
    }

    app.name_search_active = false;
    app.completed_search_target = Some(app.search_target);
    app.name_search_status = None;
    app.name_search_rx = None;
    app.name_search_cancel_txs.clear();

    // Write newly discovered files to DB
    flush_pending_index_writes(app, db_file);
    // Only clear location states if content search is not still running.
    // If content search is active it will update (and eventually clear) these states itself.
    if !app.content_search_active {
        app.location_search_states.clear();
    }
    app.name_search_tx = None;

    let name_count = app.results.iter().filter(|r| matches!(r.match_type, MatchType::Name)).count();
    let content_count = app.results.iter().filter(|r| matches!(r.match_type, MatchType::Content)).count();
    app.search_status = if content_count > 0 {
        format!("{} name + {} content results", name_count, content_count)
    } else if app.content_search_active {
        format!("{} results (searching content…)", name_count)
    } else {
        format!("{} results", name_count)
    };
}

async fn start_content_search(app: &mut App, config: &Config) {
    debug::log(2, &format!("start_content_search query={:?}", app.query));
    // Cancel previous search
    if let Some(tx) = &app.search_cancel_tx {
        tx.send(()).await.ok();
    }
    app.search_rx = None;
    app.search_cancel_tx = None;

    // Remove old content results
    app.results.retain(|r| matches!(r.match_type, MatchType::Name));

    let (result_tx, result_rx) = mpsc::channel::<SearchMsg>(256);
    let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

    app.search_rx = Some(result_rx);
    app.search_cancel_tx = Some(cancel_tx);
    app.content_search_active = true;
    // Snapshot index counts as denominators for per-location progress bars.
    app.content_location_totals = app.index_location_counts.clone();
    // Clear previous scan counts.
    if let Ok(mut m) = app.content_scan_counts.lock() { m.clear(); }

    let query = app.query.clone();
    let locations = config.locations.clone();
    let credentials = app.credentials.clone();
    let scan_counts = app.content_scan_counts.clone();

    search::search_contents(query, locations, credentials, result_tx, cancel_rx, scan_counts).await;
}

fn open_selected(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    status_clear_time: &mut Option<Instant>,
) -> Result<()> {
    let result = match app.visible_results().get(app.selected).map(|r| (*r).clone()) {
        Some(r) => r,
        None => return Ok(()),
    };
    open_path_in_editor(app, terminal, status_clear_time, &result.file.location_id, &result.file.path)
}

fn open_path_in_editor(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    status_clear_time: &mut Option<Instant>,
    loc_id: &str,
    file_path: &str,
) -> Result<()> {
    let loc = app.config.locations.iter()
        .find(|l| l.id == loc_id)
        .cloned();

    if let Some(loc) = loc.filter(|l| l.kind == "ssh") {
        open_ssh_file(app, terminal, status_clear_time, &loc, file_path)?;
    } else {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

        let editor = app.config.settings.editor.clone();
        let status = std::process::Command::new(&editor)
            .arg(file_path)
            .status();

        enable_raw_mode()?;
        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
        terminal.clear()?;

        match status {
            Ok(s) if s.success() => {}
            Ok(_) => {
                app.set_status(format!("{} exited with non-zero status", editor), StatusLevel::Warning);
                *status_clear_time = Some(Instant::now() + Duration::from_secs(3));
            }
            Err(e) => {
                app.set_status(format!("Failed to open {}: {}", editor, e), StatusLevel::Error);
                *status_clear_time = Some(Instant::now() + Duration::from_secs(3));
            }
        }
    }

    Ok(())
}

/// Open the file browser in location-picker mode (virtual top level listing all locations).
fn open_location_root_browser(app: &mut App) {
    // Remember which location was active so we can pre-select it in the picker.
    let current_loc_id = app.fb.as_ref().map(|fb| fb.location_id.clone()).unwrap_or_default();

    let (tx, rx) = tokio::sync::mpsc::channel::<app::FbMsg>(64);
    let entries: Vec<app::FbEntry> = app.config.locations.iter()
        .filter(|l| l.enabled)
        .map(|l| app::FbEntry { name: l.name.clone(), is_dir: true, size: 0, modified: None })
        .collect();
    let location_entry_ids: Vec<String> = app.config.locations.iter()
        .filter(|l| l.enabled)
        .map(|l| l.id.clone())
        .collect();
    let selected = location_entry_ids.iter().position(|id| *id == current_loc_id).unwrap_or(0);
    let middle = app::FbPane { path: String::new(), entries, selected, scroll: 0, loading: false };
    app.fb = Some(app::FileBrowser {
        location_id: String::new(),
        is_ssh: false,
        initial_focus: None,
        left: app::FbPane::empty(String::new()),
        middle,
        right: app::FbPane::empty(String::new()),
        tx,
        right_lines: Vec::new(),
        right_content_loading: false,
        right_content_path: String::new(),
        right_lines_path: String::new(),
        preview_focused: false,
        preview_scroll: 0,
        at_location_root: true,
        location_entry_ids,
        preview_wrap: false,
        middle_wrap: false,
    });
    app.fb_rx = Some(rx);
    // Load the right pane for the initially selected entry.
    if let Some(fb) = &mut app.fb {
        trigger_fb_right(fb, &app.config, &app.credentials);
    }
}

/// Enter the location currently selected in the locations-picker middle pane.
fn enter_selected_location(app: &mut App) {
    let (loc_idx, loc_id, is_ssh, middle_path, password, loc) = {
        let fb = match &app.fb { Some(fb) => fb, None => return };
        let idx = fb.middle.selected;
        let loc_id = match fb.location_entry_ids.get(idx) {
            Some(id) => id.clone(),
            None => return,
        };
        let loc = match app.config.locations.iter().find(|l| l.id == loc_id) {
            Some(l) => l.clone(),
            None => return,
        };
        let is_ssh = loc.kind == "ssh";
        let middle_path = loc.path.trim_end_matches('/').to_string();
        let password = app.credentials.get(&loc_id).cloned().unwrap_or_default();
        (idx, loc_id, is_ssh, middle_path, password, loc)
    };

    // Build the left pane as the locations list so pressing ← returns to the picker.
    let loc_entries: Vec<app::FbEntry> = app.config.locations.iter()
        .filter(|l| l.enabled)
        .map(|l| app::FbEntry { name: l.name.clone(), is_dir: true, size: 0, modified: None })
        .collect();
    let location_entry_ids: Vec<String> = app.config.locations.iter()
        .filter(|l| l.enabled)
        .map(|l| l.id.clone())
        .collect();
    let left_pane = app::FbPane {
        path: "__locations__".to_string(), // sentinel: renderer shows "Locations" title
        entries: loc_entries,
        selected: loc_idx,
        scroll: 0,
        loading: false,
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<app::FbMsg>(64);
    app.fb = Some(app::FileBrowser {
        location_id: loc_id,
        is_ssh,
        initial_focus: None,
        left: left_pane,
        middle: app::FbPane::loading(middle_path.clone()),
        right: app::FbPane::empty(String::new()),
        tx: tx.clone(),
        right_lines: Vec::new(),
        right_content_loading: false,
        right_content_path: String::new(),
        right_lines_path: String::new(),
        preview_focused: false,
        preview_scroll: 0,
        at_location_root: false,
        location_entry_ids,
        preview_wrap: false,
        middle_wrap: false,
    });
    app.fb_rx = Some(rx);

    // Only load the middle pane; left is pre-populated as the locations list.
    if is_ssh {
        file_browser::launch_ssh_load(app::FbPaneTarget::Middle, middle_path, loc, password, tx);
    } else {
        file_browser::launch_local_load(app::FbPaneTarget::Middle, middle_path, tx);
    }
}

/// Trigger an async load for the right pane of the file browser based on the
/// currently selected entry in the middle pane.
fn trigger_fb_right(
    fb: &mut app::FileBrowser,
    cfg: &config::Config,
    credentials: &std::collections::HashMap<String, String>,
) {
    // Navigation always leaves preview-focus mode and resets scroll.
    fb.preview_focused = false;
    fb.preview_scroll = 0;

    // Location-picker root: right pane shows the selected location's root dir.
    if fb.at_location_root {
        let idx = fb.middle.selected;
        if let Some(loc_id) = fb.location_entry_ids.get(idx).cloned() {
            if let Some(loc) = cfg.locations.iter().find(|l| l.id == loc_id).cloned() {
                let path = loc.path.trim_end_matches('/').to_string();
                fb.right = app::FbPane::loading(path.clone());
                fb.right_lines.clear();
                fb.right_content_loading = false;
                fb.right_content_path.clear();
                fb.right_lines_path.clear();
                let tx = fb.tx.clone();
                let is_ssh = loc.kind == "ssh";
                let password = credentials.get(&loc.id).cloned().unwrap_or_default();
                if is_ssh {
                    file_browser::launch_ssh_load(app::FbPaneTarget::Right, path, loc, password, tx);
                } else {
                    file_browser::launch_local_load(app::FbPaneTarget::Right, path, tx);
                }
            }
        }
        return;
    }

    let loc_id = fb.location_id.clone();
    let is_ssh = fb.is_ssh;
    let tx = fb.tx.clone();
    if let Some(entry) = fb.middle.entries.get(fb.middle.selected).cloned() {
        if entry.is_dir {
            let path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
            fb.right = app::FbPane::loading(path.clone());
            fb.right_lines.clear();
            fb.right_content_loading = false;
            fb.right_content_path.clear();
            fb.right_lines_path.clear();
            let loc = cfg.locations.iter().find(|l| l.id == loc_id).cloned();
            let password = credentials.get(&loc_id).cloned().unwrap_or_default();
            if is_ssh {
                if let Some(loc) = loc {
                    file_browser::launch_ssh_load(app::FbPaneTarget::Right, path, loc, password, tx);
                }
            } else {
                file_browser::launch_local_load(app::FbPaneTarget::Right, path, tx);
            }
        } else {
            // File: trigger async content load
            let file_path = format!("{}/{}", fb.middle.path.trim_end_matches('/'), entry.name);
            fb.right = app::FbPane::empty(file_path.clone());
            fb.right_content_path = file_path.clone();
            fb.right_content_loading = true;
            let loc = cfg.locations.iter().find(|l| l.id == loc_id).cloned();
            let password = credentials.get(&loc_id).cloned().unwrap_or_default();
            if is_ssh {
                if let Some(loc) = loc {
                    file_browser::launch_ssh_file_content(file_path, loc, password, tx);
                }
            } else {
                file_browser::launch_local_file_content(file_path, tx);
            }
        }
    } else {
        fb.right = app::FbPane::empty(String::new());
        fb.right_lines.clear();
        fb.right_content_loading = false;
        fb.right_content_path.clear();
        fb.right_lines_path.clear();
    }
}

fn open_ssh_file(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    status_clear_time: &mut Option<Instant>,
    loc: &crate::config::Location,
    file_path: &str,
) -> Result<()> {
    use std::io::Write;

    let user = loc.ssh_user.as_deref().unwrap_or("root");
    let host = loc.ssh_host.as_deref().unwrap_or("localhost");
    let port = loc.ssh_port;
    let editor = app.config.settings.editor.clone();
    let password = app.credentials.get(&loc.id).cloned();

    // Write a temporary SSH_ASKPASS script if we have a stored password.
    // SSH_ASKPASS_REQUIRE=force tells OpenSSH ≥8.4 to always use it, even on a tty.
    let askpass_path: Option<String> = if let Some(ref pw) = password {
        use std::os::unix::fs::OpenOptionsExt;
        let path = format!("/tmp/rum-askpass-{}", std::process::id());
        let result: std::io::Result<()> = (|| {
            // create_new + mode 0o700 is atomic — no race window between create and chmod.
            let mut f = std::fs::OpenOptions::new()
                .write(true).create_new(true).mode(0o700)
                .open(&path)?;
            writeln!(f, "#!/bin/sh")?;
            // printf avoids a trailing newline and handles special chars safely
            writeln!(f, "printf '%s' '{}'", pw.replace('\'', "'\\''"))?;
            Ok(())
        })();
        match result {
            Ok(()) => Some(path),
            Err(_) => None,
        }
    } else {
        None
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let status = if matches!(editor.as_str(), "nvim" | "vim" | "vi") {
        // nvim netrw: scp://user@host:port//absolute/path
        // file_path already starts with '/', so the extra '/' gives the required double-slash.
        let uri = format!("scp://{}@{}:{}/{}", user, host, port, file_path);
        let mut cmd = std::process::Command::new(&editor);
        cmd.args(["--cmd", "let g:netrw_silent=1"]);
        cmd.arg(&uri);
        if let Some(ref ap) = askpass_path {
            cmd.env("SSH_ASKPASS", ap).env("SSH_ASKPASS_REQUIRE", "force");
        }
        cmd.status()
    } else {
        // nano/other: run the editor on the remote host via ssh -t
        let quoted = format!("'{}'", file_path.replace('\'', "'\\''"));
        let editor_escaped = format!("'{}'", editor.replace('\'', "'\\''"));
        let remote_cmd = format!("{} {}", editor_escaped, quoted);
        let host_arg = format!("{}@{}", user, host);
        let port_str = port.to_string();
        let mut cmd = std::process::Command::new("ssh");
        cmd.args(["-t", "-p", &port_str, &host_arg, &remote_cmd]);
        if let Some(ref ap) = askpass_path {
            cmd.env("SSH_ASKPASS", ap).env("SSH_ASKPASS_REQUIRE", "force");
        }
        cmd.status()
    };

    // Always clean up the temp askpass script
    if let Some(ref ap) = askpass_path {
        let _ = std::fs::remove_file(ap);
    }

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;

    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            app.set_status(
                format!("{} exited with non-zero status", editor),
                StatusLevel::Warning,
            );
            *status_clear_time = Some(Instant::now() + Duration::from_secs(3));
        }
        Err(e) => {
            app.set_status(
                format!("Failed to open SSH file in {}: {}", editor, e),
                StatusLevel::Error,
            );
            *status_clear_time = Some(Instant::now() + Duration::from_secs(3));
        }
    }

    Ok(())
}


/// Estimate the visible height of the results list from the terminal size.
/// Accounts for: logo(3) + results block overhead + status bar + search bar(3).
/// Results block overhead: 2 borders + 1 col-header = 3; +2 when controls bar is
/// visible (settings header row + divider row).
fn visible_height(app: &App, terminal: &Terminal<CrosstermBackend<io::Stdout>>) -> usize {
    let term_h = terminal.size().map(|s| s.height as usize).unwrap_or(24);
    let num_enabled_locs = app.config.locations.iter().filter(|l| l.enabled).count();
    let show_index_row = !matches!(app.search_mode, crate::app::SearchMode::Live);
    let show_live_rows = !matches!(app.search_mode, crate::app::SearchMode::Index);
    let status_h = if !app.status_bar_visible {
        0
    } else if app.status_bar_expanded && num_enabled_locs > 0 && show_live_rows {
        2 + (show_index_row as usize) + num_enabled_locs
    } else {
        2 + (show_index_row as usize) + (show_live_rows as usize)
    };
    // results overhead: 2 (block borders) + 1 (col header) + 2 when controls bar shown
    let results_overhead = 3 + if app.controls_bar_visible { 2 } else { 0 };
    let chrome = 3 + results_overhead + status_h + 3; // logo + results overhead + status bar + search bar
    let extra = if app.db_progress.is_some() { 4 } else { 0 };
    let pixel_h = term_h.saturating_sub(chrome + extra);
    if pixel_h == 0 { return 1; }
    // Count items that fit from the current scroll position.
    // Content results are 2 display lines; name results are 1.
    let visible = app.visible_results();
    if visible.is_empty() { return pixel_h; }
    let start = app.result_scroll.min(visible.len());
    let mut count = 0usize;
    let mut used = 0usize;
    for r in visible.iter().skip(start) {
        let h = if r.content_line.is_some() { 2 } else { 1 };
        if used + h > pixel_h { break; }
        used += h;
        count += 1;
    }
    count.max(1)
}

