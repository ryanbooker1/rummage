use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc::Sender;

use crate::app::{IndexedFile, MatchType, NameSearchMsg, SearchMsg, SearchResult, SearchTarget};
use crate::config::Location;
use crate::ssh_client::SshClient;

pub fn search_names_indexed_async(
    query: String,
    index: Arc<RwLock<Vec<IndexedFile>>>,
    active_location: Option<String>,
    target: SearchTarget,
    consecutive: bool,
    case_sensitive: bool,
    tx: tokio::sync::mpsc::Sender<NameSearchMsg>,
    mut cancel_rx: tokio::sync::mpsc::Receiver<()>,
) {
    tokio::task::spawn_blocking(move || {
        let matcher = if case_sensitive {
            SkimMatcherV2::default().respect_case()
        } else {
            SkimMatcherV2::default()
        };
        let index = match index.read() {
            Ok(i) => i,
            Err(_) => {
                let _ = tx.blocking_send(NameSearchMsg::Done { loc_id: String::new() });
                return;
            }
        };
        let total = index.len();
        let _ = tx.blocking_send(NameSearchMsg::IndexProgress { processed: 0, total });
        let mut batch: Vec<SearchResult> = Vec::new();
        for (n, file) in index.iter().enumerate() {
            if n % 500 == 0 && cancel_rx.try_recv().is_ok() {
                return;
            }
            if n % 2000 == 1999 {
                let _ = tx.try_send(NameSearchMsg::IndexProgress { processed: n + 1, total });
            }
            if let Some(ref loc_id) = active_location {
                if &file.location_id != loc_id {
                    continue;
                }
            }
            let matches_target = match target {
                SearchTarget::Files => !file.is_dir,
                SearchTarget::Folders => file.is_dir,
                SearchTarget::Both | SearchTarget::Content => true,
            };
            if !matches_target {
                continue;
            }
            let filename = std::path::Path::new(&file.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| file.path.clone());
            if let Some(sr) = do_match(file, &filename, &query, consecutive, case_sensitive, &matcher, true) {
                batch.push(sr);
                if batch.len() >= 25 {
                    if tx.blocking_send(NameSearchMsg::Batch(std::mem::take(&mut batch))).is_err() {
                        return;
                    }
                }
            }
        }
        if !batch.is_empty() {
            let _ = tx.blocking_send(NameSearchMsg::Batch(batch));
        }
        let _ = tx.blocking_send(NameSearchMsg::IndexProgress { processed: total, total });
        let _ = tx.blocking_send(NameSearchMsg::Done { loc_id: String::new() });
    });
}

