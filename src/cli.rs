use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use clap::Parser;
use tokio::sync::mpsc;

use crate::app::{IndexedFile, NameSearchMsg, SearchTarget};
use crate::config::Config;
use crate::search;

#[derive(Parser, Debug)]
#[command(name = "rum")]
#[command(about = "Fuzzy file finder with SSH support")]
#[command(long_about = "rum — fuzzy file finder with SSH support.\n\nRun without arguments to open the interactive TUI.\nProvide a QUERY to search non-interactively and print matching paths to stdout.")]
#[command(version)]
pub struct Cli {
    /// Search query (omit to open the interactive TUI)
    pub query: Option<String>,

    /// What to search: files, dirs, all, content (default: from saved settings)
    #[arg(short = 't', long = "search-target", value_name = "TARGET")]
    pub target: Option<String>,

    /// Search mode: index, live, both (default: from saved settings)
    #[arg(short = 's', long = "search-mode", value_name = "MODE")]
    pub mode: Option<String>,

    /// Search inside file contents (default: from saved settings)
    #[arg(short = 'f', long = "search-file-content")]
    pub content_search: bool,

    /// Case-sensitive matching (default: from saved settings)
    #[arg(short = 'a', long = "case-sensitive")]
    pub case_sensitive: bool,

    /// Enable fuzzy matching: letters can be non-adjacent (e.g. 'ac' matches 'abc') (default: from saved settings)
    #[arg(short = 'z', long)]
    pub fuzzy: bool,

    /// Restrict to a location by name or ID
    #[arg(short, long, value_name = "LOCATION")]
    pub location: Option<String>,

    /// Maximum number of results to output (0 = unlimited)
    #[arg(short = 'n', long, default_value = "0", value_name = "N")]
    pub limit: usize,

    /// Separate output paths with NUL bytes instead of newlines (for xargs -0)
    #[arg(short = '0', long)]
    pub null: bool,
}

pub async fn run_cli(
    cli: &Cli,
    config: &Config,
    credentials: HashMap<String, String>,
    file_index: Arc<RwLock<Vec<IndexedFile>>>,
) {
    let query = match &cli.query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => return,
    };

    if query.len() < 2 {
        eprintln!("rum: query must be at least 2 characters");
        std::process::exit(1);
    }

    // Resolve each option: CLI flag takes priority, then saved config settings.
    let target_str = cli.target.as_deref().unwrap_or(&config.settings.search_target).to_string();
    let mode_str   = cli.mode.as_deref().unwrap_or(&config.settings.search_mode).to_string();
    let use_content        = cli.content_search || config.settings.content_search;
    let use_case_sensitive = cli.case_sensitive || config.settings.case_sensitive_search;
    // --fuzzy turns fuzzy ON (consecutive OFF); without the flag, use the saved setting.
    let use_consecutive = if cli.fuzzy { false } else { config.settings.consecutive_search };
    // 0 = unlimited; use a sentinel that won't overflow when doubled in comparisons.
    let limit = if cli.limit == 0 { usize::MAX / 2 } else { cli.limit };

    let target = match target_str.as_str() {
        "dirs"    => SearchTarget::Folders,
        "all"     => SearchTarget::Both,
        "content" => SearchTarget::Content,
        _         => SearchTarget::Files,
    };

    // Resolve location filter: match by name or ID (case-insensitive name).
    let loc_filter: Option<String> = cli.location.as_ref().and_then(|name| {
        config.locations.iter()
            .find(|l| l.id == *name || l.name.eq_ignore_ascii_case(name))
            .map(|l| l.id.clone())
    });

    if let Some(ref wanted) = cli.location {
        if loc_filter.is_none() {
            eprintln!("rum: location {:?} not found", wanted);
            std::process::exit(1);
        }
    }

    let show_location = config.locations.iter().filter(|l| l.enabled).count() > 1;
    let sep: &str = if cli.null { "\0" } else { "\n" };

    // ── Name search ─────────────────────────────────────────────────────────
    if target != SearchTarget::Content {
        let results = run_name_search(
            &query, config, &credentials, &file_index,
            target, use_consecutive, use_case_sensitive,
            &loc_filter, limit, &mode_str,
        ).await;

        for r in &results {
            if show_location {
                let loc_name = config.locations.iter()
                    .find(|l| l.id == r.file.location_id)
                    .map(|l| l.name.as_str())
                    .unwrap_or("?");
                print!("{}:{}{}", loc_name, r.file.path, sep);
            } else {
                print!("{}{}", r.file.path, sep);
            }
        }
    }

    // ── Content search ───────────────────────────────────────────────────────
    if use_content || target == SearchTarget::Content {
        run_content_search(
            &query, config, &credentials,
            use_case_sensitive, &loc_filter, limit, sep,
        ).await;
    }
}

