# Feature Checklist

## Current Requirements

### Product

- [ ] Windows desktop tray utility
- [ ] Single portable `.exe`
- [ ] Runs without admin rights for normal use
- [ ] App name is `Backup Sync Tool`
- [ ] Config stored locally next to the executable as `backupsynctool.json`
- [ ] Config is not stored in roaming app data

### Platform / Architecture

- [ ] Implemented as a Rust app in the repo root
- [ ] Uses raw Win32 UI via `windows-rs`
- [ ] Uses blocking HTTP (no async runtime required by current docs)
- [ ] Password protection uses Windows DPAPI
- [ ] Executable embeds icons and manifest

### Tray Behavior

- [ ] App closes to tray instead of exiting
- [ ] Tray double-click reopens the window
- [ ] Tray menu supports exit
- [ ] Tray icon has at least idle and syncing states
- [ ] App keeps running in the background while UI is hidden

### Core Sync

- [ ] Monitors exactly one local folder
- [ ] Syncs local changes to a WebDAV destination
- [ ] After startup or after saving valid settings, the app checks the configured local folder and uploads existing local files to the remote destination
- [ ] Uploads new files
- [ ] Uploads changed files
- [ ] Automatically uploads newly created files in the local folder without any user action
- [ ] Uses a file watcher / continuous watch mechanism so local changes are detected automatically
- [ ] Creates remote folders as needed
- [ ] Continues background sync after setup

### Settings / Actions

- [ ] User can configure local folder
- [ ] User can configure WebDAV URL
- [ ] User can configure username
- [ ] User can configure password
- [ ] User can configure remote folder
- [ ] Manual save of settings is supported
- [ ] WebDAV connection test is supported from the UI
- [ ] `Start with Windows` is supported
- [ ] `Start with Windows` uses current-user startup registration
- [ ] `Start with Windows` is enabled by default unless the user unticks it

### Defaults / Auto-Detection

- [ ] Default local folder is `C:\XDSoftware\backups` only when no folder is saved and that path exists
- [ ] If XD software is not installed or detectable, the app leaves the local folder empty instead of prefilling it
- [ ] If `remote_folder` is empty and XD software is installed/detectable, the app may invoke `license-inspector.exe --remote-folder`
- [ ] If XD software is not installed or detection fails, the app leaves the remote folder empty instead of prefilling it
- [ ] If XD-based detection succeeds, the remote folder is prefilled automatically
- [ ] Remote folder detection derives the value from XD license data

### Update Flow

- [ ] App checks GitHub Releases on startup for updates
- [ ] Update check is silent/background
- [ ] Startup update checks do not create visible activity/log noise when no update is available
- [ ] UPDATE button is hidden on startup
- [ ] UPDATE button is shown only when a newer version is detected
- [ ] Update-related activity/log entries appear only when an update is available or the user actually starts the update
- [ ] Updater downloads the new version
- [ ] Updater replaces the executable in place
- [ ] Updater restarts the app after replacement

### Current UI Expectations

- [ ] Window/branding uses `Backup Sync Tool`
- [ ] UI sections are static and not collapsible
- [ ] Fonts follow the documented Segoe UI sizing intent
- [ ] Connect and Save are primary buttons
- [ ] Browse, Close, and Show are secondary buttons
- [ ] Password field has adjacent Show/Hide control

### Security Requirements

- [ ] Password is stored with Windows DPAPI
- [ ] Password is never stored as plain text
- [ ] Password is recoverable for authentication rather than hashed for storage
- [ ] Passwords are not logged
- [ ] Authorization headers are not logged
- [ ] TLS certificates are validated
- [ ] TLS hostnames are validated
- [ ] Non-HTTPS WebDAV endpoints are rejected in production use unless explicitly allowed for testing
- [ ] Remote delete behavior, if supported, is opt-in

## Planned Later

- [ ] Tray menu item: `Open Logs`