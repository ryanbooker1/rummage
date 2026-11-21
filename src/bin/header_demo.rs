/// Demo: 10 header layout alternatives.
/// Run with: nix develop --command cargo run --bin header_demo

use std::io;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Terminal,
};

const BG:     Color = Color::Rgb(40,  40,  40);
const BG1:    Color = Color::Rgb(60,  56,  54);
const BG2:    Color = Color::Rgb(80,  73,  69);
const BG4:    Color = Color::Rgb(124, 111, 100);
const FG:     Color = Color::Rgb(235, 219, 178);
const FG4:    Color = Color::Rgb(146, 131, 116);
const GREEN:  Color = Color::Rgb(184, 187, 38);
const YELLOW: Color = Color::Rgb(250, 189, 47);
const BLUE:   Color = Color::Rgb(131, 165, 152);
const AQUA:   Color = Color::Rgb(142, 192, 124);
const ORANGE: Color = Color::Rgb(254, 128, 25);
const PURPLE: Color = Color::Rgb(211, 134, 155);

fn fg(c: Color) -> Style { Style::default().fg(c).bg(BG) }
fn bold(c: Color) -> Style { Style::default().fg(c).bg(BG).add_modifier(Modifier::BOLD) }
fn inv(fc: Color, bc: Color) -> Style { Style::default().fg(fc).bg(bc) }
fn inv_bold(fc: Color, bc: Color) -> Style { Style::default().fg(fc).bg(bc).add_modifier(Modifier::BOLD) }

fn sp(c: Color) -> Span<'static> { Span::styled(" ", Style::default().fg(c).bg(BG)) }

fn row(f_: &mut ratatui::Frame, area: Rect, dy: u16, line: Line<'static>) {
    if dy < area.height {
        f_.render_widget(
            Paragraph::new(line).style(Style::default().bg(BG)),
            Rect::new(area.x, area.y + dy, area.width, 1),
        );
    }
}

