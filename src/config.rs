// config.rs — load/save JSON config next to the .exe
// Password / S3 secret stored as base64-encoded DPAPI blobs via secret.rs

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    S3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub watch_folder: String,
    /// Legacy field kept so old JSON still deserializes; unused by sync.
    #[serde(default)]
    pub webdav_url: String,
    /// Legacy field kept so old JSON still deserializes; unused by sync.
    #[serde(default)]
    pub username: String,
    /// Legacy DPAPI blob field; unused by S3 sync.
    #[serde(default)]
    pub password_enc: String,
    #[serde(default)]
    pub remote_folder: String,
    /// Must be `"s3"`. Empty / `"webdav"` is rejected.
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
    pub credential_profile_id: Option<u64>,
    #[serde(default)]
    pub credential_version: Option<u64>,
    #[serde(default)]
    pub server_approved_at: Option<String>,
    #[serde(default = "default_true")]
    pub start_with_windows: bool,
    #[serde(default)]
    pub sync_remote_changes: bool,
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default = "default_parallel_uploads")]
    pub parallel_uploads: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch_folder: String::new(),
            webdav_url: String::new(),
            username: String::new(),
            password_enc: String::new(),
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
    match cfg.transport.trim().to_ascii_lowercase().as_str() {
        "s3" => Some(TransportKind::S3),
        _ => None,
    }
}

pub fn effective_parallel_uploads(cfg: &Config) -> usize {
    cfg.parallel_uploads.clamp(1, 2)
}

fn config_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_default();
    p.set_file_name("backupsynctool.json");
    p
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

fn default_s3_region() -> String {
    "us-east-1".to_string()
}

fn default_s3_part_size_mib() -> u64 {
    32
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_webdav_transport_is_not_s3() {
        let json = r#"{
            "watch_folder": "C:\\backups",
            "webdav_url": "https://example.com/webdav",
            "username": "u",
            "password_enc": "",
            "remote_folder": "Customer"
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.transport.is_empty());
        assert_eq!(transport_kind(&cfg), None);
        assert_eq!(cfg.s3_region, "us-east-1");
        assert!(cfg.s3_path_style);
        assert_eq!(cfg.s3_part_size_mib, 32);
        assert!(cfg.auto_update);
        assert_eq!(cfg.parallel_uploads, 2);

        let mut webdav = cfg.clone();
        webdav.transport = "webdav".into();
        assert_eq!(transport_kind(&webdav), None);
    }

    #[test]
    fn s3_transport_parses() {
        let json = r#"{
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