pub fn search_names_live_async(
    query: String,
    locations: Vec<crate::config::Location>,
    target: SearchTarget,
    consecutive: bool,
    case_sensitive: bool,
    tx: tokio::sync::mpsc::Sender<NameSearchMsg>,
    mut cancel_rx: tokio::sync::mpsc::Receiver<()>,
) {
    tokio::task::spawn_blocking(move || {
        let matcher = if case_sensitive {
            SkimMatcherV2::default().respect_case()
        } else {
            SkimMatcherV2::default()
        };
        let mut batch: Vec<SearchResult> = Vec::new();
        for location in locations.iter().filter(|l| l.enabled && l.kind == "local") {
            if cancel_rx.try_recv().is_ok() {
                return;
            }
            if tx.blocking_send(NameSearchMsg::Searching { loc_id: location.id.clone(), count: 0, current_dir: location.name.clone() }).is_err() {
                return;
            }
            let root = std::path::Path::new(&location.path);
            if !root.exists() {
                continue;
            }
            let walker = ignore::WalkBuilder::new(root)
                .hidden(true)
                .git_ignore(true)
                .build();
            let mut walk_count = 0usize;
            for entry in walker.flatten() {
                if cancel_rx.try_recv().is_ok() {
                    return;
                }
                let path = entry.path();
                let is_dir = path.is_dir();
                if path == root {
                    continue;
                }
                walk_count += 1;
                if walk_count % 200 == 0 {
                    let dir = if is_dir {
                        path.to_string_lossy().to_string()
                    } else {
                        path.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default()
                    };
                    if tx.blocking_send(NameSearchMsg::Searching { loc_id: location.id.clone(), count: walk_count, current_dir: dir }).is_err() {
                        return;
                    }
                }
                let matches_target = match target {
                    SearchTarget::Files => !is_dir,
                    SearchTarget::Folders => is_dir,
                    SearchTarget::Both | SearchTarget::Content => true,
                };
                if !matches_target {
                    continue;
                }
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let path_str = path.to_string_lossy().to_string();
                let file = IndexedFile {
                    location_id: location.id.clone(),
                    path: path_str.clone(),
                    display_path: path_str,
                    is_dir,
                };
                if let Some(sr) = do_match(&file, &filename, &query, consecutive, case_sensitive, &matcher, false) {
                    batch.push(sr);
                    if batch.len() >= 25 {
                        if tx.blocking_send(NameSearchMsg::Batch(std::mem::take(&mut batch))).is_err() {
                            return;
                        }
                    }
                }
            }
            // Mark this location complete (also fires for tiny folders with < 200 entries)
            let _ = tx.blocking_send(NameSearchMsg::Searching {
                loc_id: location.id.clone(),
                count: walk_count,
                current_dir: "Complete".to_string(),
            });
        }
        if !batch.is_empty() {
            let _ = tx.blocking_send(NameSearchMsg::Batch(batch));
        }
        let done_loc = locations.first().map(|l| l.id.clone()).unwrap_or_default();
        let _ = tx.blocking_send(NameSearchMsg::Done { loc_id: done_loc });
    });
}

