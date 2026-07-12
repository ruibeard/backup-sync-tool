# Agent Instructions

`SPEC.md` is the technical spec. `README.md` is the GitHub-facing summary. Do not add separate feature/spec markdown for implemented behavior; update `SPEC.md` instead.

## Three systems

| System | Where |
| --- | --- |
| Control plane | Laravel `box-rui-cam` → `https://backup.rui.cam` |
| Sync app | this repo |
| Object storage | MinIO → `https://s3.rui.cam` |

Do not conflate pairing (`backup.rui.cam`) with object storage (`s3.rui.cam`). Legacy `box.rui.cam` = old WebDAV host.

**Never access Forge** (no tokens, deploy, or production `.env`). Operator owns Laravel live env/deploy. Cutover status: `docs/plans/2026-07-11-HANDOFF.md` / `SPEC.md` checklist.

## Build & Launch Rules

After every code change, on a **Windows** host (Proxmox VM 102):

```powershell
.\build-local.ps1
```

Always from repo root. Target is **`x86_64-win7-windows-msvc`** (Win7-compatible). Never launch from `target/debug` or `target/release`.

Confirm: release build 0 errors · root `backupsynctool.exe` copied · app running from repo root.

## Project Rules

- Rust app lives in the repo root.
- UI is raw Win32 through `windows-rs`; do not add egui, nwg, webview, Electron, or an async runtime.
- HTTP uses blocking `ureq` (S3 SigV4 + pairing API). No async runtime / AWS SDK.
- Config is `backupsynctool.json` next to the exe.
- S3 secret and device token are encrypted with Windows DPAPI in `src/secret.rs` (entropy remains `webdavsync-v1`).
- Sync storage goes through `Arc<dyn BackupTransport>` in `src/transport/` — `sync.rs` must not call S3 APIs directly. Transport is S3-only.
- Tray: closing hides; double-click reopens.
- Auto-update replaces exe in place from GitHub releases.
- `target/` is ignored; do not commit.

## Sync And Pairing (must match SPEC.md)

- **Start sync** via `restart_sync_engine()` — launch (if configured), after pair approval, and Save paths. Pairing must start the engine.
- **First backup:** no local manifest + `sync_remote_changes` false → upload all local files.
- Local manifest updated only after successful upload; remote manifest from server listing only.
- S3: PutObject ≤ `s3_part_size_mib`; larger = persistent multipart. File concurrency capped at 2.
- Pair start sends `supported_transports: ["s3"]`. Empty/`webdav` `transport` → re-pair.
- Default `pair_api_base` = `https://backup.rui.cam`.
- Logs always on under `logs/` next to exe.

## Storage Errors

- S3 auth/policy failures → pause sync + pair-again UI.
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