const NAMES: &[&str] = &[
    "1 — Minimal  ·  dot-separated single line",
    "2 — Two-row  ·  name+count / settings",
    "3 — Powerline  ·  colored segments",
    "4 — Outlined card",
    "5 — Inverse banner  ·  full-width color strip",
    "6 — Mode-prominent  ·  editor style",
    "7 — Toggle indicators  ·  ◉ / ○",
    "8 — Ruled  ·  title / separator / detail",
    "9 — Breadcrumb  ·  chevron-separated path",
    "10 — Compact bar  ·  right-aligned count",
    "11 — Card outline + powerline interior",
];

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut current = 0usize;

    loop {
        terminal.draw(|f| {
            let area = f.area();
            f.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);

            // Nav bar
            let nav = format!("  ← → / h l  cycle   q  quit   {} / {}:  {}",
                current + 1, NAMES.len(), NAMES[current]);
            f.render_widget(
                Paragraph::new(Span::styled(nav, Style::default().fg(FG4).bg(BG1))),
                Rect::new(area.x, area.y, area.width, 1),
            );

            // 3-row header preview
            if area.height >= 5 {
                let header = Rect::new(area.x, area.y + 2, area.width, 3);
                match current {
                    0 => opt1(f, header),
                    1 => opt2(f, header),
                    2 => opt3(f, header),
                    3 => opt4(f, header),
                    4 => opt5(f, header),
                    5 => opt6(f, header),
                    6 => opt7(f, header),
                    7 => opt8(f, header),
                    8 => opt9(f, header),
                    9 => opt10(f, header),
                    10 => opt11(f, header),
                    _ => {}
                }
            }

            // Separator + state note
            if area.height >= 7 {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        "─".repeat(area.width as usize),
                        Style::default().fg(BG2),
                    )),
                    Rect::new(area.x, area.y + 5, area.width, 1),
                );
            }
            if area.height >= 9 {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        "  Simulated state:  42 results  ·  mode:index  ·  target:files  ·  content:off  ·  case:off  ·  consec:off",
                        Style::default().fg(BG4),
                    )),
                    Rect::new(area.x, area.y + 7, area.width, 1),
                );
            }
        })?;

        if let Event::Key(k) = event::read()? {
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Right | KeyCode::Char('l') => current = (current + 1) % NAMES.len(),
                KeyCode::Left  | KeyCode::Char('h') => current = (current + NAMES.len() - 1) % NAMES.len(),
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

// ── Option 1: Minimal dot-separated ─────────────────────────────────────────
// Single info line centered vertically in the 3-row block.
fn opt1(f: &mut ratatui::Frame, area: Rect) {
    row(f, area, 0, Line::from(""));
    row(f, area, 1, Line::from(vec![
        Span::styled(" bs", bold(AQUA)),
        sp(BG),
        Span::styled("│", fg(BG2)),
        sp(BG),
        Span::styled("42 results", bold(FG)),
        sp(BG),
        Span::styled("│", fg(BG2)),
        sp(BG),
        Span::styled("index", fg(YELLOW)),
        sp(BG),
        Span::styled("·", fg(BG4)),
        sp(BG),
        Span::styled("files", fg(YELLOW)),
        sp(BG),
        Span::styled("│", fg(BG2)),
        sp(BG),
        Span::styled("content:off", fg(FG4)),
        sp(BG),
        Span::styled("·", fg(BG4)),
        sp(BG),
        Span::styled("case:off", fg(FG4)),
        sp(BG),
        Span::styled("·", fg(BG4)),
        sp(BG),
        Span::styled("consec:off", fg(FG4)),
    ]));
    row(f, area, 2, Line::from(""));
}

// ── Option 2: Two-row name+count / settings ──────────────────────────────────
fn opt2(f: &mut ratatui::Frame, area: Rect) {
    row(f, area, 0, Line::from(vec![
        Span::styled(" booker-search", bold(AQUA)),
        Span::styled("  ─────────────────────────────────────────────────  ", fg(BG2)),
        Span::styled("42 results ", bold(FG)),
    ]));
    row(f, area, 1, Line::from(vec![
        Span::styled("  mode:", fg(FG4)),
        Span::styled("index", fg(YELLOW)),
        Span::styled("   target:", fg(FG4)),
        Span::styled("files", fg(YELLOW)),
        Span::styled("   content:", fg(FG4)),
        Span::styled("off", fg(FG4)),
        Span::styled("   case:", fg(FG4)),
        Span::styled("off", fg(FG4)),
        Span::styled("   consec:", fg(FG4)),
        Span::styled("off", fg(FG4)),
    ]));
    row(f, area, 2, Line::from(""));
}

// ── Option 3: Powerline colored segments ────────────────────────────────────
fn opt3(f: &mut ratatui::Frame, area: Rect) {
    row(f, area, 0, Line::from(""));
    row(f, area, 1, Line::from(vec![
        Span::styled(" bs ", inv_bold(BG, AQUA)),
        Span::styled("▶", inv(AQUA, BG1)),
        Span::styled(" 42 results ", inv(FG, BG1)),
        Span::styled("▶", inv(BG1, BG2)),
        Span::styled(" INDEX ", inv_bold(YELLOW, BG2)),
        Span::styled("▶", inv(BG2, BG1)),
        Span::styled(" FILES ", inv(FG, BG1)),
        Span::styled("▶", inv(BG1, BG)),
        Span::styled("  content:off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("case:off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("consec:off", fg(FG4)),
    ]));
    row(f, area, 2, Line::from(""));
}

// ── Option 4: Outlined card ──────────────────────────────────────────────────
fn opt4(f: &mut ratatui::Frame, area: Rect) {
    let w = area.width as usize;
    let inner = w.saturating_sub(2);
    let top    = format!("╭─ bs ─{}─ 42 results ─╮", "─".repeat(inner.saturating_sub(20)));
    let middle = format!("│  mode:index   target:files   content:off   case:off{:>pad$}│",
        "", pad = inner.saturating_sub(50));
    let bottom = format!("╰{}╯", "─".repeat(inner));
    row(f, area, 0, Line::from(Span::styled(top,    fg(AQUA))));
    row(f, area, 1, Line::from(vec![
        Span::styled("│", fg(AQUA)),
        Span::styled("  mode:", fg(FG4)),
        Span::styled("index", fg(YELLOW)),
        Span::styled("   target:", fg(FG4)),
        Span::styled("files", fg(YELLOW)),
        Span::styled("   content:off   case:off   consec:off", fg(FG4)),
    ]));
    row(f, area, 2, Line::from(Span::styled(bottom, fg(AQUA))));
    let _ = middle; // used for sizing reference only
}

// ── Option 5: Inverse banner ─────────────────────────────────────────────────
fn opt5(f: &mut ratatui::Frame, area: Rect) {
    // Full-width inverse colour strip for row 0
    let w = area.width as usize;
    let title = " booker-search";
    let count = "42 results ";
    let pad = w.saturating_sub(title.len() + count.len());
    row(f, area, 0, Line::from(vec![
        Span::styled(title,                           inv_bold(BG, BLUE)),
        Span::styled(" ".repeat(pad),                 inv(BG, BLUE)),
        Span::styled(count,                           inv_bold(BG, BLUE)),
    ]));
    row(f, area, 1, Line::from(vec![
        Span::styled("  ", fg(BG)),
        Span::styled("index", inv_bold(BG, YELLOW)),
        Span::styled("  ", fg(BG)),
        Span::styled("files", inv_bold(BG, GREEN)),
        Span::styled("  content:off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("case:off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("consec:off", fg(FG4)),
    ]));
    row(f, area, 2, Line::from(""));
}

// ── Option 6: Mode-prominent / editor style ──────────────────────────────────
fn opt6(f: &mut ratatui::Frame, area: Rect) {
    row(f, area, 0, Line::from(""));
    row(f, area, 1, Line::from(vec![
        Span::styled(" INDEX ", inv_bold(BG, ORANGE)),
        Span::styled("▶", inv(ORANGE, BG)),
        Span::styled("  FILES ", inv_bold(BG1, BG)),
        Span::styled("│", fg(BG2)),
        Span::styled("  bs  ", bold(AQUA)),
        Span::styled("  42 results", bold(FG)),
        Span::styled("   content:off  case:off  consec:off", fg(FG4)),
    ]));
    let w = area.width as usize;
    row(f, area, 2, Line::from(Span::styled(
        format!(" {}", "─".repeat(w.saturating_sub(1))),
        fg(ORANGE),
    )));
}

// ── Option 7: Toggle indicators ◉ / ○ ───────────────────────────────────────
fn opt7(f: &mut ratatui::Frame, area: Rect) {
    row(f, area, 0, Line::from(""));
    row(f, area, 1, Line::from(vec![
        Span::styled(" bs", bold(AQUA)),
        Span::styled("   42 results", bold(FG)),
        Span::styled("     ", fg(BG)),
        Span::styled("◉ ", fg(YELLOW)),
        Span::styled("INDEX", fg(YELLOW)),
        Span::styled("   ", fg(BG)),
        Span::styled("◉ ", fg(GREEN)),
        Span::styled("FILES", fg(GREEN)),
        Span::styled("   ", fg(BG)),
        Span::styled("○ ", fg(BG4)),
        Span::styled("CONTENT", fg(FG4)),
        Span::styled("   ", fg(BG)),
        Span::styled("○ ", fg(BG4)),
        Span::styled("CASE", fg(FG4)),
        Span::styled("   ", fg(BG)),
        Span::styled("○ ", fg(BG4)),
        Span::styled("CONSEC", fg(FG4)),
    ]));
    row(f, area, 2, Line::from(""));
}

// ── Option 8: Ruled — title / separator / detail ────────────────────────────
fn opt8(f: &mut ratatui::Frame, area: Rect) {
    let w = area.width as usize;
    row(f, area, 0, Line::from(vec![
        Span::styled(" booker-search", bold(AQUA)),
        Span::styled("   ─── ", fg(BG2)),
        Span::styled("42 results", bold(FG)),
    ]));
    row(f, area, 1, Line::from(Span::styled(
        "─".repeat(w),
        fg(BG2),
    )));
    row(f, area, 2, Line::from(vec![
        Span::styled(" mode:", fg(FG4)),
        Span::styled("index", fg(YELLOW)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("target:", fg(FG4)),
        Span::styled("files", fg(YELLOW)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("content:", fg(FG4)),
        Span::styled("off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("case:", fg(FG4)),
        Span::styled("off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("consec:", fg(FG4)),
        Span::styled("off", fg(FG4)),
    ]));
}

// ── Option 9: Breadcrumb chevron path ───────────────────────────────────────
fn opt9(f: &mut ratatui::Frame, area: Rect) {
    row(f, area, 0, Line::from(""));
    row(f, area, 1, Line::from(vec![
        Span::styled(" bs", bold(AQUA)),
        Span::styled("  ›  ", fg(BG4)),
        Span::styled("index", fg(YELLOW)),
        Span::styled("  ›  ", fg(BG4)),
        Span::styled("files", fg(YELLOW)),
        Span::styled("  ›  ", fg(BG4)),
        Span::styled("content:off", fg(FG4)),
        Span::styled("  ›  ", fg(BG4)),
        Span::styled("case:off", fg(FG4)),
        Span::styled("  ›  ", fg(BG4)),
        Span::styled("consec:off", fg(FG4)),
        Span::styled("     ", fg(BG)),
        Span::styled("42 results", bold(FG)),
    ]));
    row(f, area, 2, Line::from(""));
}

// ── Option 10: Compact bar — right-aligned count ────────────────────────────
fn opt10(f: &mut ratatui::Frame, area: Rect) {
    let w = area.width as usize;
    // Build left side
    let left_spans: Vec<Span> = vec![
        Span::styled(" bs", bold(AQUA)),
        Span::styled("   ", fg(BG)),
        Span::styled("INDEX", inv_bold(BG, BG2)),
        Span::styled(" ", fg(BG)),
        Span::styled("FILES", inv_bold(BG, BG2)),
        Span::styled("   content:off  case:off  consec:off", fg(FG4)),
    ];
    let left_w: usize = left_spans.iter().map(|s| s.content.len()).sum();
    let count_str = " 42 results ";
    let pad = w.saturating_sub(left_w + count_str.len());

    row(f, area, 0, Line::from(""));
    let mut spans = left_spans;
    spans.push(Span::styled(" ".repeat(pad), fg(BG)));
    spans.push(Span::styled(count_str, inv_bold(BG, PURPLE)));
    row(f, area, 1, Line::from(spans));
    row(f, area, 2, Line::from(""));
}

// ── Option 11: Card outline (opt4) + powerline interior (opt3, no "bs") ─────
fn opt11(f: &mut ratatui::Frame, area: Rect) {
    let w = area.width as usize;

    // Top border: ╭─ booker-search ─...─╮
    let title_prefix = "╭─ booker-search ";
    let top = format!("{}{}╮", title_prefix, "─".repeat(w.saturating_sub(title_prefix.len() + 1)));
    row(f, area, 0, Line::from(Span::styled(top, fg(AQUA))));

    // Middle row: │ <powerline content> <padding> │
    let content: Vec<Span<'static>> = vec![
        Span::styled(" 42 results ", inv(FG, BG1)),
        Span::styled("▶", inv(BG1, BG2)),
        Span::styled(" INDEX ", inv_bold(YELLOW, BG2)),
        Span::styled("▶", inv(BG2, BG1)),
        Span::styled(" FILES ", inv(FG, BG1)),
        Span::styled("▶", inv(BG1, BG)),
        Span::styled("  content:off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("case:off", fg(FG4)),
        Span::styled("  ·  ", fg(BG4)),
        Span::styled("consec:off", fg(FG4)),
    ];
    let content_w: usize = content.iter().map(|s| s.content.chars().count()).sum();
    let inner_w = w.saturating_sub(2); // subtract the two │ chars
    let pad = inner_w.saturating_sub(content_w);

    let mut mid: Vec<Span<'static>> = vec![Span::styled("│", fg(AQUA))];
    mid.extend(content);
    if pad > 0 { mid.push(Span::styled(" ".repeat(pad), fg(BG))); }
    mid.push(Span::styled("│", fg(AQUA)));
    row(f, area, 1, Line::from(mid));

    // Bottom border: ╰─...─╯
    let bottom = format!("╰{}╯", "─".repeat(w.saturating_sub(2)));
    row(f, area, 2, Line::from(Span::styled(bottom, fg(AQUA))));
}
