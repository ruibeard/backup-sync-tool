# Backup Sync Tool — Technical Spec v4

**Locked architecture: Option H — mini-Dropbox** (metadata plane + chunk plane).

Greenfield product. WebDAV, Syncthing, CT 105 hub provisioning, and shared storage passwords are out of scope. Existing prod stacks are ignored; desktops will be replaced by hand later.

## Product decisions (locked 2026-07-20)

| Decision | Choice |
| --- | --- |
| Sync model | Full multi-device live two-way from day one |
| Conflicts | Last-writer-wins (no conflict copies) |
| Metadata host | Laravel (pairing, sync metadata API, admin shelf, revoke) |
| Bytes host | S3-compatible object store via storage driver |
| Default driver | `garage` (interface first; live Proxmox optional while coding) |
| Future drivers | `b2`, `r2`, `minio`, `spaces` — Laravel-side only |
| Version retention | 30 days |
| Browse UI | Laravel file shelf only (no Filestash requirement) |
| Legacy WebDAV | Does not exist for this product |
| Windows | Win7 SP1 x64 through Win11 — hard release requirement |
| macOS | Separate native client; Win7 constraints do not apply to macOS builds |

## Architecture

| Layer | Windows | macOS |
| --- | --- | --- |
| UI | Raw Win32 through `windows-rs` | Native menu bar app; `--daemon` for LaunchAgent |
| Sync engine | In-process Rust sync engine (this repo) | same |
| HTTP | Blocking `ureq` | same |
| Desktop secrets | Device token + chunk-store secret in DPAPI | Keychain (`cam.rui.backupsynctool`) |
| Control / metadata | Laravel at editable `pair_api_base` | same |
| Chunk bytes | Object store endpoint from approval payload | same |

There is no bundled Syncthing, no WebDAV client, no Electron/webview/egui/nwg, no async runtime, and no AWS SDK. XD licence detection remains Windows-only.

### Three systems

| System | Responsibility |
| --- | --- |
| Laravel | Pairing/QR approve, device tokens, revoke, sync metadata (files, revisions, chunks, change cursor), 30-day version history, operator file shelf / backup health |
| Object store | Opaque content-addressed chunk bytes only |
| Desktop | Watch selected folder, chunk/hash, upload/download missing chunks, apply last-writer-wins updates, report status |

`pair_api_base` is Laravel only. The object-store endpoint in the approval payload is never confused with the control-plane URL. Desktop never chooses or exposes the storage vendor; Laravel’s `BACKUP_STORAGE_DRIVER` decides.

```text
[Win/Mac app] --pair / sync metadata / cursor--> [Laravel]
       |                                              |
       | put/get chunks (device-scoped key)           | provision bucket/prefix + keys
       v                                              v
                    [Object store driver]
```

## Data model

### Content-addressed chunks

- Files are split with content-defined chunking (FastCDC or equivalent).
- Each chunk is addressed by SHA-256.
- Object key layout (driver-normalized): `{destination_prefix}/chunks/{sha256[0:2]}/{sha256}`.
- Identical bytes across files/devices store once per destination.

### File revision (metadata, Laravel)

A live file is an ordered list of chunk hashes plus:

- stable `file_id` (UUID; survives renames)
- relative path within the customer destination
- size, mtime (client hint), content sha256 of the full file
- `revision` (monotonic per `file_id`)
- `updated_at` (server time)
- `updated_by_device_uuid`
- `deleted_at` (tombstone when deleted)

### Last-writer-wins

When two devices mutate the same `file_id` (or same path for a new file) concurrently:

1. Laravel accepts the write with the higher server-assigned `revision` / later commit timestamp as authoritative.
2. The losing revision is retained as history for 30 days, then pruned with unreferenced chunks.
3. Desktops do **not** create `.sync-conflict` copies.
4. The losing device replaces its local bytes with the winner on next pull.

Renames update path metadata for the same `file_id`. Deletes set a tombstone and propagate to all devices; tombstones and prior revisions remain recoverable in Laravel for 30 days.

### Destinations and devices

