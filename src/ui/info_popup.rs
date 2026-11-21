use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::app::{App, SshStatInfo};
use crate::ui::theme::{Theme, BG};

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let modal_width = 72u16.min(area.width.saturating_sub(4));
    let modal_height = area.height.saturating_sub(6).max(16).min(32);
    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;
    let modal_area = Rect::new(x, y, modal_width, modal_height);

    f.render_widget(Clear, modal_area);

    let visible = app.visible_results();
    let result: &crate::app::SearchResult = match &app.fb_info_result {
        Some(r) => r,
        None => match visible.get(app.selected) {
            Some(r) => r,
            None => return,
        },
    };

    let loc = app.config.locations.iter().find(|l| l.id == result.file.location_id);
    let is_ssh = loc.map(|l| l.kind == "ssh").unwrap_or(false);

    let fname = Path::new(&result.file.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&result.file.path);

    let title = format!(" {} ", fname);

    let block = Block::default()
        .style(Style::default().bg(BG))
        .borders(Borders::ALL)
        .border_style(Theme::progress_border())
        .title(Span::styled(title, Theme::progress_border().add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(
            " Tab/←→ Switch tab  ↑↓ Scroll  Esc Close ",
            Theme::dim(),
        ));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    if inner.height < 4 {
        return;
    }

    // Tab header row
    let tab_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let sep_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    let content_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2));

    let tab_labels: &[&str] = if is_ssh {
        &["  General  ", "  Times  ", "  Unix  ", "  Location  "]
    } else {
        &["  General  ", "  Times  ", "  Unix  "]
    };

    let mut tab_spans: Vec<Span> = vec![];
    for (i, &label) in tab_labels.iter().enumerate() {
        let style = if i == app.info_popup_tab {
            Theme::tab_active()
        } else {
            Theme::tab_inactive()
        };
        tab_spans.push(Span::styled(label, style));
        tab_spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(tab_spans)), tab_area);
    f.render_widget(
        Paragraph::new(Span::styled("─".repeat(inner.width as usize), Theme::dim())),
        sep_area,
    );

    if content_area.height == 0 {
        return;
    }

    let cw = content_area.width as usize;

    // Build lines for the current tab
    let lines: Vec<Line> = if is_ssh {
        match app.info_popup_tab {
            0 => build_general_lines(result, loc, cw),
            1 => build_ssh_times_lines(app.fb_info_ssh_stat.as_ref()),
            2 => build_ssh_unix_lines(app.fb_info_ssh_stat.as_ref()),
            3 => build_location_lines(result, loc, cw),
            _ => vec![],
        }
    } else {
        match app.info_popup_tab {
            0 => build_general_lines(result, loc, cw),
            1 => build_times_lines(&result.file.path),
            2 => build_unix_lines(&result.file.path),
            _ => vec![],
        }
    };

    let scroll = app.info_popup_scroll.min(lines.len().saturating_sub(content_area.height as usize));
    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();

    f.render_widget(
        Paragraph::new(visible_lines),
        content_area,
    );
}

// ── Line builders ────────────────────────────────────────────────────────────

fn build_general_lines<'a>(
    result: &crate::app::SearchResult,
    loc: Option<&crate::config::Location>,
    content_width: usize,
) -> Vec<Line<'a>> {
    let path = &result.file.path;
    let p = Path::new(path);

    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("—").to_string();
    let kind = if result.file.is_dir { "Directory" } else { "File" }.to_string();
    let loc_name = loc.map(|l| l.name.clone()).unwrap_or_else(|| "—".to_string());
    let display = result.file.display_path.clone();

    let size_str = if !result.file.is_dir {
        std::fs::metadata(path)
            .map(|m| format_size(m.len()))
            .unwrap_or_else(|_| "—".to_string())
    } else {
        "—  (directory)".to_string()
    };

    let symlink_target = if let Ok(target) = std::fs::read_link(path) {
        Some(target.to_string_lossy().to_string())
    } else {
        None
    };

    let mut lines = vec![
        kv("Name", &name),
        kv("Type", &kind),
        kv("Extension", &ext),
        kv("Size", &size_str),
        Line::raw(""),
        kv("Location", &loc_name),
    ];
    lines.extend(kv_path("Path", &display, content_width));

    if let Some(target) = symlink_target {
        lines.extend(kv_path("Symlink →", &target, content_width));
    }

    lines
}

