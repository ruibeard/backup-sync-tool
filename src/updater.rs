//! updater.rs — check GitHub releases, download, replace in place, restart.
//! Windows: `.exe` asset + `.bat` swap. macOS: `backupsynctool-macos-*.tar.gz` or raw binary.

use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};

const RELEASES_API: &str = "https://api.github.com/repos/ruibeard/backup-sync-tool/releases/latest";

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String, // e.g. "v0.2.0"
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

pub enum CheckResult {
    UpdateAvailable(UpdateInfo),
    UpToDate,
    Error(String),
}

pub fn check(current_version: &str) -> CheckResult {
    let resp = match ureq::get(RELEASES_API)
        .set("User-Agent", "backup-sync-tool-updater")
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(resp) => resp,
        Err(err) => return CheckResult::Error(err.to_string()),
    };

    let body = match resp.into_string() {
        Ok(body) => body,
        Err(err) => return CheckResult::Error(err.to_string()),
    };

    let release: GhRelease = match serde_json::from_str(&body) {
        Ok(release) => release,
        Err(err) => return CheckResult::Error(format!("Invalid release JSON: {err}")),
    };

    let version = release.tag_name.trim_start_matches('v').to_string();

    if !is_newer(&version, current_version) {
        return CheckResult::UpToDate;
    }

    let asset = match find_asset_for_platform(&release.assets) {
        Some(asset) => asset,
        None => {
            return CheckResult::Error(format!(
                "Release {version} has no asset for this platform ({})",
                std::env::consts::OS
            ));
        }
    };

    CheckResult::UpdateAvailable(UpdateInfo {
        version,
        url: asset.browser_download_url.clone(),
    })
}

fn find_asset_for_platform(assets: &[GhAsset]) -> Option<&GhAsset> {
    #[cfg(windows)]
    {
        return assets.iter().find(|a| a.name.ends_with(".exe"));
    }
    #[cfg(target_os = "macos")]
    {
        let arch = std::env::consts::ARCH; // aarch64 | x86_64
        let arch_aliases: &[&str] = match arch {
            "aarch64" => &["aarch64", "arm64"],
            other => &[other],
        };
        for alias in arch_aliases {
            let want = format!("backupsynctool-macos-{alias}.tar.gz");
            if let Some(a) = assets.iter().find(|a| a.name == want) {
                return Some(a);
            }
        }
        if let Some(a) = assets.iter().find(|a| {
            a.name.starts_with("backupsynctool-macos-") && a.name.ends_with(".tar.gz")
        }) {
            return Some(a);
        }
        return assets.iter().find(|a| a.name == "backupsynctool");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = assets;
        None
    }
}

pub fn download_and_replace(url: &str, progress: impl Fn(u8)) -> Result<(), String> {
    let resp = ureq::get(url)
        .set("User-Agent", "backup-sync-tool-updater")
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|e| e.to_string())?;

    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    let mut downloaded: u64 = 0;
    let mut chunk = [0u8; 65536];

    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        downloaded += n as u64;
        if total > 0 {
            progress(((downloaded * 100) / total) as u8);
        }
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    #[cfg(windows)]
    {
        replace_windows(&exe, &buf)?;
    }
    #[cfg(target_os = "macos")]
    {
        replace_macos(&exe, url, &buf)?;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (exe, buf, url);
        return Err("auto-update unsupported on this OS".into());
    }

    #[allow(unreachable_code)]
    {
        std::process::exit(0);
    }
}

