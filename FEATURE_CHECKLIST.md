# Feature Checklist

## Current Requirements

### Product

- [x] Windows desktop tray utility
- [x] Single portable `.exe`
- [x] Runs without admin rights for normal use
- [x] App name is `Backup Sync Tool`
- [x] Config stored locally next to the executable as `backupsynctool.json`
- [x] Config is not stored in roaming app data

### Platform / Architecture

- [x] Implemented as a Rust app in the repo root
- [x] Uses raw Win32 UI via `windows-rs`
- [x] Uses blocking HTTP (no async runtime required by current docs)
- [x] Password protection uses Windows DPAPI
- [x] Executable embeds icons and manifest

### Tray Behavior

- [x] App closes to tray instead of exiting
- [x] Tray double-click reopens the window
- [x] Tray menu supports exit
- [x] Tray icon has idle, syncing, and complete states
- [x] Animated tray icon cycles through 6 syncing frames without resetting between sync activity events
- [x] App keeps running in the background while UI is hidden

### Core Sync

- [x] Monitors exactly one local folder
- [x] Syncs local changes to a WebDAV destination
- [x] After startup or after saving valid settings, the app checks the configured local folder and uploads existing local files to the remote destination
- [x] Initial startup scan runs in the background so the window opens immediately
- [x] Uploads new files
- [x] Uploads changed files
- [x] Uploads can run in parallel with a bounded concurrency limit
- [x] Automatically uploads newly created files in the local folder without any user action
- [x] Uses a file watcher / continuous watch mechanism so local changes are detected automatically
- [x] Creates remote folders as needed
- [x] Continues background sync after setup

### Settings / Actions

- [x] User can configure local folder
- [x] User can configure WebDAV URL
- [x] User can configure username
- [x] User can configure password
- [x] User can configure remote folder
- [x] Manual save of settings is supported
- [x] WebDAV connection test is supported from the UI
- [x] `Start with Windows` is supported
- [x] `Start with Windows` uses current-user startup registration
- [x] `Start with Windows` is enabled by default unless the user unticks it

### Defaults / Auto-Detection

- [x] Default local folder is `C:\XDSoftware\backups` only when no folder is saved and that path exists
- [x] If XD software is not installed or detectable, the app leaves the local folder empty instead of prefilling it
- [x] If XD software is not installed or detection fails, the app leaves the remote folder empty instead of prefilling it
- [x] If XD-based detection succeeds, the remote folder is prefilled automatically
- [x] Remote folder detection derives the value from XD license data

### Update Flow

- [x] App checks GitHub Releases on startup for updates
- [x] Update check is silent/background
- [x] Startup update checks do not create visible activity/log noise when no update is available
- [x] UPDATE button is hidden on startup
- [x] UPDATE button is shown only when a newer version is detected
- [x] Update-related activity/log entries appear only when an update is available or the user actually starts the update
- [x] Updater downloads the new version
- [x] Updater replaces the executable in place
- [x] Updater restarts the app after replacement

### Current UI Expectations

- [x] Window/branding uses `Backup Sync Tool`
- [x] UI sections are static and not collapsible
- [x] Fonts follow the documented Segoe UI sizing intent
- [x] Connect and Save are primary buttons
- [x] Browse and Show are secondary controls
- [x] Password field has adjacent Show/Hide control
- [x] Sync progress bar is shown during active upload batches
- [x] Tray tooltip shows sync progress while uploads are active
- [x] Recent Activity is a compact transfer feed (`↑ filename` / `↓ filename`)
- [x] Recent Activity does not show timestamps

### Security Requirements

- [x] Password is stored with Windows DPAPI
- [x] Password is never stored as plain text
- [x] Password is recoverable for authentication rather than hashed for storage
- [x] Passwords are not logged
- [x] Authorization headers are not logged
- [x] TLS certificates are validated
- [x] TLS hostnames are validated
- [x] Non-HTTPS WebDAV endpoints are rejected in production use unless explicitly allowed for testing
- [x] Remote delete behavior, if supported, is opt-in

### Logging

- [x] Tray menu item: `Open Logs`
- [x] Recent Activity shows only compact transfer entries (`↑ filename` / `↓ filename`) with no timestamps