fn build_ssh_times_lines<'a>(stat: Option<&Result<SshStatInfo, String>>) -> Vec<Line<'a>> {
    match stat {
        None => vec![Line::from(Span::styled("  Loading…", Theme::dim()))],
        Some(Err(e)) => vec![err_line(&format!("stat failed: {}", e))],
        Some(Ok(s)) => {
            vec![
                section("Timestamps"),
                kv("Modified", &format_epoch(s.modified)),
                kv("Accessed", &format_epoch(s.accessed)),
                kv("Changed",  &format_epoch(s.changed)),
            ]
        }
    }
}

fn build_ssh_unix_lines<'a>(stat: Option<&Result<SshStatInfo, String>>) -> Vec<Line<'a>> {
    match stat {
        None => vec![Line::from(Span::styled("  Loading…", Theme::dim()))],
        Some(Err(e)) => vec![err_line(&format!("stat failed: {}", e))],
        Some(Ok(s)) => {
            let owner = format!("{} (uid: {})", s.user_name, s.uid);
            let group = format!("{} (gid: {})", s.group_name, s.gid);
            let allocated = format!("{} allocated blocks × 512 B", s.blocks);
            vec![
                section("Permissions"),
                kv("Mode",       &s.mode_str),
                kv("Owner",      &owner),
                kv("Group",      &group),
                Line::raw(""),
                section("Filesystem"),
                kv("Inode",      &s.inode.to_string()),
                kv("Hard Links", &s.hard_links.to_string()),
                kv("Device ID",  &s.device_id.to_string()),
                kv("Block Size", &format_size(s.block_size)),
                kv("Allocated",  &allocated),
            ]
        }
    }
}

fn format_epoch(secs: i64) -> String {
    use chrono::TimeZone;
    match Local.timestamp_opt(secs, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M:%S %Z").to_string(),
        _ => format!("{} (unix)", secs),
    }
}

fn build_location_lines<'a>(
    result: &crate::app::SearchResult,
    loc: Option<&crate::config::Location>,
    content_width: usize,
) -> Vec<Line<'a>> {
    let mut lines = vec![];
    if let Some(loc) = loc {
        lines.push(kv("Name", &loc.name));
        lines.push(kv("Kind", &loc.kind));
        lines.extend(kv_path("Base Path", &loc.path, content_width));
        if let Some(ref host) = loc.ssh_host {
            lines.push(kv("SSH Host", host));
            lines.push(kv("SSH Port", &loc.ssh_port.to_string()));
            if let Some(ref user) = loc.ssh_user {
                lines.push(kv("SSH User", user));
            }
        }
        lines.push(Line::raw(""));
        lines.extend(kv_path("File Path", &result.file.display_path, content_width));
        lines.push(section("Note"));
        lines.push(Line::from(Span::styled(
            "  Full metadata requires a live SSH connection.",
            Theme::dim(),
        )));
    }
    lines
}

fn build_times_lines<'a>(path: &str) -> Vec<Line<'a>> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return vec![err_line(&e.to_string())],
    };

    let mut lines = vec![section("Timestamps")];

    lines.push(kv("Modified", &format_time(meta.modified())));
    lines.push(kv("Accessed", &format_time(meta.accessed())));
    lines.push(kv("Created", &format_time(meta.created())));

    lines
}

fn build_unix_lines<'a>(path: &str) -> Vec<Line<'a>> {
    use std::os::unix::fs::MetadataExt;

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return vec![err_line(&e.to_string())],
    };

    let mode = meta.mode();
    let perm_str = format!("{} ({:04o})", mode_string(mode), mode & 0o7777);
    let uid = meta.uid();
    let gid = meta.gid();
    let owner = format!("{} (uid: {})", uid_name(uid), uid);
    let group = format!("{} (gid: {})", gid_name(gid), gid);
    let blocks_size = format!("{} bytes ({} allocated blocks × 512 B)",
        format_size(meta.blocks() * 512), meta.blocks());

    vec![
        section("Permissions"),
        kv("Mode", &perm_str),
        kv("Owner", &owner),
        kv("Group", &group),
        Line::raw(""),
        section("Filesystem"),
        kv("Inode", &meta.ino().to_string()),
        kv("Hard Links", &meta.nlink().to_string()),
        kv("Device ID", &meta.dev().to_string()),
        kv("Block Size", &format_size(meta.blksize())),
        kv("Allocated", &blocks_size),
    ]
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn kv<'a>(key: &str, val: &str) -> Line<'a> {
    let key_w = 14usize;
    let key_str = format!("  {:<width$}", key, width = key_w);
    Line::from(vec![
        Span::styled(key_str, Theme::dim()),
        Span::styled(val.to_string(), Theme::normal()),
    ])
}

