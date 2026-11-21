use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, UNIX_EPOCH};

use tokio::sync::mpsc;

use crate::app::{FbColorSpan, FbEntry, FbMsg, FbPaneTarget, SshStatInfo};
use crate::config::Location;
use crate::ssh_client::SshClient;

// ── Syntax highlighting ──────────────────────────────────────────────────────

static GRUVBOX_THEME_BYTES: &[u8] = include_bytes!("gruvbox-dark.tmTheme");

// Default syntect syntax set (unchanged from bundled defaults).
static SYNTAX_SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
// Custom syntax set for languages not in the defaults (Nix, Python, etc.).
// Kept separate so we never mutate/rebuild the defaults, which can break cross-syntax links.
static CUSTOM_SYNTAX_SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
static GRUVBOX_THEME: OnceLock<syntect::highlighting::Theme> = OnceLock::new();

fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    SYNTAX_SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

fn custom_syntax_set() -> &'static syntect::parsing::SyntaxSet {
    CUSTOM_SYNTAX_SET.get_or_init(|| {
        use syntect::parsing::{SyntaxDefinition, SyntaxSetBuilder};
        let mut builder = SyntaxSetBuilder::new();
        builder.add_plain_text_syntax();
        for src in &[
            include_str!("nix.sublime-syntax"),
            include_str!("python.sublime-syntax"),
            include_str!("ini.sublime-syntax"),
            include_str!("csv.sublime-syntax"),
        ] {
            if let Ok(def) = SyntaxDefinition::load_from_str(src, true, None) {
                builder.add(def);
            }
        }
        builder.build()
    })
}

fn gruvbox_theme() -> &'static syntect::highlighting::Theme {
    GRUVBOX_THEME.get_or_init(|| {
        use std::io::Cursor;
        syntect::highlighting::ThemeSet::load_from_reader(&mut Cursor::new(GRUVBOX_THEME_BYTES))
            .unwrap_or_else(|_| {
                syntect::highlighting::ThemeSet::load_defaults()
                    .themes
                    .remove("base16-eighties.dark")
                    .expect("fallback theme")
            })
    })
}

/// Highlight `content` for the given `file_path` (extension used for language detection).
/// Returns up to `max_lines` highlighted lines, each as a vec of (text, rgb) spans.
pub fn highlight_content(file_path: &str, content: &str, max_lines: usize) -> Vec<Vec<FbColorSpan>> {
    use syntect::easy::HighlightLines;

    let theme = gruvbox_theme();

    let ext = Path::new(file_path).extension().and_then(|e| e.to_str()).unwrap_or("");

    // Remap extensions that share a syntax with another extension.
    let ext = match ext {
        "ino" => "cpp",  // Arduino C++ → standard C++ highlighter
        other  => other,
    };

    // Prefer custom set (Nix, Python, INI, CSV…), fall back to defaults for everything else.
    let custom = custom_syntax_set();
    let (syntax, ss) = if let Some(syn) = custom.find_syntax_by_extension(ext) {
        (syn, custom)
    } else {
        let defaults = syntax_set();
        let syn = defaults.find_syntax_by_extension(ext)
            .or_else(|| content.lines().next().and_then(|l| defaults.find_syntax_by_first_line(l)))
            .unwrap_or_else(|| defaults.find_syntax_plain_text());
        (syn, defaults)
    };

    let mut h = HighlightLines::new(syntax, theme);
    let mut result = Vec::with_capacity(max_lines);

    for line in content.split_inclusive('\n').take(max_lines) {
        let ranges = h.highlight_line(line, ss).unwrap_or_default();
        let spans: Vec<FbColorSpan> = ranges.iter()
            .filter_map(|(style, text)| {
                if text.is_empty() { return None; }
                let fg = style.foreground;
                Some((text.to_string(), (fg.r, fg.g, fg.b)))
            })
            .collect();
        result.push(spans);
    }
    result
}

pub fn launch_local_file_content(path: String, tx: mpsc::Sender<FbMsg>) {
    tokio::task::spawn_blocking(move || {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                // Try reading as bytes and lossily converting (for binary/non-UTF8)
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        if bytes.iter().take(512).any(|&b| b == 0) {
                            // Binary file
                            let _ = tx.blocking_send(FbMsg::FileContent {
                                path,
                                lines: vec![vec![("  [binary file]".to_string(), (100, 100, 100))]],
                            });
                            return;
                        }
                        String::from_utf8_lossy(&bytes).into_owned()
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(FbMsg::FileContent {
                            path,
                            lines: vec![vec![(format!("  Error: {}", e), (200, 50, 50))]],
                        });
                        return;
                    }
                }
            }
        };
        let lines = highlight_content(&path, &content, 500);
        let _ = tx.blocking_send(FbMsg::FileContent { path, lines });
    });
}

pub fn launch_ssh_file_content(path: String, loc: Location, password: String, tx: mpsc::Sender<FbMsg>) {
    tokio::task::spawn_blocking(move || {
        let client = match SshClient::connect(&loc, &password) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.blocking_send(FbMsg::Error(e.to_string()));
                return;
            }
        };
        let _ = client.exec_streamed("true", |_| {});

        // Read up to 500 lines via head -n 500
        let escaped = shell_escape(&path);
        let cmd = format!("head -n 500 {} 2>/dev/null", escaped);
        let mut content_lines: Vec<String> = Vec::new();
        let _ = client.exec_streamed(&cmd, |line| {
            content_lines.push(line.to_string());
        });

        let content = content_lines.join("\n");
        let lines = highlight_content(&path, &content, 500);
        let _ = tx.blocking_send(FbMsg::FileContent { path, lines });
    });
}