- One `BackupDestination` (customer) owns one object-store prefix/bucket assignment.
- Each approved device receives a distinct device UUID, device token (control/metadata auth), and chunk-store credentials scoped to that destination.
- Revoke: mark device revoked, invalidate device token, delete/disable that device’s chunk-store key. Do not delete customer files or other devices’ keys.
- Re-pair of the same machine creates a new device row/token/key and revokes the previous active row for that machine when policy says so.

## Configuration

Only `schema_version: 4` is accepted as paired. Any v3 Syncthing, v2 S3, WebDAV, or older config may keep watch folder / `pair_api_base` hints but requires fresh pairing.

```json
{
  "schema_version": 4,
  "pair_api_base": "https://backup.rui.cam",
  "watch_folder": "C:\\XDSoftware\\backups",
  "device_token_enc": "DPAPI-or-keychain-handle",
  "device_uuid": "desktop-uuid",
  "destination_uuid": "customer-destination-uuid",
  "transport": "chunk_store",
  "chunk_endpoint": "https://s3.example",
  "chunk_region": "garage",
  "chunk_bucket": "backup-…",
  "chunk_prefix": "dest/…/",
  "chunk_access_key_enc": "…",
  "chunk_secret_key_enc": "…",
  "chunk_path_style": true,
  "server_approved_at": "1784050000",
  "start_with_windows": true,
  "auto_update": true
}
```

Paths:

| State | Windows | macOS |
| --- | --- | --- |
| Desktop config | beside `backupsynctool.exe` | `~/Library/Application Support/BackupSyncTool/backupsynctool.json` |
| Local sync DB | `%LOCALAPPDATA%\BackupSyncTool\sync\` | `~/Library/Application Support/BackupSyncTool/sync/` |
| Logs | `logs\` beside executable | `~/Library/Application Support/BackupSyncTool/logs` |

On macOS, secret fields are Keychain handles; ad-hoc dev signing must not prompt for a Keychain password. On Windows, DPAPI uses the established application entropy (`webdavsync-v1`). Never log device tokens or chunk-store secrets.

## Pairing contract

`POST /api/pair/start`:

```json
{
  "machine_name": "RECEPTION-PC",
  "windows_user": "office",
  "app_version": "2026.2.0",
  "detected_install_path": "C:\\XDSoftware",
  "detected_backup_path": "C:\\XDSoftware\\backups",
  "xd_license_number": "XDPT.59655",
  "xd_customer_name": "Palmeira Minimercado",
  "suggested_customer": "XDPT.59655-Palmeira-Minimercado",
  "supported_transports": ["chunk_store"]
}
```

`machine_name` and `supported_transports: ["chunk_store"]` are required. Detected values are untrusted display hints.

Response includes `code`, `approve_url` (QR target), `poll_token`, `poll_interval_ms`, and `control_plane_url` (`APP_URL`, no trailing slash). Desktop logs `control_plane_url mismatch` if it disagrees with configured `pair_api_base`.

Admin approval selects/creates a `BackupDestination`, provisions chunk-store access for the new device, then returns once via poll:

```json
{
  "status": "approved",
  "transport": "chunk_store",
  "device_uuid": "desktop-uuid",
  "device_token": "one-time-device-token",
  "destination_uuid": "customer-destination-uuid",
  "destination_label": "XDPT.59655-Palmeira-Minimercado",
  "chunk_endpoint": "https://s3.example",
  "chunk_region": "garage",
  "chunk_bucket": "backup-…",
  "chunk_prefix": "dest/…/",
  "chunk_access_key": "…",
  "chunk_secret_key": "…",
  "chunk_path_style": true
}
```

Client rejects any transport other than `chunk_store`, missing fields, or invalid URLs. It stores secrets, atomically writes schema v4, and starts the sync engine. Failed/cancelled/rejected pairing must not replace an active assignment. Laravel retains chunk access-key ids for revoke and must not keep chunk secrets after handoff.

Default `pair_api_base` = `https://backup.rui.cam` (editable + persisted: Windows **CONTROL PLANE URL** on blur + pair; macOS tray **Control plane URL…**).

## Sync protocol (desktop ↔ Laravel metadata)

Authenticated with `Authorization: Bearer <device_token>`. Revoked tokens receive `401` and the desktop shows reconnect/re-pair.

Minimum surface (names may be refined in Laravel; behavior is normative):

