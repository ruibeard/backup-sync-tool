# Backup Sync Tool — Technical Spec v2

## Architecture

| Layer | Windows | macOS |
| --- | --- | --- |
| UI | Raw Win32 through `windows-rs` | Menu bar app (default); `--daemon` for LaunchAgent |
| HTTP | Blocking `ureq` | same |
| S3 request construction | `rusty_s3` Sans-I/O actions | same |
| Watcher | `notify` | `notify` (FSEvents) |
| Secrets | Windows DPAPI | Keychain (`cam.rui.backupsynctool`) |
| Control plane | `pair_api_base` (default `https://backup.rui.cam`; editable + persisted) | same |
| Object storage | Garage `s3_*` from pair **approve** only — desktop never picks provider | same |

Windows client: Windows 7 SP1 x64 through Windows 11.  
macOS client: Apple Silicon / Intel Darwin; local `./build-macos.sh` uses **ad-hoc** codesign by default (no Keychain password prompts). Pass `--identity=…` or `MACOS_SIGN_IDENTITY=…` only when you want a real cert (e.g. package/release). Not notarized in v1.

Neither client uses WebDAV, async runtime, AWS SDK, Electron/webview, or data-migration logic. **XD licence detection is Windows-only.**

## Configuration

`backupsynctool.json` sits next to the executable. Only `schema_version: 2` with `transport: "s3"` is accepted as paired configuration. Everything else starts unpaired.

### Control plane URL (`pair_api_base`)

Must match the Laravel install’s public `APP_URL` (no trailing slash). Default `https://backup.rui.cam`; not locked to that host.

| Platform | How to set | Persist |
| --- | --- | --- |
| Windows | Pair window **Change Server** | Before starting the replacement request |
| macOS | Pair window **Change Server…** or tray **Control plane URL…** | Before starting the replacement request |

During pair, the UI shows which control plane is in use. `POST /api/pair/start` may return optional `control_plane_url`; if present and it differs from configured `pair_api_base`, the client logs `control_plane_url mismatch: configured=… echoed=…`. Garage `s3_*` credentials and endpoint still come **only** from pair approve — desktop does not choose storage.

On macOS, `s3_secret_enc` / `device_token_enc` store Keychain handles (`kc1:<account>`), not DPAPI blobs. `start_with_windows` means **start at login** (LaunchAgent → `backupsynctool --daemon`).

### macOS Keychain (secrets)

Service: `cam.rui.backupsynctool`. Config stores opaque `kc1:<account>` handles. New pairing credentials use unique candidate account names so the active handles remain readable until the candidate config is verified and atomically installed.

| Rule | Detail |
| --- | --- |
| Store | `security add-generic-password … -A` into unique candidate accounts (`src/secret.rs`). `-A` = any app may read without a Keychain UI prompt — required because ad-hoc codesign changes CDHash every local rebuild. Old handles are removed only after atomic config replacement. |
| Load | CLI `find-generic-password -w` with a **2 s timeout**. On timeout or auth failure, delete the stale item and fail closed (no hang, no password dialog). |
| Startup | `purge_stale_keychain_handles()` runs before decrypt in `SyncHost::load`. |
| Local build | `./build-macos.sh` defaults to ad-hoc (`--sign -`). Combined with `-A` storage, rebuild + relaunch must not ask for the login Keychain password. |
| Real signing | `--identity=…` / `MACOS_SIGN_IDENTITY` only for package/release — never the default dev loop. |
| Migration | Items created before `-A` (or via old ACL-bound APIs) may be removed on first launch after upgrade; **re-pair once** if sync stops — that is pairing UI, not a Keychain password prompt. |

Do **not** add signing-identity helper scripts or `security add-trusted-cert` to the dev workflow.

```json
{
  "schema_version": 2,
  "pair_api_base": "https://backup.rui.cam",
  "watch_folder": "C:\\XDSoftware\\backups",
  "remote_folder": "XDPT.59655-Palmeira-Minimercado",
  "transport": "s3",
  "s3_endpoint": "https://s3.rui.cam",
  "s3_region": "garage",
  "s3_bucket": "XDPT.59655-Palmeira-Minimercado",
  "s3_access_key": "GK...",
  "s3_secret_enc": "DPAPI...",
  "s3_path_style": true,
  "s3_prefix": "",
  "device_uuid": "...",
  "device_token_enc": "DPAPI...",
  "credential_profile_id": 1,
  "credential_version": 1,
  "start_with_windows": true,
  "auto_update": true,
  "parallel_uploads": 2,
  "s3_part_size_mib": 32
}
```

