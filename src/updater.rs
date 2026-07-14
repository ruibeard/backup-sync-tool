//! updater.rs — check GitHub releases, download, replace in place, restart.
//! A release is one tested unit: desktop executable, bundled Syncthing engine,
//! and the engine license. The updater always stages and swaps the whole unit.

use serde::Deserialize;
use std::io::Read;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

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
    let release = match fetch_latest_release() {
        Ok(release) => release,
        Err(error) => return CheckResult::Error(error),
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

fn fetch_latest_release() -> Result<GhRelease, String> {
    let resp = ureq::get(RELEASES_API)
        .set("User-Agent", "backup-sync-tool-updater")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| error.to_string())?;

    let body = resp.into_string().map_err(|error| error.to_string())?;
    serde_json::from_str(&body).map_err(|error| format!("Invalid release JSON: {error}"))
}

/// Repair an old-updater rollout that installed the desktop executable without
/// its bundled engine. This intentionally ignores semantic-version equality.
pub fn repair_current_bundle(progress: impl Fn(u8)) -> Result<(), String> {
    let health = crate::paths::validate_bundled_engine_installation();
    if !repair_required(&health) {
        return Ok(());
    }
    let release = fetch_latest_release()?;
    let asset = find_asset_for_platform(&release.assets).ok_or_else(|| {
        format!(
            "Release {} has no complete bundle for {}.",
            release.tag_name,
            std::env::consts::OS
        )
    })?;
    download_and_replace(&asset.browser_download_url, progress)
}

fn repair_required(installation_health: &Result<(), String>) -> bool {
    installation_health.is_err()
}