#[cfg(windows)]
fn replace_windows(exe: &Path, buf: &[u8]) -> Result<(), String> {
    let tmp = exe.with_extension("tmp");
    let bat_path = exe.with_extension("update.bat");

    std::fs::write(&tmp, buf).map_err(|e| e.to_string())?;

    let bat = format!(
        "@echo off\r\ntimeout /t 2 /nobreak >nul\r\nmove /y \"{tmp}\" \"{exe}\"\r\nstart \"\" \"{exe}\"\r\ndel \"%~f0\"\r\n",
        tmp = tmp.display(),
        exe = exe.display(),
    );
    std::fs::write(&bat_path, bat).map_err(|e| e.to_string())?;

    std::process::Command::new("cmd")
        .args(["/c", &bat_path.to_string_lossy()])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn replace_macos(exe: &Path, url: &str, buf: &[u8]) -> Result<(), String> {
    let new_bin = if url.contains(".tar.gz") || looks_like_gzip(buf) {
        extract_via_system_tar(buf)?
    } else {
        buf.to_vec()
    };

    let tmp = exe.with_extension("tmp");
    std::fs::write(&tmp, &new_bin).map_err(|e| e.to_string())?;
    set_executable(&tmp)?;

    let script = format!(
        "#!/bin/bash\nsleep 1\nmv -f \"{tmp}\" \"{exe}\"\nchmod +x \"{exe}\"\nrm -f \"{script}\"\nexec \"{exe}\"\n",
        tmp = tmp.display(),
        exe = exe.display(),
        script = exe.with_extension("update.sh").display(),
    );
    let script_path = exe.with_extension("update.sh");
    std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
    set_executable(&script_path)?;

    std::process::Command::new("bash")
        .arg(&script_path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn looks_like_gzip(buf: &[u8]) -> bool {
    buf.len() >= 2 && buf[0] == 0x1f && buf[1] == 0x8b
}

#[cfg(target_os = "macos")]
fn extract_via_system_tar(buf: &[u8]) -> Result<Vec<u8>, String> {
    let dir = std::env::temp_dir().join(format!(
        "backupsynctool-update-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tgz = dir.join("update.tar.gz");
    std::fs::write(&tgz, buf).map_err(|e| e.to_string())?;

    let status = std::process::Command::new("tar")
        .args(["-xzf", &tgz.to_string_lossy(), "-C", &dir.to_string_lossy()])
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("tar extract failed".into());
    }

    let found = find_file_named(&dir, "backupsynctool")?;
    let bytes = std::fs::read(&found).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn find_file_named(root: &Path, name: &str) -> Result<PathBuf, String> {
    fn walk(dir: &Path, name: &str) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path, name) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|s| s.to_str()) == Some(name) {
                return Some(path);
            }
        }
        None
    }
    walk(root, name).ok_or_else(|| format!("archive has no {name} binary"))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| e.to_string())
}

fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> (u32, u32, u32) {
        let mut parts = v.trim_start_matches('v').splitn(3, '.');
        let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    }
    parse(candidate) > parse(current)
}

/// Background-friendly check; returns a short status string for logs/UI.
pub fn check_status_line(current_version: &str) -> String {
    match check(current_version) {
        CheckResult::UpToDate => "updater: up to date".into(),
        CheckResult::UpdateAvailable(info) => {
            format!(
                "updater: v{} available — choose [6] to install",
                info.version
            )
        }
        CheckResult::Error(e) => format!("updater: check failed ({e})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_year_semver() {
        assert!(is_newer("2026.0.4", "2026.0.3"));
        assert!(!is_newer("2026.0.3", "2026.0.4"));
        assert!(!is_newer("2026.0.4", "2026.0.4"));
        assert!(is_newer("2026.1.0", "2026.0.99"));
        assert!(is_newer("v2026.0.4", "2026.0.3"));
    }

    #[test]
    fn is_newer_old_scheme_to_year_scheme() {
        assert!(is_newer("2026.0.1", "0.3.0"));
        assert!(!is_newer("0.9.0", "2026.0.1"));
    }

    #[test]
    #[ignore = "hits GitHub API"]
    fn check_detects_update_when_behind() {
        match check("2026.0.0") {
            CheckResult::UpdateAvailable(info) => {
                assert!(!info.url.is_empty());
                assert!(is_newer(&info.version, "2026.0.0"));
            }
            CheckResult::UpToDate => panic!("expected update from 2026.0.0"),
            CheckResult::Error(e) => panic!("GitHub API error: {e}"),
        }
    }

    #[test]
    #[ignore = "hits GitHub API"]
    fn check_up_to_date_at_current_version() {
        match check(env!("CARGO_PKG_VERSION")) {
            CheckResult::UpToDate => {}
            CheckResult::UpdateAvailable(info) => {
                panic!(
                    "unexpected update v{} while running v{}",
                    info.version,
                    env!("CARGO_PKG_VERSION")
                )
            }
            CheckResult::Error(e) => panic!("GitHub API error: {e}"),
        }
    }
}