pub fn search_names_ssh_async(
    query: String,
    location: crate::config::Location,
    credentials: std::collections::HashMap<String, String>,
    target: SearchTarget,
    consecutive: bool,
    case_sensitive: bool,
    tx: Sender<NameSearchMsg>,
    mut cancel_rx: tokio::sync::mpsc::Receiver<()>,
) {
    tokio::task::spawn_blocking(move || {
        fn log(msg: &str) {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
                .mode(0o600).open("/tmp/rum-ssh-search.log") {
                let _ = writeln!(f, "{}", msg);
            }
        }

        log(&format!("[ssh-search] task started for '{}'", location.name));

        // Send status immediately so the UI shows activity before the slow SSH connect
        let loc_id = location.id.clone();
        if tx.blocking_send(NameSearchMsg::Searching { loc_id: loc_id.clone(), count: 0, current_dir: format!("SSH: {} — connecting…", location.name) }).is_err() {
            log("[ssh-search] channel closed before first send");
            return;
        }

        let password = match credentials.get(&location.id) {
            Some(p) => p.clone(),
            None => {
                log(&format!("[ssh-search] no credentials for '{}'", location.name));
                let _ = tx.blocking_send(NameSearchMsg::Error(
                    format!("No credentials for '{}' — unlock first (Tab → Settings)", location.name)
                ));
                let _ = tx.blocking_send(NameSearchMsg::Done { loc_id: loc_id.clone() });
                return;
            }
        };
        log(&format!("[ssh-search] connecting to '{}'", location.name));
        let client = match SshClient::connect(&location, &password) {
            Ok(c) => { log(&format!("[ssh-search] connected to '{}'", location.name)); c }
            Err(e) => {
                log(&format!("[ssh-search] connect failed for '{}': {}", location.name, e));
                let _ = tx.blocking_send(NameSearchMsg::Error(
                    format!("SSH connect failed for '{}': {}", location.name, e)
                ));
                let _ = tx.blocking_send(NameSearchMsg::Done { loc_id: loc_id.clone() });
                return;
            }
        };

        let matcher = if case_sensitive {
            SkimMatcherV2::default().respect_case()
        } else {
            SkimMatcherV2::default()
        };
        let mut result_batch: Vec<SearchResult> = Vec::new();
        let mut cancelled = false;
        let mut dir_count = 0usize;

        // Helper closure to process one entry
        let mut process = |file_path: &str, is_dir: bool| {
            if cancelled { return; }
            if cancel_rx.try_recv().is_ok() { cancelled = true; return; }
            if is_dir { dir_count += 1; }

            let path_str = file_path.to_string();

            let matches_target = match target {
                SearchTarget::Files => !is_dir,
                SearchTarget::Folders => is_dir,
                SearchTarget::Both | SearchTarget::Content => true,
            };
            if !matches_target { return; }

            let filename = file_path.rsplit('/').next().unwrap_or(file_path);
            let file = crate::app::IndexedFile {
                location_id: loc_id.clone(),
                path: path_str.clone(),
                display_path: path_str,
                is_dir,
            };

            if let Some(sr) = do_match(&file, filename, &query, consecutive, case_sensitive, &matcher, false) {
                result_batch.push(sr);
                if result_batch.len() >= 25 {
                    if tx.blocking_send(NameSearchMsg::Batch(std::mem::take(&mut result_batch))).is_err() {
                        cancelled = true;
                    }
                }
            }
        };

        // Search files (unless dirs-only)
        let mut file_cb_count = 0usize;
        let mut dir_cb_count = 0usize;
        if !matches!(target, SearchTarget::Folders) {
            log(&format!("[ssh-search] scanning files on '{}'", location.name));
            if tx.blocking_send(NameSearchMsg::Searching { loc_id: loc_id.clone(), count: 0, current_dir: format!("SSH: {} — scanning files…", location.name) }).is_err() {
                return;
            }
            // Warmup: run a trivial command first to avoid SSH channel setup race.
            let _ = client.exec_streamed("true", |_| {});
            let _ = client.list_files_streamed(&location.path, |p| {
                file_cb_count += 1;
                if file_cb_count % 5000 == 0 {
                    let dir = p.rfind('/').map(|i| &p[..i]).unwrap_or(p).to_string();
                    let _ = tx.try_send(NameSearchMsg::Searching {
                        loc_id: loc_id.clone(),
                        count: file_cb_count,
                        current_dir: dir,
                    });
                }
                process(p, false);
            });
            log(&format!("[ssh-search] files scan done ({} files)", file_cb_count));
        }

        // Search dirs (unless files-only)
        if !matches!(target, SearchTarget::Files) {
            log(&format!("[ssh-search] scanning dirs on '{}'", location.name));
            if tx.blocking_send(NameSearchMsg::Searching { loc_id: loc_id.clone(), count: 0, current_dir: format!("SSH: {} — scanning dirs…", location.name) }).is_err() {
                return;
            }
            // Warmup: run a trivial command first; without this the first heavy find
            // may return EOF immediately due to an SSH channel setup race.
            let _ = client.exec_streamed("true", |_| {});
            let _ = client.list_dirs_streamed(&location.path, |p| {
                dir_cb_count += 1;
                if dir_cb_count % 5000 == 0 {
                    let _ = tx.try_send(NameSearchMsg::Searching {
                        loc_id: loc_id.clone(),
                        count: dir_cb_count,
                        current_dir: p.to_string(),
                    });
                }
                process(p, true);
            });
            log(&format!("[ssh-search] dirs scan done ({} dirs, {} pending matches)", dir_cb_count, result_batch.len()));
        }

        // Only send final messages if not cancelled — prevents spurious Done on the shared channel
        // when other live tasks are still running and the channel is kept open.
        if !cancelled {
            log(&format!("[ssh-search] sending Done for '{}'", location.name));
            if !result_batch.is_empty() { let _ = tx.blocking_send(NameSearchMsg::Batch(result_batch)); }
            let _ = tx.blocking_send(NameSearchMsg::Searching {
                loc_id: loc_id.clone(),
                count: file_cb_count + dir_cb_count,
                current_dir: "Complete".to_string(),
            });
            let _ = tx.blocking_send(NameSearchMsg::Done { loc_id: loc_id.clone() });
        } else {
            log(&format!("[ssh-search] cancelled for '{}', skipping Done", location.name));
        }
    });
}

