use ratatui::style::{Color, Modifier, Style};

// ── Gruvbox Dark palette ──────────────────────────────────────────────────────
pub const BG:       Color = Color::Rgb(40,  40,  40);   // #282828
pub const BG1:      Color = Color::Rgb(60,  56,  54);   // #3c3836
#[allow(dead_code)]
pub const BG2:      Color = Color::Rgb(80,  73,  69);   // #504945
pub const BG4:      Color = Color::Rgb(124, 111, 100);  // #7c6f64
pub const FG:       Color = Color::Rgb(235, 219, 178);  // #ebdbb2
pub const FG4:      Color = Color::Rgb(146, 131, 116);  // #928374
pub const RED:      Color = Color::Rgb(251, 73,  52);   // #fb4934
pub const GREEN:    Color = Color::Rgb(184, 187, 38);   // #b8bb26
pub const YELLOW:   Color = Color::Rgb(250, 189, 47);   // #fabd2f
pub const BLUE:     Color = Color::Rgb(131, 165, 152);  // #83a598
pub const PURPLE:   Color = Color::Rgb(211, 134, 155);  // #d3869b
pub const AQUA:     Color = Color::Rgb(142, 192, 124);  // #8ec07c
pub const ORANGE:   Color = Color::Rgb(254, 128, 25);   // #fe8019
pub const NEUTRAL_GREEN: Color = Color::Rgb(152, 151, 26);   // #98971a — muted green for hidden dirs

pub struct Theme;

impl Theme {
    // ── Core semantic styles (unchanged API) ─────────────────────────────────
    pub fn primary()        -> Style { Style::default().fg(AQUA) }
    pub fn success()        -> Style { Style::default().fg(GREEN) }
    pub fn warning()        -> Style { Style::default().fg(YELLOW) }
    pub fn error()          -> Style { Style::default().fg(RED) }
    pub fn selected()       -> Style { Style::default().fg(YELLOW).add_modifier(Modifier::BOLD) }
    pub fn name_badge()     -> Style { Style::default().fg(BG).bg(BLUE) }
    pub fn content_badge()  -> Style { Style::default().fg(BG).bg(PURPLE) }
    pub fn title()          -> Style { Style::default().fg(AQUA).add_modifier(Modifier::BOLD) }
    pub fn normal()         -> Style { Style::default().fg(FG) }
    pub fn dim()            -> Style { Style::default().fg(FG4) }
    pub fn header()         -> Style { Style::default().fg(FG).add_modifier(Modifier::BOLD) }
    pub fn input_active()   -> Style { Style::default().fg(FG) }
    pub fn enabled()        -> Style { Style::default().fg(GREEN) }
    pub fn disabled()       -> Style { Style::default().fg(RED) }
    pub fn status_border()  -> Style { Style::default().fg(ORANGE) }
    pub fn progress_border()-> Style { Style::default().fg(PURPLE) }

    // ── New helpers ───────────────────────────────────────────────────────────
    /// [ON] toggle value
    pub fn on()  -> Style { Style::default().fg(GREEN).add_modifier(Modifier::BOLD) }
    /// [OFF] toggle value
    pub fn off() -> Style { Style::default().fg(FG4) }

    /// Match-highlight (bold yellow) — results and content preview
    pub fn highlight() -> Style { Style::default().fg(YELLOW).add_modifier(Modifier::BOLD) }

    /// Match-highlight on a selected row — inverted so it shows through the yellow selection colour
    pub fn highlight_selected() -> Style { Style::default().fg(BG).bg(YELLOW).add_modifier(Modifier::BOLD) }

    /// Cursor block █
    pub fn cursor() -> Style { Style::default().fg(ORANGE) }

    /// Active tab: inverse
    pub fn tab_active()   -> Style { Style::default().fg(BG).bg(AQUA).add_modifier(Modifier::BOLD) }
    /// Inactive tab: dim
    pub fn tab_inactive() -> Style { Style::default().fg(FG4) }

    /// Scrollbar thumb █ and track │
    pub fn scrollbar_thumb() -> Style { Style::default().fg(BG4) }
    pub fn scrollbar_track() -> Style { Style::default().fg(BG1) }

    /// Per-location status row styles
    pub fn status_error()     -> Style { Style::default().fg(RED) }
    pub fn status_complete()  -> Style { Style::default().fg(GREEN) }
    pub fn status_searching() -> Style { Style::default().fg(YELLOW) }

    /// File browser overlay outer border (orange = distinctive overlay accent)
    pub fn fb_border()      -> Style { Style::default().fg(ORANGE) }
    /// File browser active pane border
    pub fn pane_active()    -> Style { Style::default().fg(AQUA) }
    /// File browser inactive pane border
    pub fn pane_inactive()  -> Style { Style::default().fg(BG4) }

    /// Directory badge in results list
    pub fn dir_badge()      -> Style { Style::default().fg(BG).bg(YELLOW) }
    /// Directory entry in file browser (matches nvim GruvboxGreen / Directory)
    pub fn dir()            -> Style { Style::default().fg(GREEN) }
    /// Hidden directory entry (muted green)
    pub fn dir_hidden()     -> Style { Style::default().fg(NEUTRAL_GREEN) }

    /// Cycle-value indicator (search target, mode, filter, editor)
    #[allow(dead_code)]
    pub fn indicator() -> Style { Style::default().fg(AQUA).add_modifier(Modifier::BOLD) }

    /// Location column in results (unselected)
    pub fn loc_col() -> Style { Style::default().fg(FG4) }

    /// Section header in info popup (bold fg)
    pub fn section_header() -> Style { Style::default().fg(FG).add_modifier(Modifier::BOLD) }
    /// Subsection header in manual
    pub fn subsection_header() -> Style { Style::default().fg(AQUA) }
}
