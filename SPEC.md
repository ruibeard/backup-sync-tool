# Backup Sync Tool — Technical Spec v2

## Architecture

| Layer | Windows | macOS (v1) |
| --- | --- | --- |
| UI | Raw Win32 through `windows-rs` | Menu bar app (default); `--daemon` for LaunchAgent |
| HTTP | Blocking `ureq` | same |
| S3 request construction | `rusty_s3` Sans-I/O actions | same |
| Watcher | `notify` | `notify` (FSEvents) |
| Secrets | Windows DPAPI | Keychain (`cam.rui.backupsynctool`) |
| Control plane | `https://backup.rui.cam` | same |
| Object storage | Garage at `https://s3.rui.cam` | same |

Windows client: Windows 7 SP1 x64 through Windows 11.  
macOS client: Apple Silicon / Intel Darwin; local `./build-macos.sh` uses **ad-hoc** codesign by default (no Keychain password prompts). Pass `--identity=…` or `MACOS_SIGN_IDENTITY=…` only when you want a real cert (e.g. package/release). Not notarized in v1.

Neither client uses WebDAV, async runtime, AWS SDK, Electron/webview, or data-migration logic. **XD licence detection is Windows-only.**

## Configuration

`backupsynctool.json` sits next to the executable. Only `schema_version: 2` with `transport: "s3"` is accepted as paired configuration. Everything else starts unpaired.

On macOS, `s3_secret_enc` / `device_token_enc` store Keychain handles (`kc1:<account>`), not DPAPI blobs. `start_with_windows` means **start at login** (LaunchAgent → `backupsynctool --daemon`).

### macOS Keychain (secrets)

Service: `cam.rui.backupsynctool`. Accounts: `s3_secret`, `device_token`.

| Rule | Detail |
| --- | --- |
| Store | `security add-generic-password … -A` after deleting any existing row for that account (`src/secret.rs`). `-A` = any app may read without a Keychain UI prompt — required because ad-hoc codesign changes CDHash every local rebuild. |
| Load | CLI `find-generic-password -w` with a **2 s timeout**. On timeout or auth failure, delete the stale item and fail closed (no hang, no password dialog). |
| Startup | `purge_stale_keychain_handles()` runs before decrypt in `SyncHost::load`. |
| Local build | `./build-macos.sh` defaults to ad-hoc (`--sign -`). Combined with `-A` storage, rebuild + relaunch must not ask for the login Keychain password. |
| Real signing | `--identity=…` / `MACOS_SIGN_IDENTITY` only for package/release — never the default dev loop. |
| Migration | Items created before `-A` (or via old ACL-bound APIs) may be removed on first launch after upgrade; **re-pair once** if sync stops — that is pairing UI, not a Keychain password prompt. |

Do **not** add signing-identity helper scripts or `security add-trusted-cert` to the dev workflow.

```json
{
  "schema_version": 2,
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

The QR popup is a dedicated pairing window (~380×500): title “Scan to pair…”, large QR of the approve URL, status “Waiting for admin approval…”, pairing code, expiry note, and approve link. Windows uses Win32 (`pair_qr.rs`); macOS uses a modeless `NSPanel` with the same layout. The client polls until approved/rejected/expired. An approved response must contain `device_uuid`, device token, S3 endpoint/region/bucket/access key/secret, and the admin-approved customer name. Approval is persisted with DPAPI (Windows) or Keychain (macOS) and immediately starts the upload engine. macOS never sends XD detection fields.

Wire contract: `box-rui-cam/BACKUP_SYNC_COMMUNICATION_SPEC.md`.

## Upload engine

- Upload-only: startup scan plus recursive watcher for new/changed files.
- Preserve each relative path at the customer bucket root.
- Never delete a remote object because a local file disappeared.
- Local manifest is keyed to `device_uuid` and stored atomically under the platform app-support `state-v2` directory (see table above).
- Update the manifest only after S3 verifies the successful object size.
- Periodically rescan and heal missing/size-mismatched objects.
- Maximum two concurrent file uploads.

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
7. Report progress and failed paths. Authentication failures require new pairing.

## Build and verification

**Windows (from Mac):** `./build-windows.sh` pushes the branch, builds on Proxmox VM 102 via `build-local.ps1 -NoLaunch`, and copies `backupsynctool.exe` to `dist/windows/`. Target remains `x86_64-win7-windows-msvc`. On the VM: `build-local.ps1`. Validate on Win7 test VM 100 and a modern Windows VM.

**macOS:** `./build-macos.sh` builds, ad-hoc-signs the `.app` once (no Keychain password), and launches `dist/macos/Backup Sync Tool.app`. Real signing only with `--identity=…` / `MACOS_SIGN_IDENTITY`. `--install` copies to `/Applications` via `ditto` (preserves signature). `--no-launch` builds only. `--package` also writes `dist/macos/backupsynctool-macos-{aarch64|x86_64}.tar.gz` (updater asset; implies `--no-launch`). Never `open` the raw binary (opens Terminal / Taskgated SIGKILL).

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

**Release (Mac):** `./release.sh` on a clean tree — bump patch → commit → macOS package + Windows build → tag `vX.Y.Z` → push → upload both assets with `gh`. GitHub Actions may create notes-only release shell; assets come from `release.sh`. Prefer this over legacy `release.ps1` (Windows-only).

| Action | How |
| --- | --- |
| Main window | **Left-click** menubar shield → glance popover **under the status icon** (Cocoa `NSStatusItem` button window frame; status + **Recent uploads** after ~1s + **Open Window…** only). Full titled `NSWindow` via Open Window…. ⌘Q quits |
| Tray menu | Secondary click (or menu) → **Open Backup Sync Tool…** / **Open Logs** / **Quit Backup Sync**. Left click does **not** show the menu (`set_show_menu_on_left_click(false)`). |
| Shortcuts | Minimal `NSApp` main menu: **⌘Q** Quit · **⌘W** Close frontmost (status hides to menubar; pair QR closes; popover dismisses) |
| Logs | Tray menu **Open Logs** |
| Quit | Tray menu **Quit Backup Sync** (also ⌘Q) |
| Daemon only | `backupsynctool --daemon` |

Menubar: left click → popover under icon; tray menu keeps Open / Logs / Quit. Pairing QR remains a separate `NSPanel` (`notify.rs`). Routine notices use `notify_user()` / tray tips — not `NSAlert` action sheets for primary workflow.

Config/state: `~/Library/Application Support/BackupSyncTool/` · Secrets: Keychain `cam.rui.backupsynctool` (see Keychain table above).

Checklist: menubar icon · click → glance popover (uploads after 1s) · Open Window full UI · watch folder · pair QR → sync · drop file uploads · quit/relaunch **no Keychain password prompt** (ad-hoc + `-A`) · restore · login toggle → `~/Library/LaunchAgents/` · daemon when configured · second instance takeover · idle RSS ≤ 20 MB (`ps -o rss= -p $(pgrep -n backupsynctool)`).

Limits: not notarized; release assets `backupsynctool.exe` + `backupsynctool-macos-*.tar.gz` on GitHub Releases.