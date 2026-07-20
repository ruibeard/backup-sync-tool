//! Local sync cursor + per-file tip metadata (JSON under app support).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncState {
    #[serde(default)]
    pub cursor: u64,
    /// Relative path → live tip known to this device.
    #[serde(default)]
    pub files: HashMap<String, FileTip>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTip {
    pub file_id: String,
    pub revision: u64,
    pub size: u64,
    #[serde(default)]
    pub content_sha256: String,
}

impl SyncState {
    pub fn load(path: &Path) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("sync state dir: {e}"))?;
        }
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, raw).map_err(|e| format!("sync state write: {e}"))?;
        fs::rename(&tmp, path).map_err(|e| format!("sync state rename: {e}"))?;
        Ok(())
    }

    pub fn path_for_file_id(&self, file_id: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(_, tip)| tip.file_id == file_id)
            .map(|(path, _)| path.as_str())
    }

    pub fn upsert_tip(&mut self, path: &str, tip: FileTip) {
        // Drop any stale path for the same file_id (rename).
        let stale: Vec<String> = self
            .files
            .iter()
            .filter(|(p, t)| t.file_id == tip.file_id && p.as_str() != path)
            .map(|(p, _)| p.clone())
            .collect();
        for p in stale {
            self.files.remove(&p);
        }
        self.files.insert(path.to_string(), tip);
    }

    pub fn remove_path(&mut self, path: &str) {
        self.files.remove(path);
    }
}

pub fn state_path_for_destination(destination_uuid: &str) -> PathBuf {
    crate::paths::app_support_dir()
        .join("sync")
        .join(format!("{destination_uuid}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rename_replaces_old_path_for_same_file_id() {
        let mut state = SyncState::default();
        state.upsert_tip(
            "old.txt",
            FileTip {
                file_id: "f1".into(),
                revision: 1,
                size: 1,
                content_sha256: "aa".into(),
            },
        );
        state.upsert_tip(
            "new.txt",
            FileTip {
                file_id: "f1".into(),
                revision: 2,
                size: 1,
                content_sha256: "aa".into(),
            },
        );
        assert!(!state.files.contains_key("old.txt"));
        assert_eq!(state.files["new.txt"].revision, 2);
        assert_eq!(state.path_for_file_id("f1"), Some("new.txt"));
    }

    #[test]
    fn save_load_roundtrip() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("bst-sync-state-{nanos}"));
        let path = dir.join("state.json");
        let mut state = SyncState {
            cursor: 9,
            files: HashMap::new(),
        };
        state.upsert_tip(
            "a/b.txt",
            FileTip {
                file_id: "uuid".into(),
                revision: 3,
                size: 10,
                content_sha256: "abcd".into(),
            },
        );
        state.save(&path).unwrap();
        let loaded = SyncState::load(&path);
        assert_eq!(loaded.cursor, 9);
        assert_eq!(loaded.files["a/b.txt"].file_id, "uuid");
        let _ = fs::remove_dir_all(dir);
    }
}