pub fn sort_entries(entries: &mut Vec<FbEntry>) {
    entries.sort_by(|a, b| {
        // Dirs first, then alphabetical case-insensitive, hidden files last within each group
        let a_hidden = a.name.starts_with('.');
        let b_hidden = b.name.starts_with('.');
        b.is_dir.cmp(&a.is_dir)
            .then_with(|| a_hidden.cmp(&b_hidden))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

pub fn launch_local_load(
    pane: FbPaneTarget,
    path: String,
    tx: mpsc::Sender<FbMsg>,
) {
    tokio::task::spawn_blocking(move || {
        let entries = list_local_dir(&path);
        let _ = tx.blocking_send(FbMsg::Entries { pane, path, entries });
    });
}

fn list_local_dir(path: &str) -> Vec<FbEntry> {
    let mut entries: Vec<FbEntry> = match std::fs::read_dir(path) {
        Ok(dir) => dir
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let meta = e.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = meta.and_then(|m| m.modified().ok());
                FbEntry { name, is_dir, size, modified }
            })
            .collect(),
        Err(_) => vec![],
    };
    sort_entries(&mut entries);
    entries
}

pub fn launch_ssh_load(
    pane: FbPaneTarget,
    path: String,
    loc: Location,
    password: String,
    tx: mpsc::Sender<FbMsg>,
) {
    tokio::task::spawn_blocking(move || {
        let result = SshClient::connect(&loc, &password);
        match result {
            Err(e) => {
                let _ = tx.blocking_send(FbMsg::Error(e.to_string()));
            }
            Ok(client) => {
                // SSH channel warmup required before first real command
                let _ = client.exec_streamed("true", |_| {});
                let entries = list_ssh_dir(&client, &path);
                let _ = tx.blocking_send(FbMsg::Entries { pane, path, entries });
            }
        }
    });
}

fn list_ssh_dir(client: &SshClient, path: &str) -> Vec<FbEntry> {
    let escaped = shell_escape(path);
    // %y=type char (d/f/l/etc), %s=size, %T@=mtime unix float, %f=filename only
    let cmd = format!(
        "find {} -maxdepth 1 -mindepth 1 -printf '%y\\t%s\\t%T@\\t%f\\n' 2>/dev/null",
        escaped
    );
    let mut entries = Vec::new();
    let _ = client.exec_streamed(&cmd, |line| {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() == 4 {
            let type_char = parts[0];
            let size: u64 = parts[1].parse().unwrap_or(0);
            let mtime_secs: f64 = parts[2].parse().unwrap_or(0.0);
            let name = parts[3].to_string();
            if name == "." || name == ".." || name.is_empty() {
                return;
            }
            let is_dir = type_char == "d";
            let modified = if mtime_secs > 0.0 {
                UNIX_EPOCH.checked_add(Duration::from_secs_f64(mtime_secs))
            } else {
                None
            };
            entries.push(FbEntry { name, is_dir, size, modified });
        }
    });
    sort_entries(&mut entries);
    entries
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Fetch file metadata via `stat` over SSH and send the result back as `FbMsg::SshStat`.
pub fn launch_ssh_stat(path: String, loc: Location, password: String, tx: mpsc::Sender<FbMsg>) {
    tokio::task::spawn_blocking(move || {
        let result = fetch_ssh_stat(&path, &loc, &password);
        let _ = tx.blocking_send(FbMsg::SshStat { path, info: result });
    });
}

fn fetch_ssh_stat(path: &str, loc: &Location, password: &str) -> Result<SshStatInfo, String> {
    let client = SshClient::connect(loc, password).map_err(|e| e.to_string())?;
    let _ = client.exec_streamed("true", |_| {});
    let escaped = shell_escape(path);
    // Tab-separated: mode_str, uid, user, gid, group, size, inode, hard_links, dev, blksize, blocks, atime, mtime, ctime
    // --printf (not --format) interprets \t as a real tab character
    let cmd = format!(
        "stat --printf='%A\\t%u\\t%U\\t%g\\t%G\\t%s\\t%i\\t%h\\t%d\\t%B\\t%b\\t%X\\t%Y\\t%Z\\n' {}",
        escaped
    );
    let output = client.exec(&cmd).map_err(|e| e.to_string())?;
    let line = output.trim();
    let parts: Vec<&str> = line.splitn(14, '\t').collect();
    if parts.len() < 14 {
        return Err(format!("unexpected stat output: {:?}", line));
    }
    Ok(SshStatInfo {
        mode_str:    parts[0].to_string(),
        uid:         parts[1].parse().unwrap_or(0),
        user_name:   parts[2].to_string(),
        gid:         parts[3].parse().unwrap_or(0),
        group_name:  parts[4].to_string(),
        size:        parts[5].parse().unwrap_or(0),
        inode:       parts[6].parse().unwrap_or(0),
        hard_links:  parts[7].parse().unwrap_or(0),
        device_id:   parts[8].parse().unwrap_or(0),
        block_size:  parts[9].parse().unwrap_or(0),
        blocks:      parts[10].parse().unwrap_or(0),
        accessed:    parts[11].parse().unwrap_or(0),
        modified:    parts[12].parse().unwrap_or(0),
        changed:     parts[13].parse().unwrap_or(0),
    })
}
