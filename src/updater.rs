// updater.rs — check GitHub releases API for a newer version, download, replace in place, restart

use serde::Deserialize;

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
    #[allow(dead_code)]
    pub version: String,
    pub url: String,
}

pub fn check(current_version: &str) -> Option<UpdateInfo> {
    let body = ureq::get(RELEASES_API)
        .set("User-Agent", "backup-sync-tool-updater")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let release: GhRelease = serde_json::from_str(&body).ok()?;

    let version = release.tag_name.trim_start_matches('v').to_string();

    if !is_newer(&version, current_version) {
        return None;
    }

    // Find the .exe asset
    let asset = release.assets.iter().find(|a| a.name.ends_with(".exe"))?;

    Some(UpdateInfo {
        version,
        url: asset.browser_download_url.clone(),
    })
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
        use std::io::Read;
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

    // Write to .tmp next to the current exe, then bat-swap-restart
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tmp = exe.with_extension("tmp");
    let bat_path = exe.with_extension("update.bat");

    std::fs::write(&tmp, &buf).map_err(|e| e.to_string())?;

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

    std::process::exit(0);
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
