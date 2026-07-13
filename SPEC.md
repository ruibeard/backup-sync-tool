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
macOS client: Apple Silicon / Intel Darwin; local builds sign with stable self-signed identity `Backup Sync Tool Dev` (auto-created in login keychain by `scripts/ensure-macos-sign-identity.sh`) so Keychain “Always Allow” sticks across rebuilds. Not notarized in v1; override identity with `MACOS_SIGN_IDENTITY`.

Neither client uses WebDAV, async runtime, AWS SDK, Electron/webview, or data-migration logic. **XD licence detection is Windows-only.**

## Configuration

`backupsynctool.json` sits next to the executable. Only `schema_version: 2` with `transport: "s3"` is accepted as paired configuration. Everything else starts unpaired.

On macOS, `s3_secret_enc` / `device_token_enc` store Keychain handles (`kc1:…`), not DPAPI blobs. `start_with_windows` means **start at login** (LaunchAgent → `backupsynctool --daemon`).

```json
{
  "schema_version": 2,
  "watch_folder": "C:\\XDSoftware\\backups",
  "remote_folder": "Palmeira Minimercado",
  "transport": "s3",
  "s3_endpoint": "https://s3.rui.cam",
  "s3_region": "garage",
  "s3_bucket": "backup-01abc...",
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

**macOS:** `./build-macos.sh` builds, signs with stable identity (not ad-hoc `-`), and launches `dist/macos/Backup Sync Tool.app`. First Keychain access after switching to this identity: click **Always Allow** once. `--install` copies to `/Applications`. `--no-launch` builds only. `--package` also writes `dist/macos/backupsynctool-macos-{aarch64|x86_64}.tar.gz` (updater asset; implies `--no-launch`). Never `open` the raw binary (opens Terminal / Taskgated SIGKILL).

**Release (Mac):** `./release.sh` on a clean tree — bump patch → commit → macOS package + Windows build → tag `vX.Y.Z` → push → upload both assets with `gh`. GitHub Actions may create notes-only release shell; assets come from `release.sh`. Prefer this over legacy `release.ps1` (Windows-only).

| Action | How |
| --- | --- |
| Main window | Menu → Open Backup Sync Tool… (watch / pair / restore / login / update) |
| Logs | Open Logs |
| Daemon only | `backupsynctool --daemon` |

Config/state: `~/Library/Application Support/BackupSyncTool/` · Secrets: Keychain `cam.rui.backupsynctool`.

Checklist: menubar icon · watch folder · pair QR window → sync · drop file uploads · quit/relaunch keeps Keychain · restore · login toggle → `~/Library/LaunchAgents/` · daemon when configured · second instance takeover · idle RSS ≤ 20 MB (`ps -o rss= -p $(pgrep -n backupsynctool)`).

Limits: not notarized; release assets `backupsynctool.exe` + `backupsynctool-macos-*.tar.gz` on GitHub Releases.