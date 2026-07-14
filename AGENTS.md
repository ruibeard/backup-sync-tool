# Agent Instructions

Docs: `SPEC.md` = technical contract + platform checklists. `README.md` = short GitHub summary. **Do not add more markdown** for product/behavior — edit `SPEC.md`. Leave `proxmox/` and `license-inspector/` alone (unrelated tooling).

## Three systems

| System | Where |
| --- | --- |
| Control plane | Laravel `box-rui-cam` — public `APP_URL`. Desktop `pair_api_base` must match (default `https://backup.rui.cam`; editable + persisted) |
| Sync app | this repo — native shells supervise the bundled engine |
| Sync hub | CT 105 — always-online Syncthing peer with one folder per customer and staggered versioning |

Do not conflate pairing (`pair_api_base`) with Syncthing hub addresses. Desktop does not choose or expose the control-plane storage implementation. A sync domain is optional; approved `syncthing_hub_addresses` come only from Laravel.

**Never access Forge** (no tokens, deploy, or production `.env`). Operator owns Laravel live env/deploy.

## YOU DO — operator smoke

After builds, operator (not agent) smokes Control plane URL:

1. Laravel `APP_URL` = public control-plane base.
2. Windows: `.\build-windows.ps1` → **CONTROL PLANE URL** = that `APP_URL` (blur + pair persist) → pair → confirm CT 105 sync.
3. Mac: `./build-macos.sh` → tray **Control plane URL…** → same → pair → confirm CT 105 sync.
4. Report failures; fix any `control_plane_url mismatch` in logs.

Windows 7, current Windows, and macOS smoke must cover initial convergence, edits, conflicts, renames, offline changes, deletion propagation from every device, and CT 105 staggered-version recovery.

## Build & Launch Rules

After **every** code change that affects the running app, rebuild. Do not leave a stale binary.

Three scripts only:

| Script | Use |
| --- | --- |
| `./build-macos.sh` | Mac: build + bundle pinned engine + launch `.app` (`--package` / `--install` / `--no-launch` / `--identity=…`) |
| `.\build-windows.ps1` | Any Windows machine with Rust/VS: Win7 desktop + pinned Win7-compatible engine + `dist\windows\` (`-NoLaunch` to skip run) |
| `./release.sh` | Mac: needs the complete Windows app/engine bundle already staged → bump + mac package + tag + GitHub |

Never launch from `target/debug` or `target/release`. Confirm: 0 errors · process running (or both app and engine are present with no-launch). The engine is pinned to Syncthing v2.1.1. Its self-updater is disabled; upgrades happen only through a tested Backup Sync Tool release.

## Project Rules

- Rust app lives in the repo root.
- UI is raw Win32 through `windows-rs`; do not add egui, nwg, webview, Electron, or an async runtime.
- HTTP uses blocking `ureq`; no async runtime or AWS SDK.
- Config is `backupsynctool.json` next to the exe on Windows and under app support on macOS.
- Device token: Windows DPAPI in `src/secret.rs` (established entropy `webdavsync-v1`); macOS Keychain via `security … -A` (no Keychain password prompts on ad-hoc rebuilds).
- Syncthing identity (`cert.pem` / `key.pem`), config, database, and API key stay under the private app-support `syncthing/` home. Never send or log the API key or identity key.
- Desktop sync goes only through the bundled Syncthing v2.1.1 supervisor and its loopback REST/event API. Do not add a custom transport, manifest, watcher, multipart uploader, lease, tombstone, or file-transfer implementation.
- CT 105 is the only approved remote device in the private engine. Re-pair replaces stale device/folder assignments.
- Tray: closing hides; double-click reopens.
- Auto-update must replace the desktop and bundled engine as one tested bundle.
- Config schema must be v3; S3, WebDAV, v2, and other legacy configurations require new pairing.
- Every approved desktop folder is `sendreceive`. Every device propagates creates, edits, renames, conflicts, and deletions.
- CT 105 keeps staggered versions. Recovery uses hub versions; do not restore the removed whole-customer object-store workflow.
- `target/` is ignored; do not commit.

## Sync And Pairing (must match SPEC.md)

- Start sync through the platform's Syncthing restart/supervisor path on launch (if configured), after pair approval, and after saving a watch path. Pairing must start the engine.
- Pair start first creates/loads the private identity, queries `/rest/system/status`, and sends `syncthing_device_id` plus `supported_transports: ["syncthing"]`.
- Approval must contain `transport: "syncthing"`, `device_uuid`, `device_token`, `syncthing_hub_device_id`, `syncthing_hub_addresses`, `syncthing_folder_id`, and `syncthing_folder_label`.
- Device IDs are exactly eight seven-character uppercase Syncthing base32 groups (`A-Z`, `2-7`) separated by hyphens.
- Approval provisions the device/customer folder on CT 105 before returning success. Desktop validates the assignment, writes one `sendreceive` folder and one hub device through `/rest/config`, triggers `/rest/db/scan`, atomically saves schema v3, and starts monitoring.
- All approved devices may delete. Do not add `can_delete_files` or Syncthing `ignoreDelete`.
- Default `pair_api_base` = `https://backup.rui.cam`; editable + persisted (Windows **CONTROL PLANE URL** on blur + pair; macOS tray **Control plane URL…**). Shown during pair. Optional Laravel `control_plane_url` on pair/start → mismatch log if different.
- The Syncthing GUI/API binds only to `127.0.0.1:8385`, uses its private API key, has a hidden GUI login, and never opens a browser. Peer sync ports/addresses are distinct from the GUI/API.
- Launch the engine with private home, `--no-browser`, `--no-restart`, and `--no-upgrade`; forward stdout/stderr to app logs and shut it down with the app.
- Runtime status comes from `/rest/db/status`, hub `/rest/db/completion`, `/rest/system/connections`, and `/rest/events`.
- Logs always on under `logs/` next to the exe on Windows and app support on macOS.

## Sync Errors

- Missing engine, wrong engine version, startup exit, private API failure, identity mismatch, malformed approval, invalid hub assignment, or rejected config must stop activation and show a visible reconnect/error state.
- Normal offline hub state is not credential failure. Syncthing owns retry and convergence.
- A failed new approval must not silently preserve unauthorized stale hub/folder assignments.

## UI Notices

- Use `notify_user()` / `notify_user_status()` — no MessageBox for routine notices.
- Do not expose Syncthing's browser GUI. Native Windows/macOS UI remains the product surface.

## Release

`./build-macos.sh` / `.\build-windows.ps1` for cycles; `./release.sh` for `vX.Y.Z` (complete Windows app/engine bundle must already be in `dist/windows/`). Do not force-move tags unless repairing.

## Win32 Gotchas

- `WM_DRAWITEM` only for direct children of parent.
- Preallocate `WM_CTLCOLORSTATIC` brushes in `WndState`.
- `SS_CENTERIMAGE` (`0x0200`) is `SS_REALSIZEIMAGE` — center text manually.
- BGR: `#2B4FA3` → `COLORREF(0x00A34F2B)`.
- `Config::Default` must be explicit.
- `ureq` v2: `.into_string()` + `serde_json::from_str()`.
