# Agent Instructions

Docs: `SPEC.md` = technical contract + platform checklists. `README.md` = short GitHub summary. **Do not add more markdown** for product/behavior — edit `SPEC.md`. Leave `proxmox/` and `license-inspector/` alone (unrelated tooling).

## Three systems

| System | Where |
| --- | --- |
| Control plane | Laravel `box-rui-cam` → `https://backup.rui.cam` |
| Sync app | this repo |
| Object storage | Garage → `https://s3.rui.cam` |

Do not conflate pairing (`backup.rui.cam`) with object storage (`s3.rui.cam`).

**Never access Forge** (no tokens, deploy, or production `.env`). Operator owns Laravel live env/deploy.

## Build & Launch Rules

After **every** code change that affects the running app, build and relaunch. Do not leave the user on a stale binary.

### Windows (Proxmox VM 102)

```powershell
.\build-local.ps1
```

Always from repo root. Target is **`x86_64-win7-windows-msvc`** (Win7-compatible). Never launch from `target/debug` or `target/release`.

Confirm: release build 0 errors · root `backupsynctool.exe` copied · app running from repo root.

### macOS (this machine / Darwin host)

```bash
./build-macos.sh
```

Script builds, codesigns, kills old process, launches `.app`, checks pid. Confirm: 0 errors · process running. Details in `SPEC.md`.

## Project Rules

- Rust app lives in the repo root.
- UI is raw Win32 through `windows-rs`; do not add egui, nwg, webview, Electron, or an async runtime.
- HTTP uses blocking `ureq`; `rusty_s3` constructs and signs S3 requests. No async runtime / AWS SDK.
- Config is `backupsynctool.json` next to the exe.
- Garage S3 secret and device token are encrypted with Windows DPAPI in `src/secret.rs` (entropy remains `webdavsync-v1`).
- Sync storage goes through `Arc<dyn BackupTransport>` in `src/transport/` — `sync.rs` must not call S3 APIs directly. Transport is S3-only.
- Tray: closing hides; double-click reopens.
- Auto-update replaces exe in place from GitHub releases.
- Config schema must be v2; any legacy configuration requires new pairing.
- Upload is one-way. Restore is an explicit whole-customer download into a new folder.
- `target/` is ignored; do not commit.

## Sync And Pairing (must match SPEC.md)

- **Start sync** via `restart_sync_engine()` — launch (if configured), after pair approval, and Save paths. Pairing must start the engine.
- **First backup:** no v2 local manifest → upload all local files.
- Local manifest lives under `%LOCALAPPDATA%\BackupSyncTool` and updates only after successful upload verification.
- S3: PutObject ≤ `s3_part_size_mib`; larger = persistent multipart. File concurrency capped at 2.
- Pair start sends `supported_transports: ["s3"]`. Non-`s3` `transport` → re-pair.
- Default `pair_api_base` = `https://backup.rui.cam`.
- Logs always on under `logs/` next to exe.

## Storage Errors

- Garage S3 auth/policy failures → pause sync + pair-again UI.
- Missing object is not auth failure.

## UI Notices

- Use `notify_user()` / `notify_user_status()` — no MessageBox for routine notices.

## Release

`.\build-local.ps1` for local cycles. `.\release.ps1` for public `vX.Y.Z` (same Win7 target). Do not force-move tags unless repairing.

## Win32 Gotchas

- `WM_DRAWITEM` only for direct children of parent.
- Preallocate `WM_CTLCOLORSTATIC` brushes in `WndState`.
- `SS_CENTERIMAGE` (`0x0200`) is `SS_REALSIZEIMAGE` — center text manually.
- BGR: `#2B4FA3` → `COLORREF(0x00A34F2B)`.
- `Config::Default` must be explicit.
- `ureq` v2: `.into_string()` + `serde_json::from_str()`.