`s3_bucket` is the Garage bucket alias (uploads). `remote_folder` is the admin-approved customer label (XD style when detected). Newly provisioned destinations use the same string for both; case is preserved.

Local sync state:

| Platform | Manifest / multipart root |
| --- | --- |
| Windows | `%LOCALAPPDATA%\BackupSyncTool` |
| macOS | `~/Library/Application Support/BackupSyncTool` |

## XD detection and pairing

XD detection is optional and checks only:

- `C:\XDSoftware`
- `C:\XDSoftware\backups`
- `C:\XDSoftware\cfg\xd.lic`
- `C:\XDSoftware\cfg\xd.pem`

The app decrypts `Number` and `ClientComercialName` and sends them separately with the detected install/backup paths and suggested customer label. A manually chosen folder does not pretend to be an XD installation. Pairing remains available when detection fails.

The QR popup is a dedicated pairing window: title “Scan to pair…”, large QR of the approve URL, status “Waiting for admin approval…”, pairing code, expiry note, approve link, active control-plane base, **Cancel**, and **Change Server**. Change Server cancels local polling, validates and persists the new control-plane URL, then creates a fresh request. Windows uses Win32 (`pair_qr.rs`); macOS uses a modeless `NSPanel` with the same workflow. The client polls typed pending/approved/rejected/expired/failed states; transport and malformed response errors are never silently converted to pending.

An approved response must contain `device_uuid`, device token, S3 endpoint/region/bucket/access key/secret, and the admin-approved customer name. Before local activation, the client proves List plus temporary Put/Head/Delete access under `.backupsynctool-validation/`. It stages DPAPI/Keychain secrets, atomically replaces the complete config, then starts the initial upload. Cancellation, rejection, and timeout leave the active local config untouched. The current Laravel backend may already have revoked the old key after an approval, so a failed post-approval validation is shown as **Reconnect required**; true server-side rollback requires the separately planned two-phase activation API.

macOS sends its chosen backup path and a `{hostname}-{folder}` suggestion, but never sends XD licence fields.

Wire contract: `box-rui-cam/BACKUP_SYNC_COMMUNICATION_SPEC.md`.

## YOU DO — operator smoke (Control plane URL)

Agent does not run Windows VM / interactive GUI smoke. Operator:

1. Confirm Laravel `APP_URL` is the public control-plane URL you intend to pair with.
2. **Windows:** `.\build-windows.ps1` → set **CONTROL PLANE URL** to that `APP_URL` → pair → QR/status shows that server → admin approve → upload. Report failures.
3. **Mac:** `./build-macos.sh` → tray **Control plane URL…** → same `APP_URL` → pair → confirm. Report failures.
4. Mismatch log `control_plane_url mismatch` → fix desktop URL or Laravel `APP_URL` until they match.

## Upload engine

- Upload-only: startup scan plus recursive watcher for new/changed files.
- Preserve each relative path at the customer bucket root.
- Never delete a remote object because a local file disappeared.
- Local manifest is keyed to `device_uuid` and stored atomically under the platform app-support `state-v2` directory (see table above).
- Update the manifest only after S3 verifies the successful object size.
- Periodically rescan and heal missing/size-mismatched objects.
- Maximum two concurrent file uploads.
- Transfer workers emit typed per-file events with actual bytes completed and total. The UI retains at most 200 rows from the current process run and never reconstructs activity from log text.
- Connection state is independent of transfer state: a running watcher is not displayed as an active upload.
- Ordinary failures remain visible with **Retry Failed**. S3 authentication/policy failures stop the engine and show **Reconnect required**.

Files at or below `s3_part_size_mib` use streamed PutObject. Larger files use persistent multipart:

- State under app-support `multipart-v1` records source identity, upload ID, completed part number/size/ETag/digest, and phase.
- Reconcile saved state with ListParts and never adopt server-only parts.
- Retry transient idempotent operations.
- Abort/restart if the source size or nanosecond mtime changes.
- Verify completed object size and upload token before updating the manifest.
- `rusty_s3` owns URL construction and SigV4 query signing; transport code owns blocking I/O and resume policy.

## Restore

