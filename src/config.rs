// config.rs — load/save JSON config next to the .exe
// S3 secret / device token stored as base64-encoded DPAPI blobs via secret.rs

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static CONFIG_SAVE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    S3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub schema_version: u32,
    pub watch_folder: String,
    #[serde(default)]
    pub remote_folder: String,
    /// Must be `"s3"`. Anything else → re-pair.
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub s3_endpoint: String,
    #[serde(default = "default_s3_region")]
    pub s3_region: String,
    #[serde(default)]
    pub s3_bucket: String,
    #[serde(default)]
    pub s3_access_key: String,
    /// base64-encoded DPAPI ciphertext for the S3 secret key
    #[serde(default)]
    pub s3_secret_enc: String,
    #[serde(default = "default_true")]
    pub s3_path_style: bool,
    #[serde(default)]
    pub s3_prefix: String,
    #[serde(default = "default_s3_part_size_mib")]
    pub s3_part_size_mib: u64,
    #[serde(default = "default_pair_api_base")]
    pub pair_api_base: String,
    #[serde(default)]
    pub device_token_enc: String,
    #[serde(default)]
    pub device_uuid: String,
    #[serde(default)]
    pub credential_profile_id: Option<u64>,
    #[serde(default)]
    pub credential_version: Option<u64>,
    #[serde(default)]
    pub server_approved_at: Option<String>,
    #[serde(default = "default_true")]
    pub start_with_windows: bool,
    #[serde(skip)]
    pub sync_remote_changes: bool,
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default = "default_parallel_uploads")]
    pub parallel_uploads: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 2,
            watch_folder: String::new(),
            remote_folder: String::new(),
            transport: String::new(),
            s3_endpoint: String::new(),
            s3_region: default_s3_region(),
            s3_bucket: String::new(),
            s3_access_key: String::new(),
            s3_secret_enc: String::new(),
            s3_path_style: true,
            s3_prefix: String::new(),
            s3_part_size_mib: default_s3_part_size_mib(),
            pair_api_base: default_pair_api_base(),
            device_token_enc: String::new(),
            device_uuid: String::new(),
            credential_profile_id: None,
            credential_version: None,
            server_approved_at: None,
            start_with_windows: true,
            sync_remote_changes: false,
            auto_update: true,
            parallel_uploads: default_parallel_uploads(),
        }
    }
}

pub fn transport_kind(cfg: &Config) -> Option<TransportKind> {
    if cfg.schema_version != 2 {
        return None;
    }

    match cfg.transport.trim().to_ascii_lowercase().as_str() {
        "s3" => Some(TransportKind::S3),
        _ => None,
    }
}

pub fn effective_parallel_uploads(cfg: &Config) -> usize {
    cfg.parallel_uploads.clamp(1, 2)
}

fn config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        // Stable path so install to ~/.local/bin does not drop pairing.
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
        let mut p = std::env::current_exe().unwrap_or_default();
        p.set_file_name("backupsynctool.json");
        p
    }
}

fn default_true() -> bool {
    true
}

fn default_parallel_uploads() -> usize {
    2
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
    let path_start = without_trail
        .find("://")
        .and_then(|i| without_trail[i + 3..].find('/').map(|j| i + 3 + j));
    if let Some(idx) = path_start {
        let path = &without_trail[idx..];
        if path.eq_ignore_ascii_case("/api") || path.to_ascii_lowercase().starts_with("/api/") {
            return Err("Use the site root (e.g. https://backup.example.com), not /api.".into());
        }
    }
    Ok(without_trail.to_string())
}

fn default_s3_region() -> String {
    "garage".to_string()
}

fn default_s3_part_size_mib() -> u64 {
    32
}

