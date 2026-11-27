use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::config::Config;

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Search,
    Config,
    Settings,
    AddLocation { editing_id: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputField {
    Name,
    Type,
    Path,
    SshHost,
    SshPort,
    SshUser,
    SshPassword,
}

impl InputField {
    pub fn index(&self) -> usize {
        match self {
            InputField::Name => 0,
            InputField::Type => 1,
            InputField::Path => 2,
            InputField::SshHost => 3,
            InputField::SshPort => 4,
            InputField::SshUser => 5,
            InputField::SshPassword => 6,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            InputField::Name => "Name",
            InputField::Type => "Type (Local/SSH)",
            InputField::Path => "Path",
            InputField::SshHost => "SSH Host",
            InputField::SshPort => "SSH Port",
            InputField::SshUser => "SSH User",
            InputField::SshPassword => "SSH Password",
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexedFile {
    pub location_id: String,
    pub path: String,
    pub display_path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub enum MatchType {
    Name,
    Content,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file: IndexedFile,
    pub match_type: MatchType,
    pub score: i64,
    pub content_line: Option<String>,
    pub content_line_number: Option<u64>,
    pub match_indices: Vec<usize>,
    /// True if the result came from the on-disk index (I), false if from a live scan/grep (S).
    pub is_indexed: bool,
}

#[derive(Debug, Clone)]
pub struct IndexProgress {
    pub total: usize,
    pub current: usize,
    pub current_path: String,
    pub location_name: String,
    pub eta_secs: Option<f64>,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatusLevel {
    Info,
    Success,
    Warning,
    Error,
}


#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SearchMode {
    Index,
    Live,
    Both,
}

impl SearchMode {
    pub fn cycle(self) -> Self {
        match self {
            SearchMode::Index => SearchMode::Live,
            SearchMode::Live => SearchMode::Both,
            SearchMode::Both => SearchMode::Index,
        }
    }
    pub fn cycle_back(self) -> Self {
        match self {
            SearchMode::Index => SearchMode::Both,
            SearchMode::Live => SearchMode::Index,
            SearchMode::Both => SearchMode::Live,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SearchMode::Index => "index",
            SearchMode::Live => "live",
            SearchMode::Both => "both",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum SearchTarget {
    Files,
    Folders,
    Both,
    /// Content-only: skip name matching, search file contents only.
    Content,
}

impl SearchTarget {
    pub fn cycle(self) -> Self {
        match self {
            SearchTarget::Files   => SearchTarget::Folders,
            SearchTarget::Folders => SearchTarget::Both,
            SearchTarget::Both    => SearchTarget::Content,
            SearchTarget::Content => SearchTarget::Files,
        }
    }
    pub fn cycle_back(self) -> Self {
        match self {
            SearchTarget::Files   => SearchTarget::Content,
            SearchTarget::Folders => SearchTarget::Files,
            SearchTarget::Both    => SearchTarget::Folders,
            SearchTarget::Content => SearchTarget::Both,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SearchTarget::Files   => "files",
            SearchTarget::Folders => "dirs",
            SearchTarget::Both    => "all",
            SearchTarget::Content => "content",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocationForm {
    pub fields: Vec<String>,
    pub focused_field: usize,
    pub is_ssh: bool,
    pub cursor_pos: usize,
    /// True after the first Enter on a sensitive path — next Enter saves anyway.
    pub path_warn_shown: bool,
}

impl Default for LocationForm {
    fn default() -> Self {
        Self {
            fields: vec![
                String::new(), // Name
                String::new(), // Type (unused, is_ssh flag used)
                String::new(), // Path
                String::new(), // SshHost
                String::from("22"), // SshPort
                String::new(), // SshUser
                String::new(), // SshPassword
            ],
            focused_field: 0,
            is_ssh: false,
            cursor_pos: 0,
            path_warn_shown: false,
        }
    }
}

impl LocationForm {
    pub fn get_field(&self, field: &InputField) -> &str {
        &self.fields[field.index()]
    }

    pub fn field_count(&self) -> usize {
        if self.is_ssh { 7 } else { 3 }
    }

    /// Display order for form fields — single source of truth used by both
    /// the renderer and the key-handler.
    pub fn fields_in_order(&self) -> Vec<InputField> {
        if self.is_ssh {
            vec![
                InputField::Name,
                InputField::Type,
                InputField::SshHost,
                InputField::SshPort,
                InputField::Path,
                InputField::SshUser,
                InputField::SshPassword,
            ]
        } else {
            vec![InputField::Name, InputField::Type, InputField::Path]
        }
    }

    pub fn current_field(&self) -> InputField {
        self.fields_in_order()
            .get(self.focused_field)
            .copied()
            .unwrap_or(InputField::Name)
    }
}

#[derive(Debug)]
pub enum IndexMsg {
    Progress { location_name: String, current: usize, total: usize, path: String },
    Done,
    Error(String),
}

#[derive(Debug)]
pub enum SearchMsg {
    ContentResult(SearchResult),
    ContentLocationProgress { loc_id: String, count: usize, current_dir: String },
    ContentLocationDone { loc_id: String },
    ContentDone,
}

#[derive(Debug)]
pub enum NameSearchMsg {
    Batch(Vec<SearchResult>),
    /// `count` is the number of entries scanned so far (0 for status-only messages like
    /// "connecting…").  `current_dir` is the directory currently being scanned, or a
    /// short status string when count==0.
    Searching { loc_id: String, count: usize, current_dir: String },
    Done { loc_id: String },
    Error(String),               // non-fatal error (e.g. SSH credentials missing)
    IndexProgress { processed: usize, total: usize },
}

// ── File Browser types ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FbEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FbPaneTarget {
    Left,
    Middle,
    Right,
}

/// A span of highlighted text: (text, (r, g, b))
pub type FbColorSpan = (String, (u8, u8, u8));

#[derive(Debug, Clone)]
pub struct SshStatInfo {
    pub mode_str: String,
    pub uid: u32,
    pub user_name: String,
    pub gid: u32,
    pub group_name: String,
    #[allow(dead_code)]
    pub size: u64,
    pub inode: u64,
    pub hard_links: u64,
    pub device_id: u64,
    pub block_size: u64,
    pub blocks: u64,
    pub accessed: i64,
    pub modified: i64,
    pub changed: i64,
}

#[derive(Debug)]
pub enum FbMsg {
    Entries { pane: FbPaneTarget, path: String, entries: Vec<FbEntry> },
    FileContent { path: String, lines: Vec<Vec<FbColorSpan>> },
    SshStat { #[allow(dead_code)] path: String, info: Result<SshStatInfo, String> },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct FbPane {
    pub path: String,
    pub entries: Vec<FbEntry>,
    pub selected: usize,
    pub scroll: usize,
    pub loading: bool,
}

impl FbPane {
    pub fn loading(path: String) -> Self {
        Self { path, entries: vec![], selected: 0, scroll: 0, loading: true }
    }
    pub fn empty(path: String) -> Self {
        Self { path, entries: vec![], selected: 0, scroll: 0, loading: false }
    }
}

#[derive(Debug)]
pub struct FileBrowser {
    pub location_id: String,
    pub is_ssh: bool,
    /// Entry name in middle pane to select when middle loads.
    pub initial_focus: Option<String>,
    pub left: FbPane,
    pub middle: FbPane,
    pub right: FbPane,
    pub tx: tokio::sync::mpsc::Sender<FbMsg>,
    /// Highlighted lines for right-pane file content preview.
    pub right_lines: Vec<Vec<FbColorSpan>>,
    pub right_content_loading: bool,
    /// Path of the content currently being loaded (may differ from right_lines_path while loading).
    pub right_content_path: String,
    /// Path whose content is actually displayed in right_lines (lags behind right_content_path while loading).
    pub right_lines_path: String,
    /// Whether keyboard focus is on the preview pane (right) rather than the middle pane.
    pub preview_focused: bool,
    /// Scroll offset for the preview pane (lines from the top).
    pub preview_scroll: usize,
    /// When true, the middle pane shows the locations list rather than a directory.
    pub at_location_root: bool,
    /// When at_location_root, maps middle.entries[i] → location_id.
    pub location_entry_ids: Vec<String>,
    /// When true, the file preview wraps long lines to the pane width.
    pub preview_wrap: bool,
    /// When true, long filenames in the middle pane wrap to the next visual row.
    pub middle_wrap: bool,
}

// ── App struct ───────────────────────────────────────────────────────────────

pub struct App {
    pub screen: Screen,
    pub query: String,
    pub cursor_pos: usize, // char index into query (0 = before first char)
    pub results: Vec<SearchResult>,
    pub result_scroll: usize,
    pub selected: usize,
    pub content_search: bool,
    pub search_target: SearchTarget,
    pub config: Config,
    pub credentials: HashMap<String, String>,
    pub master_password_unlocked: bool,
    pub master_password: Option<String>,
    pub db_progress: Option<IndexProgress>,
    pub status_msg: Option<(String, StatusLevel)>,
    pub location_saved_msg: Option<String>,
    pub location_deleting: bool,
    pub file_index: Arc<RwLock<Vec<IndexedFile>>>,
    pub form: LocationForm,
    pub show_master_password_prompt: bool,
    pub master_password_input: String,
    pub master_password_failed_attempts: u32,
    pub master_password_locked_until: Option<Instant>,
    pub config_selected: usize,
    pub delete_confirm: Option<String>,
    pub content_search_active: bool,
    pub index_rx: Option<tokio::sync::mpsc::Receiver<IndexMsg>>,
    pub search_rx: Option<tokio::sync::mpsc::Receiver<SearchMsg>>,
    pub search_cancel_tx: Option<tokio::sync::mpsc::Sender<()>>,
    pub name_search_rx: Option<tokio::sync::mpsc::Receiver<NameSearchMsg>>,
    pub name_search_cancel_txs: HashMap<String, tokio::sync::mpsc::Sender<()>>,
    pub name_search_pending_sources: u8,
    pub seen_result_keys: std::collections::HashSet<String>,
    pub show_settings_popup: bool,
    pub settings_popup_selected: usize,
    pub settings_popup_tab: usize,
    pub settings_header_selected: bool,
    pub settings_info_popup: bool,
    pub return_to_popup: bool,
    pub location_errors: Vec<(String, String)>, // (location_name, short error)
    pub name_search_active: bool,
    pub name_search_status: Option<String>,
    /// Always-visible one-line status above the search bar.
    pub search_status: String,
    /// Display filter applied on top of search results.
    /// What `search_target` was in effect when results were last fully populated.
    /// Used to decide whether a target change can be satisfied by filtering in
    /// place (None while a search is running or no search has completed yet).
    pub completed_search_target: Option<SearchTarget>,
    /// Which location IDs have finished their live/indexed search in the current session.
    /// Used by the smart ^L handler to decide filter-only vs restart.
    /// Cleared whenever a new search is started.
    pub completed_locations: std::collections::HashSet<String>,
    /// Set by the help popup to open a temp file in the editor on the next loop tick.
    pub pending_help_content: Option<String>,
    /// True when the user pressed Esc to stop an active search.
    pub search_stopped: bool,
    /// Cached per-location file counts derived from the in-memory index.
    /// Updated by `refresh_index_counts()` — never recomputed during rendering.
    pub index_location_counts: HashMap<String, usize>,
    /// Cached total entry count for the in-memory index.
    pub index_total_count: usize,
    /// Location filter: None = show all, Some(id) = show only that location's results.
    pub active_location: Option<String>,
    pub search_mode: SearchMode,
    pub status_bar_expanded: bool,
    pub index_search_progress: Option<(usize, usize)>,  // (processed, total)
    pub status_bar_visible: bool,
    pub controls_bar_visible: bool,
    pub both_mode_pending_live: bool,
    pub location_search_states: HashMap<String, (usize, String)>, // loc_id → (count, current_dir)
    /// Total file counts per location captured at content search start, used for progress bars.
    pub content_location_totals: HashMap<String, usize>,
    /// Files scanned per location during content search — written directly by the search thread
    /// via a shared Arc to avoid channel contention dropping progress updates.
    pub content_scan_counts: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    pub name_search_tx: Option<tokio::sync::mpsc::Sender<NameSearchMsg>>,
    pub pending_index_writes: Vec<IndexedFile>,
    pub show_info_popup: bool,
    pub info_popup_tab: usize,
    pub info_popup_scroll: usize,
    /// When set, the info popup shows this entry instead of the search result at `selected`.
    /// Used by the file browser's Space shortcut.
    pub fb_info_result: Option<SearchResult>,
    /// SSH stat data fetched asynchronously for the info popup. None = not yet fetched / loading.
    pub fb_info_ssh_stat: Option<Result<SshStatInfo, String>>,
    /// Keys of indexed results later confirmed by live search (for [S] badge without O(n) scan).
    pub live_confirmed_keys: std::collections::HashSet<String>,
    /// Cached count of visible_results() — updated after each results mutation.
    pub visible_count: usize,
    pub fb: Option<FileBrowser>,
    pub fb_rx: Option<tokio::sync::mpsc::Receiver<FbMsg>>,
    /// Location picker popup state (opened from the Settings tab).
    pub show_location_popup: bool,
    pub location_popup_scroll: usize,
    pub location_popup_selected: usize,
    pub show_debug_overlay: bool,
    /// True when a debounce timer is active (query changed but search hasn't started yet).
    /// Set by the main loop before each draw; prevents rendering "complete" state during typing.
    pub search_pending: bool,
    /// Fires once when the background index load (deferred from startup) completes.
    pub index_load_done_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    /// True while the "plain-text credentials" warning popup is showing in the location form.
    pub ssh_plain_warn: bool,
}

impl App {
    pub fn new(config: Config, file_index: Arc<RwLock<Vec<IndexedFile>>>) -> Self {
        let has_ssh = config.locations.iter().any(|l| l.kind == "ssh" && l.enabled);
        let require_unlock = config.settings.require_unlock_on_startup;
        let content_search = config.settings.content_search;
        // Extract cached counts before config is moved into the struct.
        let cached_location_counts = config.index_counts.clone();
        let cached_total: usize = config.index_counts.values().sum();
        let search_target = match config.settings.search_target.as_str() {
            "dirs"    => SearchTarget::Folders,
            "all"     => SearchTarget::Both,
            "content" => SearchTarget::Content,
            _         => SearchTarget::Files,
        };
        let search_mode = match config.settings.search_mode.as_str() {
            "index" => SearchMode::Index,
            "live" => SearchMode::Live,
            _ => SearchMode::Both,
        };
        let status_bar_expanded = config.settings.status_bar_expanded;
        let status_bar_visible = config.settings.status_bar_visible;
        let controls_bar_visible = config.settings.controls_bar_visible;
        Self {
            screen: Screen::Search,
            query: String::new(),
            cursor_pos: 0,
            results: Vec::new(),
            result_scroll: 0,
            selected: 0,
            content_search,
            search_target,
            config,
            credentials: HashMap::new(),
            master_password_unlocked: false,
            master_password: None,
            db_progress: None,
            status_msg: None,
            location_saved_msg: None,
            location_deleting: false,
            file_index,
            form: LocationForm::default(),
            show_master_password_prompt: has_ssh && require_unlock,
            master_password_input: String::new(),
            master_password_failed_attempts: 0,
            master_password_locked_until: None,
            config_selected: 0,
            delete_confirm: None,
            content_search_active: false,
            index_rx: None,
            search_rx: None,
            search_cancel_tx: None,
            name_search_rx: None,
            name_search_cancel_txs: HashMap::new(),
            name_search_pending_sources: 0,
            seen_result_keys: std::collections::HashSet::new(),
            show_settings_popup: false,
            settings_popup_selected: 0,
            settings_popup_tab: 0,
            settings_header_selected: false,
            settings_info_popup: false,
            return_to_popup: false,
            location_errors: Vec::new(),
            name_search_active: false,
            name_search_status: None,
            search_status: String::new(),
            completed_search_target: None,
            completed_locations: std::collections::HashSet::new(),
            pending_help_content: None,
            search_stopped: false,
            index_location_counts: cached_location_counts,
            index_total_count: cached_total,
            active_location: None,
            search_mode,
            status_bar_expanded,
            index_search_progress: None,
            status_bar_visible,
            controls_bar_visible,
            both_mode_pending_live: false,
            location_search_states: HashMap::new(),
            content_location_totals: HashMap::new(),
            content_scan_counts: Arc::new(std::sync::Mutex::new(HashMap::new())),
            name_search_tx: None,
            pending_index_writes: Vec::new(),
            show_info_popup: false,
            info_popup_tab: 0,
            info_popup_scroll: 0,
            fb_info_result: None,
            fb_info_ssh_stat: None,
            live_confirmed_keys: std::collections::HashSet::new(),
            visible_count: 0,
            fb: None,
            fb_rx: None,
            show_location_popup: false,
            location_popup_scroll: 0,
            location_popup_selected: 0,
            show_debug_overlay: false,
            search_pending: false,
            index_load_done_rx: None,
            ssh_plain_warn: false,
        }
    }

    /// Call after any mutation to `results`, `search_target`, or `active_location`.
    pub fn refresh_visible_count(&mut self) {
        self.visible_count = self.visible_results().len();
    }

    /// Recompute cached index counts from the in-memory index.
    /// Call this after any mutation to `file_index` (rebuild done, clear, new SSH files).
    pub fn refresh_index_counts(&mut self) {
        if let Ok(idx) = self.file_index.read() {
            self.index_total_count = idx.len();
            let mut counts: HashMap<String, usize> = HashMap::new();
            for f in idx.iter() {
                *counts.entry(f.location_id.clone()).or_insert(0) += 1;
            }
            self.index_location_counts = counts.clone();
            // Mirror into config so callers that follow with config.save() persist the counts.
            self.config.index_counts = counts;
        }
    }

    /// Returns a filtered view of results based on `search_target` and `active_location`.
    /// `selected` and `result_scroll` track positions within this filtered list.
    pub fn visible_results(&self) -> Vec<&SearchResult> {
        self.results.iter().filter(|r| {
            if let Some(ref loc_id) = self.active_location {
                if &r.file.location_id != loc_id {
                    return false;
                }
            }
            match self.search_target {
                SearchTarget::Files   => !r.file.is_dir,
                SearchTarget::Folders => r.file.is_dir,
                SearchTarget::Both    => true,
                SearchTarget::Content => matches!(r.match_type, MatchType::Content),
            }
        }).collect()
    }

    /// Cycle the location filter through enabled locations: None → loc[0] → loc[1] → … → None.
    pub fn cycle_active_location(&mut self) {
        let enabled: Vec<String> = self.config.locations.iter()
            .filter(|l| l.enabled)
            .map(|l| l.id.clone())
            .collect();
        if enabled.is_empty() {
            self.active_location = None;
            return;
        }
        let next = match &self.active_location {
            None => Some(enabled[0].clone()),
            Some(current) => {
                match enabled.iter().position(|id| id == current) {
                    Some(i) if i + 1 < enabled.len() => Some(enabled[i + 1].clone()),
                    _ => None,
                }
            }
        };
        self.active_location = next;
    }

    /// Name of the currently active location filter, or "all" if none.
    pub fn active_location_label(&self) -> &str {
        match &self.active_location {
            None => "all",
            Some(id) => self.config.locations.iter()
                .find(|l| &l.id == id)
                .map(|l| l.name.as_str())
                .unwrap_or("?"),
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>, level: StatusLevel) {
        self.status_msg = Some((msg.into(), level));
    }

    pub fn clear_status(&mut self) {
        self.status_msg = None;
    }

    pub fn scroll_up(&mut self, _visible_height: usize) {
        if self.selected > 0 {
            self.selected -= 1;
            // Only scroll viewport up when cursor hits the top edge
            if self.selected < self.result_scroll {
                self.result_scroll = self.selected;
            }
        }
    }

    pub fn scroll_down(&mut self, visible_height: usize, total_results: usize) {
        if total_results > 0 && self.selected < total_results - 1 {
            self.selected += 1;
            // Scroll viewport down when cursor hits the bottom edge
            if self.selected >= self.result_scroll + visible_height {
                self.result_scroll = self.selected + 1 - visible_height;
            }
        }
    }

    pub fn config_scroll_up(&mut self) {
        if self.config_selected > 0 {
            self.config_selected -= 1;
        }
    }

    pub fn config_scroll_down(&mut self) {
        let len = self.config.locations.len();
        if len > 0 && self.config_selected < len - 1 {
            self.config_selected += 1;
        }
    }

    pub fn reset_search(&mut self) {
        self.selected = 0;
        self.result_scroll = 0;
        self.refresh_visible_count();
    }

    pub fn page_down(&mut self, page_size: usize, total_results: usize) {
        if total_results > 0 {
            self.selected = (self.selected + page_size).min(total_results - 1);
            self.result_scroll = (self.result_scroll + page_size)
                .min(total_results.saturating_sub(page_size));
            // Ensure selected is visible
            if self.selected >= self.result_scroll + page_size {
                self.result_scroll = self.selected + 1 - page_size;
            }
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.selected = self.selected.saturating_sub(page_size);
        self.result_scroll = self.result_scroll.saturating_sub(page_size);
        // Ensure selected is visible
        if self.selected < self.result_scroll {
            self.result_scroll = self.selected;
        }
    }

}
