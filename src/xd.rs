use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_WATCH_FOLDER: &str = r"C:\XDSoftware\backups";
const HELPER_EXE: &str = "license-inspector.exe";

pub fn default_watch_folder() -> Option<String> {
    let path = Path::new(DEFAULT_WATCH_FOLDER);
    path.is_dir().then(|| path.display().to_string())
}

pub fn detect_default_remote_folder() -> Option<String> {
    let output = helper_command().output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn helper_command() -> Command {
    if let Some(path) = find_helper_exe() {
        let mut cmd = Command::new(path);
        cmd.arg("--remote-folder");
        return cmd;
    }

    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg("exit").arg("1");
    cmd
}

fn find_helper_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for dir in exe.ancestors().take(8) {
        let candidate = dir.join(HELPER_EXE);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
