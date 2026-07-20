# Agent Instructions

Docs: `SPEC.md` = technical contract + platform checklists. `README.md` = short GitHub summary. **Do not add more markdown** for product/behavior — edit `SPEC.md`. Leave `proxmox/` and `license-inspector/` alone (unrelated tooling).

## Locked architecture (Option H)

Mini-Dropbox: Laravel owns pairing + sync metadata + 30-day history + file shelf. Desktop owns the Rust sync engine. Chunk bytes live in an S3-compatible object store behind a Laravel storage driver. Last-writer-wins. Greenfield — ignore WebDAV/Syncthing/CT 105 product paths.

| System | Where |
| --- | --- |
| Control + metadata | Laravel — public `APP_URL`. Desktop `pair_api_base` must match (default `https://backup.rui.cam`; editable + persisted) |
| Sync app | this repo — native Win/Mac shells + in-process chunk sync engine |
| Chunk store | Object store via `BACKUP_STORAGE_DRIVER` (`garage` default; `b2` / `r2` / etc. later) |

Desktop does not choose or expose the storage vendor. Approval returns `transport: "chunk_store"` plus device token and chunk credentials.

**Never access Forge** (no tokens, deploy, or production `.env`). Operator owns Laravel live env/deploy.

## YOU DO — operator smoke

After builds, operator (not agent) smokes Control plane URL:

1. Laravel `APP_URL` = public control-plane base.
2. Windows: `.\build-windows.ps1` → **CONTROL PLANE URL** = that `APP_URL` → pair → two-way sync against chunk store.
3. Mac: `./build-macos.sh` → tray **Control plane URL…** → same → pair → two-way sync.
4. Confirm Laravel shelf sees files; revoke one device; fix any `control_plane_url mismatch` in logs.

Win7, current Windows, and macOS smoke must cover initial convergence, edits, renames, offline changes, deletion propagation, and last-writer-wins under concurrent edit (30-day history retains loser).

## Build & Launch Rules

After **every** code change that affects the running app, rebuild. Do not leave a stale binary.

Three scripts only:

| Script | Use |
| --- | --- |
| `./build-macos.sh` | Mac: build + launch `.app` (`--package` / `--install` / `--no-launch` / `--identity=…`) |
| `.\build-windows.ps1` | Win7 desktop → `dist\windows\` (`-NoLaunch` to skip run) |
| `./release.sh` | Mac: needs Windows bundle already staged → bump + mac package + tag + GitHub |

Never launch from `target/debug` or `target/release`. Confirm: 0 errors · process running (or staged with no-launch).

## Project Rules

- Rust app lives in the repo root.
- UI is raw Win32 through `windows-rs`; do not add egui, nwg, webview, Electron, or an async runtime.
- HTTP uses blocking `ureq`; no async runtime or AWS SDK.
- Config is `backupsynctool.json` next to the exe on Windows and under app support on macOS.
- Device token / chunk secrets: Windows DPAPI in `src/secret.rs` (entropy `webdavsync-v1`); macOS Keychain via `security … -A` (no Keychain password prompts on ad-hoc rebuilds).
- Sync is the in-process Rust engine (chunk + metadata protocol in `SPEC.md`). Do not reintroduce Syncthing, WebDAV, or a second transfer stack.
- Tray: closing hides; double-click reopens.
- Auto-update replaces one tested desktop bundle.
- Config schema must be v4; older schemas require new pairing.
- Every approved device may create, edit, rename, and delete. No `can_delete_files`.
- Conflicts are last-writer-wins; no `.sync-conflict` copies.
- Version/tombstone retention is 30 days in Laravel.
- `target/` is ignored; do not commit.

## Sync And Pairing (must match SPEC.md)

- Start sync on launch (if configured), after pair approval, and after saving a watch path.
- Pair start sends `supported_transports: ["chunk_store"]` (plus machine/XD hints).
- Approval must contain `transport: "chunk_store"`, `device_uuid`, `device_token`, `destination_uuid`, and chunk-store fields (`chunk_endpoint`, `chunk_bucket`, keys, etc.).
- Desktop validates, stores secrets, atomically saves schema v4, and starts the sync loop.
- Default `pair_api_base` = `https://backup.rui.cam`; editable + persisted. Optional Laravel `control_plane_url` on pair/start → mismatch log if different.
- Metadata calls use the device token. Chunk PUT/GET use device chunk credentials against the object store only.
- Logs always on under `logs/` next to the exe on Windows and app support on macOS.

## Sync Errors

- Malformed approval, storage auth failure after revoke, or rejected commits must show a visible reconnect/error state.
- Normal offline object-store/Laravel state is not credential failure; engine retries.
- A failed new approval must not silently preserve unauthorized credentials.

## UI Notices

- Use `notify_user()` / `notify_user_status()` — no MessageBox for routine notices.
- Native Windows/macOS UI remains the product surface.

## Release

`./build-macos.sh` / `.\build-windows.ps1` for cycles; `./release.sh` for `vX.Y.Z` (complete Windows bundle must already be in `dist/windows/`). Do not force-move tags unless repairing.

## Win32 Gotchas

- `WM_DRAWITEM` only for direct children of parent.
- Preallocate `WM_CTLCOLORSTATIC` brushes in `WndState`.
- `SS_CENTERIMAGE` (`0x0200`) is `SS_REALSIZEIMAGE` — center text manually.
- BGR: `#2B4FA3` → `COLORREF(0x00A34F2B)`.
- `Config::Default` must be explicit.
- `ureq` v2: `.into_string()` + `serde_json::from_str()`.