/// Returns true when the query contains glob wildcard characters.
pub fn is_glob_query(query: &str) -> bool {
    query.contains('*') || query.contains('?')
}

/// Match a glob pattern against a file path.
/// If the pattern contains '/', it is matched against the full path; otherwise just the basename.
fn glob_match_path(pattern: &str, path: &str, case_sensitive: bool) -> bool {
    let target = if pattern.contains('/') {
        path
    } else {
        path.rsplit('/').next().unwrap_or(path)
    };

    // Fast pre-check: extract the literal prefix (before first wildcard) and
    // suffix (after last wildcard) and reject with starts_with/ends_with before
    // running the O(m*n) DP.  For patterns like `/some/path/*.ext` this skips
    // the vast majority of files in the index without touching the DP at all.
    let first_wild = pattern.find(['*', '?']).unwrap_or(pattern.len());
    let last_wild  = pattern.rfind(['*', '?']).map(|i| i + 1).unwrap_or(0);
    let prefix = &pattern[..first_wild];
    let suffix = &pattern[last_wild..];
    if !prefix.is_empty() || !suffix.is_empty() {
        let (cmp_t, cmp_p, cmp_s) = if case_sensitive {
            (target.to_string(), prefix.to_string(), suffix.to_string())
        } else {
            (target.to_lowercase(), prefix.to_lowercase(), suffix.to_lowercase())
        };
        if !prefix.is_empty() && !cmp_t.starts_with(&cmp_p) { return false; }
        if !suffix.is_empty() && !cmp_t.ends_with(&cmp_s) { return false; }
        // If the whole pattern is literals (no wildcards), prefix == pattern so
        // starts_with alone is a full match only when lengths also agree.
        if first_wild == pattern.len() { return cmp_t == cmp_p; }
    }

    let p: Vec<char> = if case_sensitive { pattern.chars().collect() }
                        else { pattern.to_lowercase().chars().collect() };
    let t: Vec<char> = if case_sensitive { target.chars().collect() }
                        else { target.to_lowercase().chars().collect() };
    glob_dp(&p, &t)
}

/// DP wildcard matcher: '*' matches any sequence, '?' matches one char.
fn glob_dp(p: &[char], t: &[char]) -> bool {
    let (m, n) = (p.len(), t.len());
    // Use a flat array instead of Vec<Vec<_>> to avoid per-row allocations.
    let mut dp = vec![false; (m + 1) * (n + 1)];
    let idx = |i: usize, j: usize| i * (n + 1) + j;
    dp[idx(0, 0)] = true;
    for i in 1..=m {
        if p[i - 1] == '*' { dp[idx(i, 0)] = dp[idx(i - 1, 0)]; }
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[idx(i, j)] = match p[i - 1] {
                '*' => dp[idx(i - 1, j)] || dp[idx(i, j - 1)],
                '?' => dp[idx(i - 1, j - 1)],
                c   => dp[idx(i - 1, j - 1)] && t[j - 1] == c,
            };
        }
    }
    dp[idx(m, n)]
}

