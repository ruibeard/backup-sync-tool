//! Persistent desktop configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static CONFIG_SAVE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub watch_folder: String,
    #[serde(default = "default_pair_api_base")]
    pub pair_api_base: String,
    /// Protected control-plane bearer token (DPAPI on Windows, Keychain on macOS).
    #[serde(default)]
    pub device_token_enc: String,
    #[serde(default)]
    pub device_uuid: String,
    #[serde(default)]
    pub destination_uuid: String,
    #[serde(default)]
    pub destination_label: String,
    #[serde(default)]
    pub server_approved_at: Option<String>,
    #[serde(default = "default_true")]
    pub start_with_windows: bool,
    #[serde(default = "default_true")]
    pub auto_update: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch_folder: String::new(),
            pair_api_base: default_pair_api_base(),
            device_token_enc: String::new(),
            device_uuid: String::new(),
            destination_uuid: String::new(),
            destination_label: String::new(),
            server_approved_at: None,
            start_with_windows: true,
            auto_update: true,
        }
    }
}

pub fn is_paired(cfg: &Config) -> bool {
    !cfg.device_token_enc.trim().is_empty()
        && !cfg.device_uuid.trim().is_empty()
        && !cfg.destination_uuid.trim().is_empty()
}

fn config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let support = crate::paths::app_support_dir().join("backupsynctool.json");
        if support.is_file() {
            return support;
        }
        let mut beside = std::env::current_exe().unwrap_or_default();
        beside.set_file_name("backupsynctool.json");
        if beside.is_file() {
            if let Some(parent) = support.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&beside, &support).is_ok() {
                return support;
            }
            return beside;
        }
        support
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut path = std::env::current_exe().unwrap_or_default();
        path.set_file_name("backupsynctool.json");
        path
    }
}

fn default_true() -> bool {
    true
}

fn default_pair_api_base() -> String {
    "https://backup.rui.cam".to_string()
}

/// Normalize Laravel control-plane base URL (no `/api` suffix).
pub fn normalize_pair_api_base(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Control plane URL is required.".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("Control plane URL must start with http:// or https://.".into());
    }
    let without_trail = trimmed.trim_end_matches('/');
    let path_start = without_trail.find("://").and_then(|index| {
        without_trail[index + 3..]
            .find('/')
            .map(|next| index + 3 + next)
    });
    if let Some(index) = path_start {
        let path = &without_trail[index..];
        if path.eq_ignore_ascii_case("/api") || path.to_ascii_lowercase().starts_with("/api/") {
            return Err("Use the site root (e.g. https://backup.example.com), not /api.".into());
        }
    }
    Ok(without_trail.to_string())
}

pub fn load() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .map(|data| from_json(&data))
        .unwrap_or_default()
}

fn from_json(data: &str) -> Config {
    let Ok(mut cfg) = serde_json::from_str::<Config>(data) else {
        return Config::default();
    };
    cfg.pair_api_base =
        normalize_pair_api_base(&cfg.pair_api_base).unwrap_or_else(|_| default_pair_api_base());
    cfg
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let _guard = CONFIG_SAVE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let data = serde_json::to_string_pretty(cfg).expect("serialise config");
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, data)?;
    if let Err(error) = replace_file(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(temporary: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from = HSTRING::from(temporary.as_os_str());
    let to = HSTRING::from(destination.as_os_str());
    unsafe {
        MoveFileExW(
            &from,
            &to,
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| std::io::Error::last_os_error())
    }
}

/// Protect and install a complete pairing assignment in one config write.
pub fn save_pairing_candidate(mut candidate: Config, device_token: &str) -> Result<Config, String> {
    let staged =
        crate::secret::CandidateDeviceToken::stage(device_token, &candidate.device_token_enc)?;
    candidate.device_token_enc = staged.protected().to_string();
    save(&candidate).map_err(|error| format!("Pairing succeeded but save failed: {error}"))?;
    let _ = staged.commit();
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_assignment_is_paired() {
        let cfg = Config {
            device_token_enc: "protected".into(),
            device_uuid: "desktop-1".into(),
            destination_uuid: "dest-1".into(),
            ..Config::default()
        };
        assert!(is_paired(&cfg));
    }

    #[test]
    fn config_without_token_is_not_paired() {
        let cfg = from_json(
            r#"{"watch_folder": "/backups", "pair_api_base": "https://control.example/"}"#,
        );
        assert!(!is_paired(&cfg));
        assert_eq!(cfg.pair_api_base, "https://control.example");
        assert_eq!(cfg.watch_folder, "/backups");
    }

    #[test]
    fn normalize_pair_api_base_strips_slash_and_rejects_api_path() {
        assert_eq!(
            normalize_pair_api_base(" https://backup.example.com/ ").unwrap(),
            "https://backup.example.com"
        );
        assert!(normalize_pair_api_base("").is_err());
        assert!(normalize_pair_api_base("backup.example.com").is_err());
        assert!(normalize_pair_api_base("https://backup.example.com/api").is_err());
    }
}
