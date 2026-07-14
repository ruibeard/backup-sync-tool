//! Cross-platform application support / state directories.
//!
//! Windows: `%LOCALAPPDATA%\BackupSyncTool`
//! macOS:   `~/Library/Application Support/BackupSyncTool`
//! other:   `~/.local/share/BackupSyncTool`

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "BackupSyncTool";

/// Root directory for app-local state (private engine home and config).
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

/// Private Syncthing home. Its GUI/API is loopback-only and never shared with a
/// separately installed Syncthing instance.
pub fn syncthing_home_dir() -> PathBuf {
    app_support_dir().join("syncthing")
}

/// Locate the engine shipped beside the app executable. The environment
/// override exists for development and packaging smoke tests only.
pub fn syncthing_binary_path() -> PathBuf {
    if let Some(path) =
        std::env::var_os("BACKUP_SYNC_TOOL_SYNCTHING").filter(|value| !value.is_empty())
    {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().unwrap_or_default();
    #[cfg(windows)]
    path.set_file_name("syncthing.exe");
    #[cfg(target_os = "macos")]
    {
        // Packaged: BackupSyncTool.app/Contents/MacOS/backupsynctool
        //        -> BackupSyncTool.app/Contents/Resources/syncthing
        if let Some(contents) = path.parent().and_then(Path::parent) {
            let bundled = contents.join("Resources").join("syncthing");
            if bundled.is_file() {
                return bundled;
            }
        }
        // Unpackaged development builds keep both executables together.
        path.set_file_name("syncthing");
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    path.set_file_name("syncthing");
    path
}

pub fn syncthing_license_path() -> PathBuf {
    syncthing_binary_path()
        .parent()
        .map(|parent| parent.join("syncthing-LICENSE.txt"))
        .unwrap_or_else(|| PathBuf::from("syncthing-LICENSE.txt"))
}

/// Check the release unit before normal pairing/sync startup. The updater uses
/// this failure to enter same-version bundle repair, which is required when a
/// legacy single-executable updater installs v3 without its new engine files.
pub fn validate_bundled_engine_installation() -> Result<(), String> {
    let engine = syncthing_binary_path();
    let license = syncthing_license_path();
    if !engine.is_file() {
        return Err(format!(
            "Bundled Syncthing engine is missing: {}",
            engine.display()
        ));
    }
    if !license.is_file() {
        return Err(format!(
            "Bundled Syncthing license is missing: {}",
            license.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&engine)
            .map_err(|error| format!("Could not inspect bundled Syncthing engine: {error}"))?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(format!(
                "Bundled Syncthing engine is not executable: {}",
                engine.display()
            ));
        }
    }
    Ok(())
}

/// Create `path` and parents if missing.
pub fn ensure_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}