pub fn load() -> Config {
    let path = config_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        let mut parsed: Config = serde_json::from_str(&data).unwrap_or_default();
        if parsed.schema_version == 2 {
            if parsed.pair_api_base.trim().is_empty() {
                parsed.pair_api_base = default_pair_api_base();
            } else if let Ok(normalized) = normalize_pair_api_base(&parsed.pair_api_base) {
                parsed.pair_api_base = normalized;
            }
            parsed
        } else {
            Config::default()
        }
    } else {
        Config::default()
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let _guard = CONFIG_SAVE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let data = serde_json::to_string_pretty(cfg).expect("serialise config");
    let path = config_path();
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, data)?;
    if let Err(err) = replace_file(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(err);
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

/// Protect candidate credentials and install the complete v2 config as one
/// local transaction. Existing macOS Keychain handles remain active until the
/// config rename succeeds; a failed save removes only candidate handles.
pub fn save_pairing_candidate(
    mut candidate: Config,
    device_token: &str,
    s3_secret: &str,
) -> Result<Config, String> {
    let staged = crate::secret::CandidateSecrets::stage(
        device_token,
        s3_secret,
        &candidate.device_token_enc,
        &candidate.s3_secret_enc,
    )?;
    candidate.device_token_enc = staged.protected().device_token_enc.clone();
    candidate.s3_secret_enc = staged.protected().s3_secret_enc.clone();
    candidate.schema_version = 2;
    save(&candidate).map_err(|err| format!("Pairing succeeded but save failed: {err}"))?;
    let _ = staged.commit();
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_json_fields_are_ignored() {
        let json = r#"{
            "watch_folder": "C:\\backups",
            "webdav_url": "https://example.com/webdav",
            "username": "u",
            "password_enc": "",
            "remote_folder": "Customer"
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.schema_version, 0);
        assert!(cfg.transport.is_empty());
        assert_eq!(transport_kind(&cfg), None);
        assert_eq!(cfg.s3_region, "garage");
        assert!(cfg.s3_path_style);
        assert_eq!(cfg.s3_part_size_mib, 32);
        assert!(cfg.auto_update);
        assert_eq!(cfg.parallel_uploads, 2);
    }

    #[test]
    fn non_s3_transport_is_rejected() {
        let mut cfg = Config::default();
        cfg.transport = "webdav".into();
        assert_eq!(transport_kind(&cfg), None);
        cfg.transport = "future".into();
        assert_eq!(transport_kind(&cfg), None);
    }

    #[test]
    fn s3_transport_parses() {
        let json = r#"{
            "schema_version": 2,
            "watch_folder": "C:\\backups",
            "transport": "s3",
            "remote_folder": "Customer",
            "s3_endpoint": "https://s3.rui.cam",
            "s3_bucket": "device-bucket",
            "s3_access_key": "AKIA",
            "s3_secret_enc": "enc",
            "s3_prefix": "",
            "s3_path_style": true,
            "s3_part_size_mib": 32
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(transport_kind(&cfg), Some(TransportKind::S3));
        assert_eq!(cfg.s3_endpoint, "https://s3.rui.cam");
        assert!(cfg.s3_prefix.is_empty());
        assert_eq!(effective_parallel_uploads(&cfg), 2);
    }

    #[test]
    fn s3_parallel_uploads_are_capped_at_two() {
        let mut cfg = Config {
            transport: "s3".into(),
            parallel_uploads: 20,
            ..Config::default()
        };
        assert_eq!(effective_parallel_uploads(&cfg), 2);

        cfg.parallel_uploads = 1;
        assert_eq!(effective_parallel_uploads(&cfg), 1);

        cfg.parallel_uploads = 0;
        assert_eq!(effective_parallel_uploads(&cfg), 1);
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
        assert!(normalize_pair_api_base("https://backup.example.com/api/pair").is_err());
    }

    #[test]
    fn s3_build_allows_empty_prefix() {
        let cfg = Config {
            transport: "s3".into(),
            remote_folder: "Customer".into(),
            s3_endpoint: "https://s3.rui.cam".into(),
            s3_bucket: "device-1".into(),
            s3_access_key: "AKIA".into(),
            s3_prefix: String::new(),
            ..Config::default()
        };
        assert!(crate::transport::build(&cfg, "secret").is_ok());
    }
}
