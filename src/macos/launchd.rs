//! macOS LaunchAgent helpers (`start_with_windows` → start at login).

use crate::config::Config;
use crate::logs;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "cam.rui.backupsynctool";

pub fn plist_path() -> PathBuf {
    dirs_launch_agents().join(format!("{LABEL}.plist"))
}

fn dirs_launch_agents() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/LaunchAgents")
}

fn installed_binary() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("backupsynctool"))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Write LaunchAgent plist (RunAtLoad). Does not start daemon now — avoids
/// fighting the interactive instance lock. Agent starts on next login.
pub fn install() -> Result<(), String> {
    let bin = installed_binary();
    if !bin.is_file() {
        return Err(format!("binary not found: {}", bin.display()));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let agents = dirs_launch_agents();
    fs::create_dir_all(&agents).map_err(|e| e.to_string())?;
    let plist = plist_path();
    let _ = unload();
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>--daemon</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>{home}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <false/>
  <key>StandardOutPath</key>
  <string>/tmp/backupsynctool.out.log</string>
  <key>StandardErrorPath</key>
  <string>/tmp/backupsynctool.err.log</string>
</dict>
</plist>
"#,
        bin = xml_escape(&bin.display().to_string()),
        home = xml_escape(&home),
    );
    fs::write(&plist, body).map_err(|e| e.to_string())?;
    logs::append(&format!(
        "LaunchAgent installed (starts on login): {}",
        plist.display()
    ));
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let _ = unload();
    let plist = plist_path();
    if plist.is_file() {
        fs::remove_file(&plist).map_err(|e| e.to_string())?;
    }
    logs::append("LaunchAgent removed");
    Ok(())
}

fn unload() -> Result<(), String> {
    let plist = plist_path();
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".into());
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{LABEL}")])
        .status();
    if plist.is_file() {
        let _ = Command::new("launchctl")
            .args(["unload", &plist.to_string_lossy()])
            .status();
    }
    Ok(())
}

/// Apply `config.start_with_windows` meaning on macOS: start at login.
pub fn sync_from_config(cfg: &Config) -> Result<(), String> {
    if cfg.start_with_windows {
        install()
    } else {
        uninstall()
    }
}
