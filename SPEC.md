# Backup Sync Tool — Technical Spec v2

## Architecture

| Layer | Implementation |
| --- | --- |
| UI | Raw Win32 through `windows-rs` |
| HTTP | Blocking `ureq` |
| S3 request construction | `rusty_s3` Sans-I/O actions |
| Watcher | `notify` |
| Secrets | Windows DPAPI |
| Control plane | `https://backup.rui.cam` |
| Object storage | Garage at `https://s3.rui.cam` |

The client supports Windows 7 SP1 x64 through Windows 11. It contains no WebDAV compatibility, async runtime, AWS SDK, embedded browser, or data-migration logic.

## Configuration

`backupsynctool.json` sits next to the executable. Only `schema_version: 2` with `transport: "s3"` is accepted as paired configuration. Everything else starts unpaired.

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

## XD detection and pairing

XD detection is optional and checks only:

- `C:\XDSoftware`
- `C:\XDSoftware\backups`
- `C:\XDSoftware\cfg\xd.lic`
- `C:\XDSoftware\cfg\xd.pem`

The app decrypts `Number` and `ClientComercialName` and sends them separately with the detected install/backup paths and suggested customer label. A manually chosen folder does not pretend to be an XD installation. Pairing remains available when detection fails.

The QR popup opens immediately, then displays the Laravel approval URL and code. The client polls until approved/rejected/expired. An approved response must contain `device_uuid`, device token, S3 endpoint/region/bucket/access key/secret, and the admin-approved customer name. Approval is persisted with DPAPI and immediately starts the upload engine.

Wire contract: `box-rui-cam/BACKUP_SYNC_COMMUNICATION_SPEC.md`.

## Upload engine

- Upload-only: startup scan plus recursive watcher for new/changed files.
- Preserve each relative path at the customer bucket root.
- Never delete a remote object because a local file disappeared.
- Local manifest is keyed to `device_uuid` and stored atomically under `%LOCALAPPDATA%\BackupSyncTool\state-v2`.
- Update the manifest only after S3 verifies the successful object size.
- Periodically rescan and heal missing/size-mismatched objects.
- Maximum two concurrent file uploads.

Files at or below `s3_part_size_mib` use streamed PutObject. Larger files use persistent multipart:

- State under `%LOCALAPPDATA%\BackupSyncTool\multipart-v1` records source identity, upload ID, completed part number/size/ETag/digest, and phase.
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

Use `build-local.ps1` on Windows VM 102. It builds `x86_64-win7-windows-msvc`, checks forbidden Windows 8+ imports, copies the executable to the repository root, and launches that copy. Validate release builds on both the Win7 test VM and a modern Windows VM.
