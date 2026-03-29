// config.rs — load/save JSON config next to the .exe
// Password is stored as a base64-encoded DPAPI blob via secret.rs

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub watch_folder: String,
    pub webdav_url: String,
    pub username: String,
    /// base64-encoded DPAPI ciphertext — never the raw password
    #[serde(default)]
    pub password_enc: String,
    pub remote_folder: String,
    #[serde(default = "default_true")]
    pub start_with_windows: bool,
    #[serde(default)]
    pub sync_remote_changes: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch_folder: String::new(),
            webdav_url: String::new(),
            username: String::new(),
            password_enc: String::new(),
            remote_folder: String::new(),
            start_with_windows: true,  // on by default
            sync_remote_changes: false,
        }
    }
}

fn config_path() -> PathBuf {
    // Store next to the .exe
    let mut p = std::env::current_exe().unwrap_or_default();
    p.set_file_name("backupsynctool.json");
    p
}

fn default_true() -> bool { true }

pub fn load() -> Config {
    let path = config_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        Config::default()
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let data = serde_json::to_string_pretty(cfg).expect("serialise config");
    std::fs::write(config_path(), data)
}
