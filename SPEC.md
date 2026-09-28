# Backup Sync Tool — Technical Spec v4

**Architecture: live sync** — a small self-hosted Dropbox built from a metadata plane (Laravel) and a chunk plane (object store).

Greenfield product. WebDAV, Syncthing, CT 105 hub provisioning, and shared storage passwords are out of scope. Existing prod stacks are ignored; desktops will be replaced by hand later.

Branches: `main` (both repos) is the legacy WebDAV production build and stays untouched. This product lives on `live-sync` in `backup-sync-tool` and `box-rui-cam`. `box-rui-cam` `main` auto-deploys through Forge, so `live-sync` must not merge there until the operator smoke passes on a separate server.

## Product decisions (locked 2026-07-20)

| Decision | Choice |
| --- | --- |
| Sync model | Full multi-device live two-way from day one |
| Conflicts | Last-writer-wins (no conflict copies) |
| Metadata host | Laravel (pairing, sync metadata API, admin shelf, revoke) |
| Bytes host | S3-compatible object store via storage driver |
| Launch driver | `spaces` (DigitalOcean Spaces; driver already in Laravel) |
| Other drivers | `garage` (self-hosted, larger scale later), `b2` — Laravel-side only; `minio` is a local test harness only |
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
| `POST /api/sync/commit/batch` | Up to 200 commits in order; `results[i]` is item `i`'s commit or `{error}` (one bad item does not stop the rest) |
| `POST /api/sync/chunks/present` | Ask which chunk hashes the store already has |
| `POST /api/sync/restore` (admin/desktop optional) | Materialize a historical revision as the new live tip (LWW commit) |

Sync routes are rate-limited per device (`throttle:sync`, 600/min), not per IP: behind the Cloudflare tunnel many devices can share one IP.

Chunk bytes go **only** to the object store with the device chunk credentials (PUT/GET). Laravel may use a scanner/admin key to verify presence and serve the shelf; it does not proxy bulk desktop transfers.

### Desktop sync loop

On launch (if paired), after approval, and after watch-path save:

1. Ensure local sync DB exists.
2. Scan / watch the selected folder. FS events wake the loop at once (after a 0.5 s settle); otherwise it polls `sync/cursor` every 4 s.
3. Push local changes in batches (≤200 files, ~256 MB): stream-chunk files (never read whole into memory) → one `chunks/present` → upload missing chunks with 8 parallel workers → one `sync/commit/batch`. Local deletes go in `commit/batch` too.
4. When the server cursor is ahead, pull `sync/changes` pages: fetch the needed chunks in parallel into a local cache (reusing matching chunks from the old local copy), then apply changes in cursor order. Only the last change per file in a page is applied.
5. Only the pull advances the local cursor (a commit's cursor can jump past another device's change). A failing remote change is retried 3 times, then skipped. Chunk PUT/GET retry transient store errors in place.
6. Skip unchanged files by size + nanosecond mtime (whole seconds miss same-size edits made right after a sync).
7. Report status and activity through `AppCommand::EngineStatus` / `Activity`; auth failure sends `EngineFailed` (pair again). Offline is not credential failure.

Remote changes are not long-polled: on PHP-FPM each waiting device would hold a worker for the whole wait. A 4 s poll of the cheap cursor endpoint gives ~2 s average latency.

All approved devices may create, edit, rename, and delete. There is no `can_delete_files` flag.

## Laravel operator surface

- Pairing approve/deny with QR/`approve_url`.
- Device list + revoke.
- Destination list and per-customer shelf (browse live tree + 30-day history).
- Backup health derived from metadata (last activity, file counts, stale devices).
- Storage driver configured only in Laravel env (`BACKUP_STORAGE_DRIVER` + driver secrets).

## Storage drivers

Laravel binds a `DeviceStorageProvisioner`:

| Driver | Role |
| --- | --- |
| `spaces` | Launch driver. One bucket per customer; DigitalOcean API mints/deletes a per-bucket key per device |
| `garage` | Self-hosted S3-compatible; Admin API creates bucket/key/allow/delete |
| `minio` | Local dev/e2e harness only (shared root key). Not for production: MinIO community edition is in maintenance mode |
| `b2` | Not wired. Candidate managed driver if Spaces cost grows |

Desktop speaks a single chunk-store profile from approval. Adding a vendor is a Laravel provisioner change, not a desktop settings change.

Tests may use local MinIO/Garage fixtures or fakes; the wire contract stays the same.

### Storage choice (research 2026-09-28)