/// Shared match logic for both indexed and live search.
fn do_match(
    file: &IndexedFile,
    filename: &str,
    query: &str,
    consecutive: bool,
    case_sensitive: bool,
    matcher: &SkimMatcherV2,
    is_indexed: bool,
) -> Option<SearchResult> {
    if is_glob_query(query) {
        if glob_match_path(query, &file.display_path, case_sensitive) {
            Some(SearchResult {
                file: file.clone(),
                match_type: MatchType::Name,
                score: 100,
                content_line: None,
                content_line_number: None,
                match_indices: vec![],
                is_indexed,
            })
        } else {
            None
        }
    } else if consecutive {
        // Match against full path when query contains '/', otherwise just the basename.
        let target = if query.contains('/') { file.display_path.as_str() } else { filename };
        let (cmp_target, cmp_q) = if case_sensitive {
            (target.to_string(), query.to_string())
        } else {
            (target.to_lowercase(), query.to_lowercase())
        };
        let offset = if query.contains('/') {
            0
        } else {
            let fname_char_len = filename.chars().count();
            let display_char_len = file.display_path.chars().count();
            display_char_len.saturating_sub(fname_char_len)
        };
        let q_chars: Vec<char> = cmp_q.chars().collect();
        let fn_chars: Vec<char> = cmp_target.chars().collect();
        let qlen = q_chars.len();
        let mut indices = Vec::new();
        let mut start = 0usize;
        while start + qlen <= fn_chars.len() {
            if fn_chars[start..start + qlen] == q_chars[..] {
                for i in 0..qlen {
                    indices.push(offset + start + i);
                }
                start += qlen;
            } else {
                start += 1;
            }
        }
        if indices.is_empty() {
            return None;
        }
        // Also highlight occurrences in the directory portion (when query has no '/').
        let all_indices = if query.contains('/') {
            indices
        } else {
            let cmp_full = if case_sensitive {
                file.display_path.clone()
            } else {
                file.display_path.to_lowercase()
            };
            let full_chars: Vec<char> = cmp_full.chars().collect();
            let dir_char_len = full_chars.len().saturating_sub(filename.chars().count());
            let mut all = Vec::new();
            let mut s = 0usize;
            while s + qlen <= dir_char_len {
                if full_chars[s..s + qlen] == q_chars[..] {
                    for i in 0..qlen { all.push(s + i); }
                    s += qlen;
                } else {
                    s += 1;
                }
            }
            all.extend(indices);
            all
        };
        Some(SearchResult {
            file: file.clone(),
            match_type: MatchType::Name,
            score: (qlen as i64) * 10,
            content_line: None,
            content_line_number: None,
            match_indices: all_indices,
            is_indexed,
        })
    } else {
        // Match against full path when query contains '/', otherwise just the basename.
        let target = if query.contains('/') { file.display_path.as_str() } else { filename };
        let (score, target_indices) = matcher.fuzzy_indices(target, query)?;
        let offset = if query.contains('/') {
            0
        } else {
            let fname_char_len = filename.chars().count();
            let display_char_len = file.display_path.chars().count();
            display_char_len.saturating_sub(fname_char_len)
        };
        let mut display_indices: Vec<usize> = target_indices.iter().map(|&i| i + offset).collect();
        // Also highlight consecutive occurrences in the directory portion (fuzzy only matches filename).
        if !query.contains('/') {
            let cmp_full = if case_sensitive {
                file.display_path.clone()
            } else {
                file.display_path.to_lowercase()
            };
            let cmp_q = if case_sensitive { query.to_string() } else { query.to_lowercase() };
            let full_chars: Vec<char> = cmp_full.chars().collect();
            let q_chars: Vec<char> = cmp_q.chars().collect();
            let qlen = q_chars.len();
            let dir_char_len = full_chars.len().saturating_sub(filename.chars().count());
            let mut s = 0usize;
            while s + qlen <= dir_char_len {
                if full_chars[s..s + qlen] == q_chars[..] {
                    for i in 0..qlen { display_indices.push(s + i); }
                    s += qlen;
                } else {
                    s += 1;
                }
            }
            display_indices.sort_unstable();
        }
        Some(SearchResult {
            file: file.clone(),
            match_type: MatchType::Name,
            score,
            content_line: None,
            content_line_number: None,
            match_indices: display_indices,
            is_indexed,
        })
    }
}

