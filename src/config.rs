use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub kind: String, // "local" or "ssh"
    pub path: String,
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default = "default_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_port() -> u16 {
    22
}

fn default_true() -> bool {
    true
}

impl Location {
    pub fn display_path(&self, path: &str) -> String {
        path.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// When false, credentials are stored as plain JSON (600 perms) and no password is asked.
    /// When true, credentials are encrypted with the user's sudo password.
    #[serde(default)]
    pub require_unlock_on_startup: bool,
    #[serde(default)]
    pub consecutive_search: bool,
    #[serde(default)]
    pub case_sensitive_search: bool,
    #[serde(default)]
    pub content_search: bool,
    /// "files", "dirs", or "all"
    #[serde(default)]
    pub search_target: String,
    /// Editor to open on Enter: "nvim" or "nano"
    #[serde(default = "default_editor")]
    pub editor: String,
    /// Search mode: "index", "live", or "both"
    #[serde(default = "default_search_mode")]
    pub search_mode: String,
    /// Whether the status bar is expanded to show per-location rows
    #[serde(default)]
    pub status_bar_expanded: bool,
    #[serde(default = "default_true")]
    pub status_bar_visible: bool,
    /// Whether the controls bar (results header + search bar indicators) is shown
    #[serde(default = "default_true")]
    pub controls_bar_visible: bool,
    /// 0 = off, 1 = errors/warnings, 2 = info (search lifecycle), 3 = debug (channel stats etc.)
    #[serde(default)]
    pub debug_level: u8,
}

fn default_editor() -> String {
    "nvim".to_string()
}

fn default_search_mode() -> String {
    "both".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub locations: Vec<Location>,
    #[serde(default)]
    pub content_search_enabled: bool,
    #[serde(default)]
    pub last_db_build: Option<String>,
    #[serde(default)]
    pub credential_salt: Option<String>,
    #[serde(default)]
    pub settings: Settings,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config at {}", path.display()))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| "Failed to parse config.toml")?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write config to {}", path.display()))?;
        Ok(())
    }

    pub fn generate_location_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("loc_{:x}", ts)
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("rummage")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn credentials_path() -> PathBuf {
    config_dir().join("credentials.enc")
}

pub fn credentials_plain_path() -> PathBuf {
    config_dir().join("credentials.json")
}

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("rummage")
}

pub fn db_path() -> PathBuf {
    data_dir().join("index.db")
}