**Restore** is explicit; there is no automatic server-to-client synchronization.

1. User chooses an existing parent directory.
2. App creates a unique `<customer>-restore-<timestamp>` child directory and never reuses it.
3. List every object in the approved customer bucket.
4. Reject absolute paths, parent traversal, prefixes, NULs, and empty keys.
5. Stream each object to a `.part` file and atomically rename it on completion.
6. Preserve relative directories and available source modification times.
7. Report real byte progress and failed paths in the main window. Restore can be cancelled; partial `.part` files are removed. Authentication failures require new pairing.

Restore starts from the secondary tray/menu and focuses the main window. It is not a primary main-window action.

## Build and verification

Three scripts only: `./build-macos.sh` · `.\build-windows.ps1` · `./release.sh`.

| Script | What |
| --- | --- |
| `./build-macos.sh` | Release `.app` under `dist/macos/`, ad-hoc codesign by default. Flags: `--install`, `--no-launch`, `--package`, `--identity=…`. Never `open` the raw binary. |
| `.\build-windows.ps1` | Any Windows machine (Rust nightly + VS Build Tools). Target `x86_64-win7-windows-msvc` → root `backupsynctool.exe` + `dist\windows\`. `-NoLaunch` skips run. |
| `./release.sh` | Clean tree. Requires `dist/windows/backupsynctool.exe` already built on Windows. Bump → mac package → tag `vX.Y.Z` → `gh` upload. |

### Icon assets

**Source (commit these):** `assets/originals/*.svg` (9 shield masters) + `assets/bridge-pc.svg` + `assets/github.ico` + `assets/render-icons.py`.

**Generated (run script after SVG edits — do not hand-edit):**

| File(s) | Platform | Why |
| --- | --- | --- |
| `menubar-icon.png`, `menubar-syncing.png`, `menubar-complete.png` | macOS | Menu bar tray (3 states; one syncing frame) |
| `AppIcon.icns` | macOS | Dock / Finder icon |
| `app-idle.ico`, `complete.ico` | Windows | Tray idle + done |
| `syncing.ico`, `syncing2.ico` … `syncing7.ico` | Windows | 7-frame tray animation (`syncing1`–`syncing6` SVGs + frame 0) |
| `bridge-pc.png`, `bridge-server.png` | Windows | Status window bridge tiles |

macOS does **not** load the `.ico` files. Windows does **not** load the menubar PNGs. Most of `assets/` bulk is Windows ICO sizes embedded by `build.rs`.

```bash
python3 assets/render-icons.py   # cairosvg, Pillow, ImageMagick, iconutil
```

| Action | How |
| --- | --- |
| Main window | Manual launch or **left-click** menubar shield opens the full titled `NSWindow` directly. Closing hides it to the menu bar. |
| Tray menu | Secondary click → **Open Backup Sync Tool…** / **Restore Backup…** / **Open Logs** / **Control plane URL…** / **Quit Backup Sync**. |
| Shortcuts | Minimal `NSApp` main menu: **⌘Q** Quit · **⌘W** Close frontmost (status hides to menubar; pair QR closes) |
| Logs | Tray menu **Open Logs** |
| Quit | Tray menu **Quit Backup Sync** (also ⌘Q) |
| Daemon only | `backupsynctool --daemon` |

Pairing QR remains a separate `NSPanel` (`notify.rs`). Routine notices use `notify_user()` / tray tips — not `NSAlert` action sheets for primary workflow. Manual launch shows the main window; LaunchAgent `--daemon` remains hidden.

Config/state: `~/Library/Application Support/BackupSyncTool/` · Logs: `~/Library/Application Support/BackupSyncTool/logs/` (outside the sealed `.app`) · Secrets: Keychain `cam.rui.backupsynctool` (see Keychain table above).

Checklist: menubar icon · click → full window · watch folder · pair QR / Change Server / Cancel → automatic initial sync · two live filenames with byte percentages · retry ordinary failure · auth pause · quit/relaunch **no Keychain password prompt** (ad-hoc + `-A`) · tray Restore with progress/cancel · login toggle → `~/Library/LaunchAgents/` · daemon when configured · second instance takeover · idle RSS ≤ 20 MB (`ps -o rss= -p $(pgrep -n backupsynctool)`).

Limits: not notarized; release assets `backupsynctool.exe` + `backupsynctool-macos-*.tar.gz` on GitHub Releases.