/// Re-score existing results against a new query for instant UI feedback while a
/// background search runs.  Name results that no longer match are dropped; content
/// results are kept as-is (they will be flushed when the new content search fires).
pub fn refilter_results(
    results: &[SearchResult],
    query: &str,
    consecutive: bool,
    case_sensitive: bool,
) -> Vec<SearchResult> {
    let matcher = if case_sensitive {
        SkimMatcherV2::default().respect_case()
    } else {
        SkimMatcherV2::default()
    };
    let mut out = Vec::with_capacity(results.len());
    for r in results {
        match r.match_type {
            MatchType::Name => {
                let filename = r.file.path.rsplit('/').next().unwrap_or(&r.file.path);
                if let Some(new_r) = do_match(&r.file, filename, query, consecutive, case_sensitive, &matcher, r.is_indexed) {
                    out.push(new_r);
                }
            }
            MatchType::Content => {
                // Keep stale content results visible until the new content search completes.
                out.push(r.clone());
            }
        }
    }
    out
}

pub async fn search_contents(
    query: String,
    locations: Vec<Location>,
    credentials: HashMap<String, String>,
    tx: Sender<SearchMsg>,
    mut cancel_rx: tokio::sync::mpsc::Receiver<()>,
    scan_counts: std::sync::Arc<std::sync::Mutex<HashMap<String, usize>>>,
) {
    tokio::task::spawn_blocking(move || {
        search_contents_blocking(&query, &locations, &credentials, &tx, &mut cancel_rx, &scan_counts);
        let _ = tx.blocking_send(SearchMsg::ContentDone);
    });
}

