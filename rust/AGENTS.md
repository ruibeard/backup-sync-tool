# Rust App — Agent Instructions & Feature History

## Build & Launch Rules (MANDATORY)

- **After every code change: rebuild and relaunch the app before stopping.**
- Kill the running process first: `Stop-Process -Name "backupsynctool" -Force -ErrorAction SilentlyContinue`
- Build: `$env:PATH += ";$env:USERPROFILE\.cargo\bin"; cargo build`  (from `rust/`)
- Launch: `Start-Process "target\debug\backupsynctool.exe"`
- Always confirm: build succeeded (0 errors) + app is running.

---

## Project Rules

- Rust app lives in `backup-sync-tool/rust/`
- UI: raw Win32 API via `windows-rs` crate — no egui, no nwg
- No async runtime — blocking `ureq` for HTTP
- Config stored as JSON next to the `.exe` (`backupsynctool.json`)
- Password encrypted with Windows DPAPI (`secret.rs`)
- Tray icon app — closing window hides to tray, double-click reopens
- Auto-update: checks GitHub releases API directly, downloads, replaces in place, restarts
- **Do not modify** `cpp/` or `legacy-win32/` (legacy reference — read only)
- `rust/target/` is in `.gitignore`

---

## How to Release a New Version

1. Bump `version` in `rust/Cargo.toml` (e.g. `"0.2.0"` → `"0.3.0"`)
2. `cargo build --release` — binary at `rust/target/release/backupsynctool.exe`
3. Copy `backupsynctool.exe` to repo root and commit
4. Move the tag: `git tag -f vX.Y.Z && git push origin vX.Y.Z --force`

The app checks GitHub releases API on startup. If a newer version is found, the UPDATE button appears.

---

## Architecture

| File | Purpose |
|---|---|
| `src/main.rs` | Entry point, registers window class, message loop |
| `src/ui.rs` | All Win32 UI — window proc, controls, paint, layout |
| `src/config.rs` | Load/save `backupsynctool.json` next to exe |
| `src/secret.rs` | DPAPI encrypt/decrypt for password |
| `src/webdav.rs` | WebDAV HTTP client (ureq, blocking) |
| `src/sync.rs` | File watcher + upload sync engine |
| `src/tray.rs` | System tray icon + context menu |
| `src/updater.rs` | Checks GitHub releases API, downloads, bat-swap-restart |
| `build.rs` | Embeds icons + manifest into the exe |
| `assets/` | `app.ico`, `app-idle.ico`, `app-syncing.ico` |

---

## UI Design

- Window bg: `#F0F0F0` — Card bg: `#F8F8F8` — Card border: `#DEDEDE`
- Labels: `#333333` Segoe UI 12pt — Section headers: `#888888` Segoe UI 10pt SemiBold ALL CAPS
- Input border: `#CCCCCC` (blue `#2B4FA3` on focus)
- Blue buttons (Connect, Save): `#2B4FA3` white text
- Grey buttons (Browse, Close, Show): `#E8E8E8` `#333333` text
- Sections are NOT collapsible — static layout
- All controls are direct children of `hwnd` (no intermediate panels)
- Card backgrounds painted in `WM_PAINT` using stored `CardRect` list

---

## Known Win32 Gotchas (do not re-learn these)

- `WM_DRAWITEM` only arrives at parent if controls are **direct children of hwnd** — never use intermediate panel windows
- `WM_CTLCOLORSTATIC` brush must be **pre-allocated** in `WndState` — never create per message (leak)
- `SS_CENTERIMAGE` (0x0200) = `SS_REALSIZEIMAGE` on Win32 — use manual `y + (h - txt_h) / 2` instead
- BGR colour order: `#2B4FA3` → COLORREF = `0x00A34F2B`
- `EnableWindow`/`SetFocus` are in `Win32::UI::Input::KeyboardAndMouse`
- `SetWindowSubclass`/`DefSubclassProc` are in `Win32::UI::Shell`
- `Config::Default` must be an explicit `impl` — derived `Default` gives `false` for `bool` fields ignoring serde defaults
- `ureq` v2 has no `.into_json()` — use `.into_string().ok()? + serde_json::from_str()` instead

---

## Feature History (this conversation)

### Implemented
- Full Rust Win32 rewrite of the C# WPF app
- Raw Win32 UI matching C# design (cards, colours, fonts)
- DPAPI password encryption/decryption
- System tray icon (idle/syncing), double-click to reopen, right-click to exit
- WebDAV connection test (Connect button)
- File watcher + upload sync engine
- Auto-updater: silent background check, UPDATE button appears only when newer version found, bat-file swap-restart
- Config saved as JSON next to exe
- `Start with Windows` registry key (HKCU Run)
- Build embeds icons + manifest
- Renamed binary from `webdavsync` to `backupsynctool`
- Removed `appcast.json` — updater now uses GitHub releases API directly

### User-specified defaults & behaviour
- Default local folder: `C:\XDSoftware\backups` (pre-filled if empty)
- `Start with Windows` defaults to **ON** (explicit `impl Default` — not derived)
- App name: **"Backup Sync Tool"** (window title, version label, registry key)
- Sections are **not collapsible** — user removed toggle logic
- Fonts: 12pt normal, 10pt SemiBold for headers (not larger)
- UPDATE button is **hidden on startup**, only shown when update detected
- Password **Show/Hide toggle button** next to password field
- Updater uses GitHub releases API: `https://api.github.com/repos/ruibeard/backup-sync-tool/releases/latest`