/// Like `kv` but wraps long values across multiple lines, splitting at '/' where possible.
fn kv_path<'a>(key: &str, val: &str, content_width: usize) -> Vec<Line<'a>> {
    let key_col = 2 + 14usize; // "  " + 14-char key field
    let val_width = content_width.saturating_sub(key_col).max(10);
    let key_str = format!("  {:<14}", key);
    let indent = " ".repeat(key_col);

    let chunks = wrap_at_slashes(val, val_width);
    chunks.into_iter().enumerate().map(|(i, chunk)| {
        let prefix = if i == 0 { key_str.clone() } else { indent.clone() };
        Line::from(vec![
            Span::styled(prefix, Theme::dim()),
            Span::styled(chunk, Theme::normal()),
        ])
    }).collect()
}

/// Split a path string into lines of at most `max_chars`, preferring '/' break points.
fn wrap_at_slashes(val: &str, max_chars: usize) -> Vec<String> {
    if val.chars().count() <= max_chars {
        return vec![val.to_string()];
    }
    let mut lines = vec![];
    let mut remaining = val;
    while !remaining.is_empty() {
        if remaining.chars().count() <= max_chars {
            lines.push(remaining.to_string());
            break;
        }
        // Take up to max_chars characters and try to break at the last '/'.
        let end_byte = remaining.char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());
        let slice = &remaining[..end_byte];
        let break_byte = slice.rfind('/')
            .map(|i| i + 1)   // include the '/' on the current line
            .unwrap_or(end_byte); // hard break if no '/' found
        lines.push(remaining[..break_byte].to_string());
        remaining = &remaining[break_byte..];
    }
    lines
}

fn section<'a>(label: &str) -> Line<'a> {
    Line::from(Span::styled(
        format!("  {}", label),
        Theme::section_header(),
    ))
}

fn err_line<'a>(msg: &str) -> Line<'a> {
    Line::from(Span::styled(
        format!("  Error: {}", msg),
        Theme::error(),
    ))
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut idx = 0;
    while size >= 1024.0 && idx < UNITS.len() - 1 {
        size /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", size, UNITS[idx])
    }
}

fn format_time(t: std::io::Result<SystemTime>) -> String {
    match t {
        Err(_) => "—".to_string(),
        Ok(st) => {
            let dt: DateTime<Local> = st.into();
            dt.format("%Y-%m-%d %H:%M:%S %Z").to_string()
        }
    }
}

fn mode_string(mode: u32) -> String {
    let mut s = String::with_capacity(10);
    s.push(match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    });
    // owner
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o4000 != 0 {
        if mode & 0o100 != 0 { 's' } else { 'S' }
    } else if mode & 0o100 != 0 { 'x' } else { '-' });
    // group
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o2000 != 0 {
        if mode & 0o010 != 0 { 's' } else { 'S' }
    } else if mode & 0o010 != 0 { 'x' } else { '-' });
    // other
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o1000 != 0 {
        if mode & 0o001 != 0 { 't' } else { 'T' }
    } else if mode & 0o001 != 0 { 'x' } else { '-' });
    s
}

fn uid_name(uid: u32) -> String {
    std::fs::read_to_string("/etc/passwd").ok()
        .and_then(|content| {
            content.lines()
                .find(|line| {
                    let mut parts = line.splitn(4, ':');
                    let _ = parts.next(); // name
                    let _ = parts.next(); // password
                    parts.next().and_then(|id| id.parse::<u32>().ok()) == Some(uid)
                })
                .and_then(|line| line.split(':').next().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| uid.to_string())
}

fn gid_name(gid: u32) -> String {
    std::fs::read_to_string("/etc/group").ok()
        .and_then(|content| {
            content.lines()
                .find(|line| {
                    let mut parts = line.splitn(4, ':');
                    let _ = parts.next(); // name
                    let _ = parts.next(); // password
                    parts.next().and_then(|id| id.parse::<u32>().ok()) == Some(gid)
                })
                .and_then(|line| line.split(':').next().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| gid.to_string())
}