fn search_contents_blocking(
    query: &str,
    locations: &[Location],
    credentials: &HashMap<String, String>,
    tx: &Sender<SearchMsg>,
    cancel_rx: &mut tokio::sync::mpsc::Receiver<()>,
    scan_counts: &std::sync::Arc<std::sync::Mutex<HashMap<String, usize>>>,
) {


    // Mark all locations as "queued" upfront so the UI shows something for
    // every location, not just the one currently being searched.
    for location in locations.iter().filter(|l| l.enabled) {
        let _ = tx.blocking_send(SearchMsg::ContentLocationProgress {
            loc_id: location.id.clone(),
            count: 0,
            current_dir: "queued…".to_string(),
        });
    }

    for location in locations.iter().filter(|l| l.enabled) {
        if cancel_rx.try_recv().is_ok() {
            return;
        }

        if location.kind == "ssh" {
            let password = match credentials.get(&location.id) {
                Some(p) => p.clone(),
                None => continue,
            };

            let _ = tx.blocking_send(SearchMsg::ContentLocationProgress {
                loc_id: location.id.clone(),
                count: 0,
                current_dir: "connecting…".to_string(),
            });

            let client = match SshClient::connect(location, &password) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("SSH connect failed for content search {}: {}", location.name, e);
                    let _ = tx.blocking_send(SearchMsg::ContentLocationDone { loc_id: location.id.clone() });
                    continue;
                }
            };

            // Warmup: run a trivial command first; without this the first heavy grep
            // may return EOF immediately due to an SSH channel setup race.
            let _ = client.exec_streamed("true", |_| {});

            let _ = tx.blocking_send(SearchMsg::ContentLocationProgress {
                loc_id: location.id.clone(),
                count: 0,
                current_dir: "searching content…".to_string(),
            });

            let loc_id = location.id.clone();
            let loc_path = location.path.clone();
            let mut result_count = 0usize;
            let mut cancelled = false;
            let _ = client.grep_files_streamed(&loc_path, query, |file_path, lineno, content| {
                if cancelled { return; }
                if cancel_rx.try_recv().is_ok() { cancelled = true; return; }
                let display_path = location.display_path(&file_path);
                result_count += 1;
                // Send a progress message so the UI shows the last matched file path.
                // We don't have a files-scanned count for SSH grep, so just show the path.
                let _ = tx.blocking_send(SearchMsg::ContentLocationProgress {
                    loc_id: loc_id.clone(),
                    count: result_count,
                    current_dir: file_path.clone(),
                });
                let result = SearchResult {
                    file: IndexedFile {
                        location_id: loc_id.clone(),
                        path: file_path,
                        display_path,
                        is_dir: false,
                    },
                    match_type: MatchType::Content,
                    score: 0,
                    content_line: Some(content.trim().to_string()),
                    content_line_number: Some(lineno),
                    match_indices: vec![],
                    is_indexed: false,
                };
                if tx.blocking_send(SearchMsg::ContentResult(result)).is_err() {
                    cancelled = true;
                }
            });
            if cancelled { return; }
            let _ = tx.blocking_send(SearchMsg::ContentLocationDone { loc_id: location.id.clone() });
        } else {
            let root = std::path::Path::new(&location.path);
            if !root.exists() {
                continue;
            }

            let _ = tx.blocking_send(SearchMsg::ContentLocationProgress {
                loc_id: location.id.clone(),
                count: 0,
                current_dir: "searching…".to_string(),
            });

            let walker = ignore::WalkBuilder::new(root)
                .hidden(true)
                .git_ignore(true)
                .build();

            let mut file_count = 0usize;

            for entry in walker.flatten() {
                if cancel_rx.try_recv().is_ok() {
                    return;
                }

                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let path_str = path.to_string_lossy().to_string();
                file_count += 1;

                // Write scan count directly to shared counter; also send a progress message
                // every 200 files so the UI shows what directory is currently being scanned.
                if file_count % 200 == 0 {
                    if let Ok(mut m) = scan_counts.lock() { m.insert(location.id.clone(), file_count); }
                    // Show the directory currently being scanned so the user sees live feedback.
                    let dir = path.parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let _ = tx.blocking_send(SearchMsg::ContentLocationProgress {
                        loc_id: location.id.clone(),
                        count: file_count,
                        current_dir: dir,
                    });
                }

                if let Some(ext) = path.extension() {
                    let ext = ext.to_string_lossy().to_lowercase();
                    if matches!(
                        ext.as_str(),
                        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "ico" | "svg"
                            | "pdf" | "zip" | "tar" | "gz" | "bz2" | "xz"
                            | "exe" | "bin" | "so" | "dylib" | "dll"
                            | "mp3" | "mp4" | "avi" | "mkv" | "mov"
                            | "woff" | "woff2" | "ttf" | "otf"
                    ) {
                        continue;
                    }
                }

                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                for (line_idx, line) in content.lines().enumerate() {
                    if line.contains(query) {
                        let result = SearchResult {
                            file: IndexedFile {
                                location_id: location.id.clone(),
                                path: path_str.clone(),
                                display_path: path_str.clone(),
                                is_dir: false,
                            },
                            match_type: MatchType::Content,
                            score: 0,
                            content_line: Some(line.trim().to_string()),
                            content_line_number: Some(line_idx as u64 + 1),
                            match_indices: vec![],
                            is_indexed: false,
                        };
                        if tx.blocking_send(SearchMsg::ContentResult(result)).is_err() {
                            return;
                        }
                        break;
                    }
                }
            }
            let _ = tx.blocking_send(SearchMsg::ContentLocationDone { loc_id: location.id.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    fn make_location(dir: &TempDir) -> Location {
        Location {
            id: "test_loc".to_string(),
            name: "Test".to_string(),
            kind: "local".to_string(),
            path: dir.path().to_string_lossy().to_string(),
            ssh_host: None,
            ssh_port: 22,
            ssh_user: None,
            enabled: true,
        }
    }

    fn collect_results(rx: &mut mpsc::Receiver<SearchMsg>) -> Vec<SearchResult> {
        let mut results = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                SearchMsg::ContentResult(r) => results.push(r),
                SearchMsg::ContentLocationProgress { .. } | SearchMsg::ContentLocationDone { .. } => {}
                SearchMsg::ContentDone => break,
            }
        }
        results
    }

    #[tokio::test]
    async fn test_local_content_basic_match() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.txt"), "Hello world\nThis is a TODO item\nDone\n").unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let (_, cancel_rx) = mpsc::channel(1);
        let loc = make_location(&dir);
        search_contents("TODO".to_string(), vec![loc], HashMap::new(), tx, cancel_rx, std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))).await;

        // Give the blocking task a moment to run
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let results = collect_results(&mut rx);
        assert_eq!(results.len(), 1, "expected one result for TODO match");
        assert!(results[0].content_line.as_deref().unwrap_or("").contains("TODO"));
    }

    #[tokio::test]
    async fn test_local_content_no_match() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "Nothing interesting here\n").unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let (_, cancel_rx) = mpsc::channel(1);
        let loc = make_location(&dir);
        search_contents("XYZZY_NOTFOUND".to_string(), vec![loc], HashMap::new(), tx, cancel_rx, std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))).await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let results = collect_results(&mut rx);
        assert_eq!(results.len(), 0, "expected no results");
    }

    #[tokio::test]
    async fn test_local_content_special_chars_literal() {
        let dir = TempDir::new().unwrap();
        // "foo.bar" should match literally, not as a regex (regex would match "fooXbar")
        fs::write(dir.path().join("code.txt"), "call foo.bar() here\ncall fooXbar() here\n").unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let (_, cancel_rx) = mpsc::channel(1);
        let loc = make_location(&dir);
        search_contents("foo.bar".to_string(), vec![loc], HashMap::new(), tx, cancel_rx, std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))).await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let results = collect_results(&mut rx);
        // Local search uses str::contains which is literal — only the first line should match
        assert_eq!(results.len(), 1);
        assert!(results[0].content_line.as_deref().unwrap_or("").contains("foo.bar()"));
    }

    #[tokio::test]
    async fn test_local_content_case_sensitive() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "todo: fix this\nTODO: fix that\n").unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let (_, cancel_rx) = mpsc::channel(1);
        let loc = make_location(&dir);
        search_contents("TODO".to_string(), vec![loc], HashMap::new(), tx, cancel_rx, std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))).await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let results = collect_results(&mut rx);
        // Case-sensitive: only the uppercase TODO line matches
        assert_eq!(results.len(), 1);
        assert!(results[0].content_line.as_deref().unwrap_or("").contains("TODO:"));
    }

    #[tokio::test]
    async fn test_local_content_binary_skipped() {
        let dir = TempDir::new().unwrap();
        // Write a fake .jpg file (extension filter should skip it)
        fs::write(dir.path().join("image.jpg"), "TODO this is a fake jpg with text").unwrap();
        fs::write(dir.path().join("real.txt"), "TODO this is real text").unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let (_, cancel_rx) = mpsc::channel(1);
        let loc = make_location(&dir);
        search_contents("TODO".to_string(), vec![loc], HashMap::new(), tx, cancel_rx, std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))).await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let results = collect_results(&mut rx);
        // Only the .txt file should produce a result; .jpg is skipped
        assert_eq!(results.len(), 1);
        assert!(results[0].file.path.ends_with("real.txt"));
    }

    #[tokio::test]
    async fn test_local_content_multiple_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "MARKER in file a\n").unwrap();
        fs::write(dir.path().join("b.txt"), "MARKER in file b\n").unwrap();
        fs::write(dir.path().join("c.txt"), "nothing here\n").unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let (_, cancel_rx) = mpsc::channel(1);
        let loc = make_location(&dir);
        search_contents("MARKER".to_string(), vec![loc], HashMap::new(), tx, cancel_rx, std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))).await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let results = collect_results(&mut rx);
        assert_eq!(results.len(), 2, "expected results from both matching files");
    }

    #[tokio::test]
    async fn test_local_content_disabled_location_skipped() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "TODO something\n").unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let (_, cancel_rx) = mpsc::channel(1);
        let mut loc = make_location(&dir);
        loc.enabled = false;
        search_contents("TODO".to_string(), vec![loc], HashMap::new(), tx, cancel_rx, std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))).await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let results = collect_results(&mut rx);
        assert_eq!(results.len(), 0, "disabled location should produce no results");
    }
}
