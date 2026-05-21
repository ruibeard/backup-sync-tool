# Backup Sync Tool

Native Windows tray app that backs up one local folder to WebDAV. Pairing via a Laravel admin app supplies credentials; uploads go direct to WebDAV (no proxy).

## Features

- System tray — close hides; double-click restores
- Recursive folder watch + debounced uploads
- Parallel uploads (`parallel_uploads`, default 10)
- First-run baseline upload when no local manifest
- Optional download-from-server (`sync_remote_changes`)
- Admin pairing (QR/code) — server owns destination folder
- DPAPI-encrypted password + device token
- Recent Activity + sync footer progress
- Silent GitHub auto-update

## Requirements

- Windows 10+
- WebDAV server + [pairing API](SPEC.md#pairing-api) (default base `https://box.rui.cam`)

## Install

Download `backupsynctool.exe` from [Releases](https://github.com/ruibeard/backup-sync-tool/releases/latest). Place `backupsynctool.json` next to the exe.

## Use

1. Set **backup folder** (or use detected `C:\XDSoftware\backups` when present).
2. **Pair** — scan QR / enter code; admin approves customer folder on server.
3. Sync starts automatically after pairing (no Save button).
4. **Reconnect** if WebDAV returns HTTP 401.

Settings auto-save on folder browse and checkbox changes.

## Build (developers)

```powershell
.\build-local.ps1
```

Public release: `.\release.ps1` (bumps version, tags `vX.Y.Z`, pushes).

Details: [SPEC.md](SPEC.md) · Agent rules: [AGENTS.md](AGENTS.md) (LLM/Cursor only)

## Repo layout

| Path | Role |
| --- | --- |
| `src/` | Rust app (Win32 UI, sync, WebDAV, pairing) |
| `license-inspector/` | Optional XD licence diagnostic helper |
| `mockups.html` | UI layout reference |
| `build-local.ps1` / `release.ps1` | Build & release scripts |

## Security note

Desktop folder lock prevents accidental wrong-customer uploads. Hard tenant isolation needs server-scoped WebDAV credentials per customer ([SPEC.md](SPEC.md#security)).
