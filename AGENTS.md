# Agent Instructions

`SPEC.md` is the technical spec. `README.md` is the GitHub-facing summary. Do not add separate feature/spec markdown for implemented behavior; update `SPEC.md` instead.

## Build & Launch Rules

After every code change:

```powershell
Stop-Process -Name "backupsynctool" -Force -ErrorAction SilentlyContinue
$env:PATH += ";$env:USERPROFILE\.cargo\bin"
cargo build --release
Copy-Item "target\release\backupsynctool.exe" "backupsynctool.exe" -Force
Start-Process "backupsynctool.exe"
```

Always run these commands from the repo root. Never launch from `target/debug` or `target/release`; the app expects `backupsynctool.json` next to the root exe.

Always confirm:

- release build succeeded with 0 errors
- root `backupsynctool.exe` was copied
- app is running from the repo root

## Project Rules

- Rust app lives in the repo root.
- UI is raw Win32 through `windows-rs`; do not add egui, nwg, webview, Electron, or an async runtime.
- HTTP/WebDAV uses blocking `ureq`.
- Config is `backupsynctool.json` next to the exe.
- Password and device token are encrypted with Windows DPAPI in `src/secret.rs`.
- Tray behavior: closing hides to tray, double-click reopens.
- Auto-update checks GitHub releases directly and replaces the exe in place.
- `target/` is ignored and should not be committed.

## Sync And Pairing (must match SPEC.md)

- **Start sync** via `restart_sync_engine()` in `src/ui/utils.rs` — on app launch (if configured), after successful pairing (`on_app_pair_result`), and on Save (`do_save`). Pairing must not end at config save without starting the engine.
- **First backup:** no local `.backupsynctool-manifest.json` + `sync_remote_changes` false → startup uploads every file in `watch_folder`.
- **Local manifest** (`{watch_folder}/.backupsynctool-manifest.json`): last successful upload per path only; updated in `upload_path` after PUT succeeds.
- **Remote manifest:** written from `PROPFIND` (`save_remote_manifest_from_server`), never from a full local scan.
- **Upload skip** (when local manifest exists): local unchanged since last success **and** file present on server with matching size (`remote_file_states`).
- **Logs:** always on; daily files under `logs/` next to the exe (`src/logs.rs`).

## WebDAV Errors

- Only **HTTP 401** → `WebDavError::AuthFailed` → pause sync + pair-again UI.
- **HTTP 403** on `MKCOL` → treat as folder exists (403/405); continue to PUT.
- Do not show “Credentials Invalid” for Storage Box folder-create 403s.

## UI Notices

- Use `notify_user()` / `notify_user_status()` in `src/ui/utils.rs` for non-blocking ribbon + Recent Activity messages.
- Do not add `MessageBox` for routine success/error; it freezes the UI thread. Reserve modals for actions that need explicit Yes/No (e.g. update install).

## Release

Use `.\build-local.ps1` for normal local build/test cycles. It performs the required stop, release build, root exe copy, root launch, and running-process verification.

Use `.\release.ps1` for an actual public release. It bumps the patch version in `Cargo.toml`, builds release, copies `target\release\backupsynctool.exe` to repo-root `backupsynctool.exe`, commits, creates a new `vX.Y.Z` tag, pushes `main`, pushes the tag, and verifies the remote tag exists.

Do not move or force-push an existing release tag during normal releases. Only use `git tag -f` / `git push --force` when explicitly repairing a bad tag or bad release.

## Win32 Gotchas

- `WM_DRAWITEM` only arrives at the parent for direct child controls; avoid intermediate panel windows for owner-drawn controls.
- `WM_CTLCOLORSTATIC` brushes must be preallocated in `WndState`; do not create brushes per message.
- `SS_CENTERIMAGE` (`0x0200`) is `SS_REALSIZEIMAGE` on Win32; use manual text centering instead.
- BGR colour order: `#2B4FA3` is `COLORREF(0x00A34F2B)`.
- `EnableWindow` and `SetFocus` are in `Win32::UI::Input::KeyboardAndMouse`.
- `SetWindowSubclass` and `DefSubclassProc` are in `Win32::UI::Shell`.
- `Config::Default` must be explicit; derived `Default` ignores serde default functions for bool fields.
- `ureq` v2 has no `.into_json()`; use `.into_string()` and `serde_json::from_str()`.
