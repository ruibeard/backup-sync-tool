//! What this device last synced, per file (JSON under app support).
//!
//! An entry says: at the last sync the bucket held `etag` and the local file
//! had `size` and `mtime_ns`. Comparing both sides with it tells which side
//! changed since. No entry means the device has not seen the file yet.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncState {
    #[serde(default)]
    pub files: HashMap<String, Synced>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Synced {
    pub etag: String,
    pub size: u64,
    /// Local mtime in nanoseconds (UNIX).
    pub mtime_ns: u64,
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
}

pub fn state_path(device_uuid: &str) -> PathBuf {
    crate::paths::app_support_dir()
        .join("sync")
        .join(format!("{device_uuid}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_load_roundtrip() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("bst-sync-state-{nanos}"));
        let path = dir.join("state.json");
        let mut state = SyncState::default();
        let synced = Synced {
            etag: "abcd".into(),
            size: 10,
            mtime_ns: 42,
        };
        state.files.insert("a/b.txt".into(), synced.clone());
        state.save(&path).unwrap();
        assert_eq!(SyncState::load(&path).files["a/b.txt"], synced);
        let _ = fs::remove_dir_all(dir);
    }
}