| Call | Purpose |
| --- | --- |
| `GET /api/sync/cursor` | Current destination change cursor / generation |
| `GET /api/sync/changes?since=` | Metadata changes since cursor (upserts, renames, tombstones) |
| `POST /api/sync/commit` | Propose file revision: path, `file_id`, chunk hash list, size, content hash, client mtime, base revision |
| `POST /api/sync/chunks/present` | Ask which chunk hashes the store already has |
| `POST /api/sync/restore` (admin/desktop optional) | Materialize a historical revision as the new live tip (LWW commit) |

Chunk bytes go **only** to the object store with the device chunk credentials (PUT/GET). Laravel may use a scanner/admin key to verify presence and serve the shelf; it does not proxy bulk desktop transfers.

### Desktop sync loop

On launch (if paired), after approval, and after watch-path save:

1. Ensure local sync DB exists.
2. Scan / watch the selected folder.
3. For local changes: chunk → `chunks/present` → upload missing chunks → `sync/commit`.
4. Pull `sync/changes` and apply remote revisions (download missing chunks, write files, apply deletes/renames).
5. Persist cursor. Retry with backoff on offline; offline is not credential failure.
6. UI states: unpaired, pairing, syncing, idle, offline, auth/revoke error, hard failure.

All approved devices may create, edit, rename, and delete. There is no `can_delete_files` flag.

## Laravel operator surface

- Pairing approve/deny with QR/`approve_url`.
- Device list + revoke.
- Destination list and per-customer shelf (browse live tree + 30-day history).
- Backup health derived from metadata (last activity, file counts, stale devices) — not from Syncthing events.
- Storage driver configured only in Laravel env (`BACKUP_STORAGE_DRIVER` + driver secrets).

## Storage drivers

Laravel binds a `DeviceStorageProvisioner`:

| Driver | Role |
| --- | --- |
| `garage` | Default self-hosted S3-compatible; Admin API creates bucket/key/allow/delete |
| `b2` / `r2` / `minio` / `spaces` | Same approve/revoke semantics when wired |

Desktop speaks a single chunk-store profile from approval. Adding a vendor is a Laravel provisioner change, not a desktop settings change.

While Garage/Proxmox is unavailable, implementation may use local MinIO/Garage fixtures or fakes for tests; the wire contract stays the same.

## Build and release

Only `./build-macos.sh`, `.\build-windows.ps1`, and `./release.sh`.

| Script | Contract |
| --- | --- |
| `./build-macos.sh` | Build/sign/package macOS app; launch unless `--no-launch` |
| `.\build-windows.ps1` | Build Win7-compatible desktop into `dist\windows\`; `-NoLaunch` skips run |
| `./release.sh` | Requires staged Windows dist; bump/tag/upload without moving tags |

Never launch from `target/debug` or `target/release`. Auto-update replaces the whole tested desktop bundle (no separate engine binary once Syncthing is gone).

Windows 7 is a release blocker for Windows artifacts. macOS build/signing is independent.

### Operator smoke

1. Laravel `APP_URL` matches desktop Control plane URL.
2. Pair Win7, current Windows, and macOS to one disposable destination.
3. Verify two-way creates, edits, renames, offline edits, and deletes from every device.
4. Concurrent edit → last-writer-wins; loser converges to winner; loser revision visible in 30-day history.
5. Revoke one device → uploads/metadata calls fail; other devices and data remain.
6. Laravel shelf shows files and recent activity without SSH/Filestash.
7. No Syncthing process, no public chunk-store admin API exposure beyond intended S3 endpoint.

## Implementation order

1. Lock this spec (done).
2. Laravel greenfield: destinations, devices, pairing for `chunk_store`, storage provisioner interface, sync metadata tables + API, 30-day prune job, basic shelf.
3. Desktop: remove Syncthing supervisor path; schema v4; chunker + sync loop + ureq to metadata/chunk store.
4. Win7 + macOS packaged builds and smoke.
5. Optional: switch `BACKUP_STORAGE_DRIVER` to managed B2/R2 without desktop changes.

## Out of scope

- WebDAV and shared folder passwords
- Syncthing / CT 105 / sync provisioner
- Filestash as a product dependency
- Migrating old WebDAV or Garage customer data automatically
- Conflict-copy UX
- Per-device deletion permissions
- Desktop storage-vendor picker