async fn run_name_search(
    query: &str,
    config: &Config,
    credentials: &HashMap<String, String>,
    file_index: &Arc<RwLock<Vec<IndexedFile>>>,
    target: SearchTarget,
    consecutive: bool,
    case_sensitive: bool,
    loc_filter: &Option<String>,
    limit: usize,
    mode_str: &str,
) -> Vec<crate::app::SearchResult> {
    use crate::app::SearchMode;

    let mode = match mode_str {
        "index" => SearchMode::Index,
        "live"  => SearchMode::Live,
        _       => SearchMode::Both,
    };

    let (tx, mut rx) = mpsc::channel::<NameSearchMsg>(512);
    // Cancel senders kept alive so tasks don't see a disconnected receiver.
    let mut _cancel_txs: Vec<mpsc::Sender<()>> = Vec::new();
    let mut pending: usize = 0;

    // Indexed phase
    let index_empty = file_index.read().map(|i| i.is_empty()).unwrap_or(true);
    if matches!(mode, SearchMode::Index | SearchMode::Both) && !index_empty {
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        _cancel_txs.push(cancel_tx);
        pending += 1;
        search::search_names_indexed_async(
            query.to_string(), file_index.clone(), loc_filter.clone(),
            target, consecutive, case_sensitive, tx.clone(), cancel_rx,
        );
    }

    // Live phase
    if matches!(mode, SearchMode::Live | SearchMode::Both) {
        for loc in config.locations.iter().filter(|l| l.enabled && l.kind == "local") {
            if loc_filter.as_ref().map_or(false, |id| id != &loc.id) { continue; }
            let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
            _cancel_txs.push(cancel_tx);
            pending += 1;
            search::search_names_live_async(
                query.to_string(), vec![loc.clone()], target,
                consecutive, case_sensitive, tx.clone(), cancel_rx,
            );
        }
        for loc in config.locations.iter().filter(|l| l.enabled && l.kind == "ssh") {
            if loc_filter.as_ref().map_or(false, |id| id != &loc.id) { continue; }
            let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
            _cancel_txs.push(cancel_tx);
            pending += 1;
            search::search_names_ssh_async(
                query.to_string(), loc.clone(), credentials.clone(),
                target, consecutive, case_sensitive, tx.clone(), cancel_rx,
            );
        }
    }

    drop(tx); // close our sender so channel drains when all tasks finish

    let mut results: Vec<crate::app::SearchResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    use std::io::IsTerminal;
    let stderr_tty = std::io::stderr().is_terminal();
    let mut progress_shown = false;

    while pending > 0 {
        match rx.recv().await {
            Some(NameSearchMsg::Batch(batch)) => {
                for r in batch {
                    let key = format!("{}:{}", r.file.location_id, r.file.path);
                    if seen.insert(key) {
                        results.push(r);
                    }
                }
                if results.len() >= limit * 2 { break; }
            }
            Some(NameSearchMsg::Done { .. }) => {
                pending = pending.saturating_sub(1);
            }
            Some(NameSearchMsg::Error(e)) => {
                if progress_shown { eprint!("\r\x1b[K"); }
                eprintln!("rum: {}", e);
                progress_shown = false;
                pending = pending.saturating_sub(1);
            }
            Some(NameSearchMsg::Searching { count, current_dir, .. }) => {
                if stderr_tty {
                    use std::io::Write;
                    if count > 0 {
                        eprint!("\r\x1b[K  scanning: {} ({} found)", current_dir, count);
                    } else {
                        eprint!("\r\x1b[K  scanning: {}", current_dir);
                    }
                    let _ = std::io::stderr().flush();
                    progress_shown = true;
                }
            }
            Some(NameSearchMsg::IndexProgress { processed, total }) => {
                if stderr_tty {
                    use std::io::Write;
                    if total > 0 {
                        let pct = processed * 100 / total;
                        eprint!("\r\x1b[K  searching index: {}/{} ({}%)", processed, total, pct);
                    } else {
                        eprint!("\r\x1b[K  searching index…");
                    }
                    let _ = std::io::stderr().flush();
                    progress_shown = true;
                }
            }
            None => break,
        }
    }

    // Clear the progress line before printing results.
    if progress_shown {
        use std::io::Write;
        eprint!("\r\x1b[K");
        let _ = std::io::stderr().flush();
    }

    // Sort by score descending and truncate.
    results.sort_by(|a, b| b.score.cmp(&a.score));
    results.truncate(limit);
    results
}

async fn run_content_search(
    query: &str,
    config: &Config,
    credentials: &HashMap<String, String>,
    case_sensitive: bool,
    loc_filter: &Option<String>,
    limit: usize,
    sep: &str,
) {
    use crate::ssh_client::SshClient;

    let mut printed = 0usize;

    for loc in config.locations.iter().filter(|l| l.enabled) {
        if loc_filter.as_ref().map_or(false, |id| id != &loc.id) { continue; }
        if printed >= limit { break; }

        if loc.kind == "local" {
            use std::io::{IsTerminal, Write};
            let stderr_tty = std::io::stderr().is_terminal();
            let query_lower = query.to_lowercase();
            let walker = ignore::WalkBuilder::new(&loc.path)
                .hidden(true)
                .git_ignore(true)
                .build();
            for entry in walker.flatten() {
                if printed >= limit { break; }
                let path = entry.path();
                if !path.is_file() { continue; }
                let path_str = path.to_string_lossy().into_owned();
                if stderr_tty {
                    eprint!("\r\x1b[K  {}", path_str);
                    let _ = std::io::stderr().flush();
                }
                let Ok(content) = std::fs::read_to_string(path) else { continue };
                for (i, line) in content.lines().enumerate() {
                    if printed >= limit { break; }
                    let matched = if case_sensitive {
                        line.contains(query)
                    } else {
                        line.to_lowercase().contains(&query_lower)
                    };
                    if matched {
                        print!("{}:{}:{}{}", path_str, i + 1, line, sep);
                        printed += 1;
                    }
                }
            }
            if stderr_tty {
                eprint!("\r\x1b[K");
                let _ = std::io::stderr().flush();
            }
        } else if loc.kind == "ssh" {
            if let Some(password) = credentials.get(&loc.id) {
                match SshClient::connect(loc, password) {
                    Ok(client) => {
                        let _ = client.grep_files_streamed(
                            &loc.path, query,
                            |path, lineno, text| {
                                if printed < limit {
                                    print!("{}:{}:{}{}", path, lineno, text, sep);
                                    printed += 1;
                                }
                            },
                        );
                    }
                    Err(e) => {
                        eprintln!("rum: SSH connect failed for '{}': {}", loc.name, e);
                    }
                }
            }
        }
    }
}
