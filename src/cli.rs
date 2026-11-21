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

    /// What to search: files, dirs, all, content
    #[arg(short, long, default_value = "files", value_name = "TARGET")]
    pub target: String,

    /// Search mode: index, live, both
    #[arg(short, long, default_value = "both", value_name = "MODE")]
    pub mode: String,

    /// Also search inside file contents (grep)
    #[arg(short = 'f', long = "content")]
    pub content_search: bool,

    /// Case-sensitive matching
    #[arg(short = 'c', long)]
    pub case_sensitive: bool,

    /// Consecutive matching: query characters must appear in order with no gaps
    #[arg(short = 'z', long)]
    pub consecutive: bool,

    /// Restrict to a location by name or ID
    #[arg(short, long, value_name = "LOCATION")]
    pub location: Option<String>,

    /// Maximum number of results to output
    #[arg(short = 'n', long, default_value = "100", value_name = "N")]
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
        eprintln!("bs: query must be at least 2 characters");
        std::process::exit(1);
    }

    let target = match cli.target.as_str() {
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
            eprintln!("bs: location {:?} not found", wanted);
            std::process::exit(1);
        }
    }

    let show_location = config.locations.iter().filter(|l| l.enabled).count() > 1;
    let sep: &str = if cli.null { "\0" } else { "\n" };

    // ── Name search ─────────────────────────────────────────────────────────
    if target != SearchTarget::Content {
        let results = run_name_search(
            &query, config, &credentials, &file_index,
            target, cli.consecutive, cli.case_sensitive,
            &loc_filter, cli.limit, &cli.mode,
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
    if cli.content_search || target == SearchTarget::Content {
        run_content_search(
            &query, config, &credentials,
            cli.case_sensitive, &loc_filter, cli.limit, sep,
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
            Some(_) => {}
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
            let mut args: Vec<&str> = vec!["-rn"];
            if !case_sensitive { args.push("-i"); }
            args.extend_from_slice(&["--", query, &loc.path]);
            if let Ok(out) = std::process::Command::new("grep").args(&args).output() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    if printed >= limit { break; }
                    // grep format: path:lineno:text
                    let mut parts = line.splitn(3, ':');
                    if let (Some(path), Some(ln), Some(text)) =
                        (parts.next(), parts.next(), parts.next())
                    {
                        print!("{}:{}:{}{}", path, ln, text, sep);
                        printed += 1;
                    }
                }
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
                        eprintln!("bs: SSH connect failed for '{}': {}", loc.name, e);
                    }
                }
            }
        }
    }
}
