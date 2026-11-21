/// Single man-page style help document shown in the Help tab.
/// Up/Down scrolls, Enter opens in the configured editor.
pub const CONTENT: &str = "\
RUMMAGE(1)                       User Manual                       RUMMAGE(1)

NAME
    rummage (rum) — fuzzy file finder with SSH support

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

KEYBOARD SHORTCUTS
──────────────────

  Navigation
    ↑ / ↓           Move cursor up / down through results
    PgUp / PgDn     Jump by one page
    Enter           Open selected file in the configured editor
    Esc             Clear query and cancel all searches

  Search Toggles
    ^F              Content search ON/OFF — grep inside file contents
    ^T              Search target: files → dirs → all → content  (cycles; filters in-place when possible)
    ^S              Search mode: index → live → both  (cycles)
    ^Z              Fuzzy/consecutive matching ON/OFF
    ^A              Case-sensitive matching ON/OFF
    ^L              Location filter: all → each location → all  (cycles)

  Wildcard Search
    Type * or ? anywhere in your query to activate glob mode.
    No toggle needed — automatically detected.

  Status Bar / Controls
    ^V              Expand / collapse the status bar
    ^H              Show / hide the status bar entirely
    ^K              Show / hide the controls bar

  Results
    ^N              File / directory info popup for selected result
    ^C / →          Open file browser at selected result

  Index
    ^R              Rebuild search index from all configured locations

  Application
    Tab             Open settings / menu popup
    ^Q              Quit

  In Settings Popup
    ← / →           Switch tab  (Settings · Locations · Keys · Manual)
    ↑ / ↓           Navigate items / scroll manual text
    Enter / Space   Toggle setting or activate item
    Tab / Esc       Close popup

  In Locations Tab
    a               Add a new location
    e / Enter       Edit the selected location
    d               Delete the selected location
    Space           Toggle location enabled / disabled
    ^Z              Test SSH connection (in edit form)

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

WILDCARD SEARCH
───────────────

  Use * and ? in your query to switch into wildcard (glob) mode.
  Wildcards are detected automatically — no mode toggle needed.

    *   matches any sequence of characters (including none)
    ?   matches exactly one character

  Pattern Matching Rules
    · If the pattern contains no '/', it matches against the filename only.
    · If the pattern contains '/', it matches against the full path.
    · Case sensitivity follows the ^A setting.

  Filename Pattern Examples
    *.rs              All Rust source files
    *.toml            All TOML config files
    test_*            Files beginning with \"test_\"
    *test*            Files containing \"test\" anywhere in the name
    *_test.rs         Rust test files ending in _test.rs
    main.??           main.rs, main.go, main.py, main.js, etc.
    *.?s              .ts, .js, .cs, .fs — two-char extensions ending in 's'
    README*           README, README.md, README.txt, README.rst, etc.
    .env*             .env, .env.local, .env.production, etc.
    *config*          Any file with \"config\" in the name

  Path Pattern Examples  (pattern contains '/')
    src/*/main.rs         main.rs one level deep under src/
    src/**/*_test.rs      any _test.rs under src/ at any depth
    */tests/*             files inside a tests/ directory
    config/*.toml         TOML files directly under any config/ dir

  Tips
    · Wildcard mode replaces fuzzy/consecutive matching entirely.
    · The [wildcard] badge appears in the search bar when active.
    · Combine with content search (^F) to grep inside matched files.
    · Use ^T to narrow results to files, dirs, or content — re-searches only when needed.

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

SEARCH RESULTS
──────────────

  The results list has four columns:

    Location   The configured location name the file belongs to.
    Match      The match type and result source:
                 [filename]  I   matched by filename, from the index
                 [filename]  S   matched by filename, from a live scan
                 [directory] I   matched directory, from the index
                 [content]   S   content (grep) match, always live
               I = indexed (fast, from the on-disk database)
               S = scanned (live search or content grep)
    Path       The file or directory path.
    Preview    Content matches show the matching line after a │.

  Tip: use ^T to narrow by type (files / dirs / content); filters in-place when possible.

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

SETTINGS GUIDE
──────────────

  Content Search  (^F)
    When ON, searches inside file contents in addition to filenames.
    Uses grep on local files and SSH grep on remote locations.
    Slower than name-only search — best used with a specific query.
    Results show the matching line as a preview next to the [content] badge.

  Search Target  (^T)
    Determines what types of entries are returned.
      files     File results only (default)
      dirs      Directory results only
      all       Both files and directories
      content   Content search only (requires Live or Both mode)

  Result Filter  (^X)
    Filters the displayed results without re-running the search.
      all       Show everything (default)
      files     File name and content matches only
      dirs      Directory matches only
      content   Content (grep) matches only

    The current filter is shown in the search bar as  filter:files  etc.
    It disappears when set back to 'all'.

  Fuzzy / Consecutive Matching  (^Z)
    When ON (consecutive), the query letters must appear in sequence with no gaps.
    Example: \"src\" matches \"source\" (s…r…c in order) but NOT \"search\".
    When OFF, standard fuzzy matching is used (letters can skip positions).
    Note: wildcard mode (* or ?) overrides both fuzzy and consecutive.

  Case Sensitive  (^A)
    When ON, \"README\" does not match \"readme\".
    When OFF, matching ignores upper/lower case.

  Editor
    Selects which editor opens when you press Enter on a result.
      nvim    Neovim (default)
      nano    GNU Nano

    Also used when opening this help document.

  Require Unlock on Startup
    When ON, SSH passwords are stored encrypted and require a master
    password to be entered each time the app starts.
    When OFF, SSH passwords are stored in a plain JSON file (600 perms).

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

LOCATIONS
─────────

  Each location is a directory that rummage indexes and searches.
  Local locations are walked on disk. SSH locations connect over SSH.
  Disable a location (Space in Locations tab) to exclude it from results
  without deleting its configuration.

  After adding or editing a location, press ^R to rebuild the index.

  SSH Locations
    rummage tries authentication in this order:
      1. SSH agent (uses existing key if loaded)
      2. Password authentication
      3. Keyboard-interactive authentication

    Use ^T in the location edit form to test the connection before saving.

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

rummage                                                          rum(1)
";

/// Return the manual content as visual rows at the given display width.
/// Lines that exceed `text_w` are word-wrapped, with continuation lines
/// indented to match the original line's leading spaces.
pub fn visual_lines(text_w: usize) -> Vec<String> {
    if text_w == 0 {
        return CONTENT.lines().map(|l| l.to_string()).collect();
    }
    CONTENT.lines().flat_map(|line| wrap_line(line, text_w)).collect()
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.chars().count() <= width {
        return vec![line.to_string()];
    }
    // Measure leading indent so continuation lines stay aligned.
    let indent_len = line.chars().take_while(|&c| c == ' ').count();
    let indent = " ".repeat(indent_len);
    let mut result = vec![];
    let mut remaining = line.to_string();
    loop {
        if remaining.chars().count() <= width {
            result.push(remaining);
            break;
        }
        // Find the last space within the allowed width.
        let first_n: String = remaining.chars().take(width).collect();
        let break_byte = first_n.rfind(' ')
            .map(|i| i + 1)        // break after the space
            .unwrap_or(first_n.len()); // hard break if no space found
        result.push(remaining[..break_byte].to_string());
        // Re-indent the remainder so continuation lines line up with the original.
        let rest = remaining[break_byte..].trim_start_matches(' ');
        remaining = format!("{}{}", indent, rest);
    }
    result
}