fn find_asset_for_platform(assets: &[GhAsset]) -> Option<&GhAsset> {
    #[cfg(windows)]
    {
        return assets
            .iter()
            .find(|a| a.name == "backupsynctool-windows-amd64.zip");
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
        return None;
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
    let stage = std::env::temp_dir().join(format!("backupsynctool-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let archive = stage.join("update.zip");
    std::fs::write(&archive, buf).map_err(|e| e.to_string())?;

    // Shell.Application ZIP extraction is present on Windows 7 and avoids
    // depending on Expand-Archive, which was introduced in later PowerShell.
    let app = stage.join("backupsynctool.exe");
    let engine = stage.join("syncthing.exe");
    let license = stage.join("syncthing-LICENSE.txt");
    let ps = format!(
        "$zip={zip};$dest={dest};$shell=New-Object -ComObject Shell.Application;\
         $source=$shell.NameSpace($zip);$target=$shell.NameSpace($dest);\
         if(($null -eq $source) -or ($null -eq $target)){{exit 2}};\
         $target.CopyHere($source.Items(),20);\
         for($i=0;$i -lt 300;$i++){{\
           if((Test-Path {app}) -and (Test-Path {engine}) -and (Test-Path {license})){{exit 0}};\
           Start-Sleep -Milliseconds 100\
         }};exit 3",
        zip = powershell_literal(&archive),
        dest = powershell_literal(&stage),
        app = powershell_literal(&app),
        engine = powershell_literal(&engine),
        license = powershell_literal(&license),
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .status()
        .map_err(|error| format!("Could not extract update bundle: {error}"))?;
    if !status.success() || !app.is_file() || !engine.is_file() || !license.is_file() {
        let _ = std::fs::remove_dir_all(&stage);
        return Err("The Windows update archive is incomplete or could not be extracted.".into());
    }
    let version = std::process::Command::new(&engine)
        .arg("--version")
        .output()
        .map_err(|error| format!("Could not validate the update engine: {error}"))?;
    let version_text = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    if !version.status.success()
        || !version_text.starts_with("syncthing v2.1.1")
        || !version_text.contains("noupgrade")
    {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(format!(
            "The update contains an unexpected Syncthing engine: {}",
            version_text.trim()
        ));
    }

    let install_dir = exe
        .parent()
        .ok_or_else(|| "The running executable has no install directory.".to_string())?;
    let installed_engine = install_dir.join("syncthing.exe");
    let installed_license = install_dir.join("syncthing-LICENSE.txt");
    let bat_path = stage.join("install-update.bat");
    let old_app = stage.join("backupsynctool.old.exe");
    let old_engine = stage.join("syncthing.old.exe");
    let old_license = stage.join("syncthing-LICENSE.old.txt");
    std::fs::copy(exe, &old_app)
        .map_err(|error| format!("Could not stage app rollback copy: {error}"))?;
    let had_engine = installed_engine.is_file();
    let had_license = installed_license.is_file();
    if had_engine {
        std::fs::copy(&installed_engine, &old_engine)
            .map_err(|error| format!("Could not stage engine rollback copy: {error}"))?;
    }
    if had_license {
        std::fs::copy(&installed_license, &old_license)
            .map_err(|error| format!("Could not stage license rollback copy: {error}"))?;
    }

    let bat = format!(
        "@echo off\r\n\
         ping 127.0.0.1 -n 3 >nul\r\n\
         for /L %%i in (1,1,30) do (\r\n\
           move /y \"%~dp0syncthing.exe\" \"{engine}\" >nul 2>&1 && goto engine_done\r\n\
           ping 127.0.0.1 -n 2 >nul\r\n\
         )\r\n\
         goto rollback\r\n\
         :engine_done\r\n\
         move /y \"%~dp0syncthing-LICENSE.txt\" \"{license}\" >nul 2>&1 || goto rollback\r\n\
         move /y \"%~dp0backupsynctool.exe\" \"{exe}\" >nul 2>&1 || goto rollback\r\n\
         start \"\" \"{exe}\"\r\n",
        exe = exe.display(),
        engine = installed_engine.display(),
        license = installed_license.display(),
    );
    let rollback = format!(
        ":rollback\r\n\
         move /y \"%~dp0backupsynctool.old.exe\" \"{exe}\" >nul 2>&1\r\n\
         {restore_engine}\r\n\
         {restore_license}\r\n\
         start \"\" \"{exe}\"\r\n\
         exit /b 12\r\n",
        exe = exe.display(),
        restore_engine = if had_engine {
            format!(
                "move /y \"%~dp0syncthing.old.exe\" \"{}\" >nul 2>&1",
                installed_engine.display()
            )
        } else {
            format!("del /f /q \"{}\" >nul 2>&1", installed_engine.display())
        },
        restore_license = if had_license {
            format!(
                "move /y \"%~dp0syncthing-LICENSE.old.txt\" \"{}\" >nul 2>&1",
                installed_license.display()
            )
        } else {
            format!("del /f /q \"{}\" >nul 2>&1", installed_license.display())
        },
    );
    let bat = format!("{bat}{rollback}");
    std::fs::write(&bat_path, bat).map_err(|e| e.to_string())?;

    shutdown_engine_for_update()?;
    std::process::Command::new("cmd")
        .args(["/c", &bat_path.to_string_lossy()])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn shutdown_engine_for_update() -> Result<(), String> {
    crate::syncthing::SyncthingSupervisor::shutdown_if_running()
        .map_err(|error| format!("Could not stop the bundled engine before update: {error}"))
}

#[cfg(windows)]
fn powershell_literal(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

#[cfg(target_os = "macos")]
fn replace_macos(exe: &Path, url: &str, buf: &[u8]) -> Result<(), String> {
    if !url.contains(".tar.gz") && !looks_like_gzip(buf) {
        return Err("The macOS update is not a complete app and engine archive.".into());
    }
    let staged_app = if url.contains(".tar.gz") || looks_like_gzip(buf) {
        extract_via_system_tar(buf)?
    } else {
        unreachable!()
    };

    let app_root = macos_app_root(exe)?;
    verify_macos_update_app(&staged_app)?;

    // Copy the verified bundle beside the installed app before exit. Both
    // final renames are then same-volume directory operations.
    let parent = app_root
        .parent()
        .ok_or_else(|| "The app bundle has no install directory.".to_string())?;
    let prepared = parent.join(format!(
        ".Backup Sync Tool.update-{}.app",
        std::process::id()
    ));
    let rollback = parent.join(format!(
        ".Backup Sync Tool.rollback-{}.app",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&prepared);
    let _ = std::fs::remove_dir_all(&rollback);
    let status = std::process::Command::new("ditto")
        .arg(&staged_app)
        .arg(&prepared)
        .status()
        .map_err(|error| format!("Could not stage the app update: {error}"))?;
    if !status.success() {
        return Err("Could not stage the app update beside the installed app.".into());
    }
    verify_macos_update_app(&prepared)?;

    let stage = staged_app
        .parent()
        .ok_or_else(|| "Update staging directory is unavailable.".to_string())?;
    let script_path = stage.join("install-update.sh");
    let script = format!(
        "#!/bin/bash\nset -e\nsleep 1\n\
         mv {current} {rollback}\n\
         if mv {prepared} {current}; then\n\
           open {current}\n\
           rm -rf {rollback} {stage}\n\
         else\n\
           mv {rollback} {current}\n\
           open {current}\n\
           exit 12\n\
         fi\n",
        current = bash_literal(&app_root),
        rollback = bash_literal(&rollback),
        prepared = bash_literal(&prepared),
        stage = bash_literal(stage),
    );
    std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
    set_executable(&script_path)?;

    shutdown_engine_for_update()?;
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
fn extract_via_system_tar(buf: &[u8]) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("backupsynctool-update-{}", std::process::id()));
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

    find_app_bundle(&dir)
}

#[cfg(target_os = "macos")]
fn find_app_bundle(root: &Path) -> Result<PathBuf, String> {
    let direct = root.join("Backup Sync Tool.app");
    if direct.join("Contents/MacOS/backupsynctool").is_file() {
        return Ok(direct);
    }
    Err("archive has no complete Backup Sync Tool.app bundle".into())
}

#[cfg(target_os = "macos")]
fn macos_app_root(exe: &Path) -> Result<PathBuf, String> {
    let executable_dir = exe
        .parent()
        .ok_or_else(|| "The running executable has no install directory.".to_string())?;
    if executable_dir.file_name().and_then(|name| name.to_str()) == Some("MacOS") {
        let contents = executable_dir
            .parent()
            .ok_or_else(|| "The app bundle has no Contents directory.".to_string())?;
        let app_root = contents
            .parent()
            .ok_or_else(|| "The app bundle root is unavailable.".to_string())?;
        if app_root.extension().and_then(|ext| ext.to_str()) == Some("app") {
            return Ok(app_root.to_path_buf());
        }
    }
    Err("Auto-update requires the packaged Backup Sync Tool.app.".into())
}

#[cfg(target_os = "macos")]
fn verify_macos_update_app(app: &Path) -> Result<(), String> {
    let engine = app.join("Contents/Resources/syncthing");
    let license = app.join("Contents/Resources/syncthing-LICENSE.txt");
    let desktop = app.join("Contents/MacOS/backupsynctool");
    if !desktop.is_file() || !engine.is_file() || !license.is_file() {
        return Err("The macOS update bundle is missing the app, engine, or license.".into());
    }
    let version = std::process::Command::new(&engine)
        .arg("--version")
        .output()
        .map_err(|error| format!("Could not validate the bundled engine: {error}"))?;
    let version_text = String::from_utf8_lossy(&version.stdout);
    if !version.status.success() || !version_text.starts_with("syncthing v2.1.1") {
        return Err(format!(
            "The update contains an unexpected Syncthing engine: {}",
            version_text.trim()
        ));
    }
    let signature = std::process::Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(app)
        .status()
        .map_err(|error| format!("Could not verify the update signature: {error}"))?;
    if !signature.success() {
        return Err("The macOS update app failed strict code-signature verification.".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn bash_literal(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
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
    fn repair_bypasses_version_only_when_bundle_health_fails() {
        assert!(!repair_required(&Ok(())));
        assert!(repair_required(&Err("missing engine".into())));
        assert!(repair_required(&Err("missing license".into())));
    }

    fn asset(name: &str) -> GhAsset {
        GhAsset {
            name: name.into(),
            browser_download_url: format!("https://example.invalid/{name}"),
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn mac_repair_selects_only_exact_arch_complete_bundle() {
        let arch = match std::env::consts::ARCH {
            "aarch64" => "aarch64",
            other => other,
        };
        let wanted = format!("backupsynctool-macos-{arch}.tar.gz");
        let assets = vec![
            asset("backupsynctool"),
            asset("backupsynctool-macos-wrong.tar.gz"),
            asset(&wanted),
        ];
        assert_eq!(find_asset_for_platform(&assets).unwrap().name, wanted);
        assert!(find_asset_for_platform(&assets[..2]).is_none());
    }

    #[test]
    #[cfg(windows)]
    fn windows_repair_selects_only_complete_zip() {
        let assets = vec![
            asset("backupsynctool.exe"),
            asset("backupsynctool-windows-amd64.zip"),
        ];
        assert_eq!(
            find_asset_for_platform(&assets).unwrap().name,
            "backupsynctool-windows-amd64.zip"
        );
        assert!(find_asset_for_platform(&assets[..1]).is_none());
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