| Option | Verdict |
| --- | --- |
| DigitalOcean Spaces | **Launch.** Driver exists. $5/mo incl. 250 GiB + 1 TB egress, then ~$20/TB. Limits: 100 buckets, 200 keys per account (ask support to raise; presigned URLs remove the key limit) |
| Backblaze B2 | Cheaper managed option (~$6.95/TB) if data grows. Needs a driver |
| Own Hetzner dedicated server + ZFS + Garage | Cheapest per TB above ~15–20 TB; operator maintains disks/OS and a second copy |
| Hetzner Object Storage | Rejected for now: no API to mint keys, 100-bucket cap, 64 KB minimum billable object, ~100 ms small-object latency and NBG1 throttling incidents in 2026 |
| Hetzner Storage Box | Not a primary store (SFTP/WebDAV, 10 connections per box). Fine as an off-site copy |
| Proxying bytes through Laravel | Rejected: PHP workers and Cloudflare Tunnel become the bottleneck; Cloudflare Free/Pro caps request bodies at 100 MB |


## Build and release

Only `./build-macos.sh`, `.\build-windows.ps1`, and `./release.sh`.

| Script | Contract |
| --- | --- |
| `./build-macos.sh` | Build/sign/package macOS app; launch unless `--no-launch` |
| `.\build-windows.ps1` | Build Win7-compatible desktop into `dist\windows\`; `-NoLaunch` skips run |
| `./release.sh` | Requires staged Windows dist; bump/tag/upload without moving tags |

Never launch from `target/debug` or `target/release`. Auto-update replaces the whole tested desktop bundle (one binary; no separate engine).

Windows 7 is a release blocker for Windows artifacts. macOS build/signing is independent.

### Operator smoke

1. Laravel `APP_URL` matches desktop Control plane URL.
2. Pair Win7, current Windows, and macOS to one disposable destination.
3. Verify two-way creates, edits, renames, offline edits, and deletes from every device.
4. Concurrent edit → last-writer-wins; loser converges to winner; loser revision visible in 30-day history.
5. Revoke one device → uploads/metadata calls fail; other devices and data remain.
6. Laravel shelf shows files and recent activity without SSH/Filestash.
7. No public chunk-store admin API exposure beyond the intended S3 endpoint.

## Code layout (desktop)

| Path | Scope |
| --- | --- |
| `src/sync/` | Shared sync engine: chunker, chunk store client, metadata API client, local state, FS watcher |
| `src/pairing.rs`, `src/config.rs`, `src/secret.rs`, `src/updater.rs`, `src/app.rs`, `src/paths.rs`, `src/logs.rs` | Shared core |
| `src/win/` | Windows shell: Win32 UI (`ui.rs` + `ui/` shards), tray, XD licence detection |
| `src/macos/` | macOS shell: menubar, status window, LaunchAgent daemon, `host.rs` sync host |

## Implementation status (2026-09-28)

### Done

- Laravel (`box-rui-cam` `live-sync`): `chunk_store` pairing + provisioner (fake, Garage, Spaces, MinIO), sync APIs (cursor / changes / chunks/present / commit / restore), last-writer-wins, 30-day prune, shelf with history, restore and download.
- Desktop: schema v4 pairing (Win + Mac), in-process sync engine with SigV4 PUT/GET, FastCDC chunking, FS watcher with mtime/size skip.
- Engine speed: batched `chunks/present` + `commit/batch`, 8 parallel chunk transfers, streamed chunking, local chunk reuse on download, 4 s cursor poll, status and activity sent to both UIs.
- Two-device e2e without Docker: `dev/e2e/two-device-sync.sh` (rclone S3 server + Laravel on scratch SQLite + `two_device_sync_e2e`). 262 files + 9 MiB seed in ~1.9 s (release build, single-threaded PHP dev server).
- Local MinIO e2e: `dev/minio/bootstrap.sh` + `dev/minio/e2e-chunk-roundtrip.sh`.
- Cleanup: Syncthing/WebDAV leftovers, the no-op installation repair feature and dead code removed; Windows code moved to `src/win/`.

### Roadmap (in order)

1. **Presigned chunk URLs.** Laravel keeps one store key and signs short-lived PUT/GET URLs scoped to the destination prefix (`chunks/present` returns PUT URLs for missing chunks; a batch endpoint returns GET URLs). Desktop drops chunk credentials (schema v5). Revoke = disable device token. Bytes never pass through Laravel or Cloudflare.
2. **Spaces live e2e** against a real Space; then the two-device last-writer-wins smoke and the Win7 packaged smoke (operator).
3. **One pairing flow.** Windows uses `start_pairing_cancellable` / `poll_pairing_cancellable`; macOS uses `start_pairing_result` / `poll_pairing_result` and its own status handling. Move both to the cancellable flow and one status mapper in `pairing.rs`.
4. CI job for `dev/e2e/two-device-sync.sh`; then drop the Docker MinIO harness.
5. Re-pair catch-up: a fresh device replays the whole change log page by page. Add a server snapshot of live tips if large destinations make that slow.

## Out of scope

- WebDAV and shared folder passwords
- Syncthing / CT 105 / sync provisioner
- Filestash as a product dependency
- Migrating old WebDAV or Garage customer data automatically
- Conflict-copy UX
- Per-device deletion permissions
- Desktop storage-vendor picker
