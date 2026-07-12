//! Cross-platform application support / state directories.
//!
//! Windows: `%LOCALAPPDATA%\BackupSyncTool`
//! macOS:   `~/Library/Application Support/BackupSyncTool`
//! other:   `~/.local/share/BackupSyncTool`

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "BackupSyncTool";

/// Root directory for app-local state (manifest, multipart resume, etc.).
pub fn app_support_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(base) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            return PathBuf::from(base).join(APP_DIR_NAME);
        }
        return std::env::temp_dir().join(APP_DIR_NAME);
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_DIR_NAME);
        }
        return std::env::temp_dir().join(APP_DIR_NAME);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(xdg).join(APP_DIR_NAME);
        }
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(APP_DIR_NAME);
        }
        return std::env::temp_dir().join(APP_DIR_NAME);
    }

    #[cfg(not(any(windows, unix)))]
    {
        std::env::temp_dir().join(APP_DIR_NAME)
    }
}

/// Local v2 upload-manifest directory (`state-v2`).
pub fn manifest_state_dir() -> PathBuf {
    app_support_dir().join("state-v2")
}

/// Persistent S3 multipart resume directory (`multipart-v1`).
pub fn multipart_state_dir() -> PathBuf {
    app_support_dir().join("multipart-v1")
}

/// Create `path` and parents if missing.
pub fn ensure_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}
