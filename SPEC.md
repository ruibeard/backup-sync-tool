# Backup Sync Tool — Technical Spec v5

**Architecture: live sync** — a small self-hosted Dropbox built from a metadata plane (Laravel) and a file plane (object store). Files are stored whole, under their real name and path.

Greenfield product. WebDAV, Syncthing, CT 105 hub provisioning, and shared storage passwords are out of scope. Existing prod stacks are ignored; desktops will be replaced by hand later.

Branches: `main` (both repos) is the legacy WebDAV production build and stays untouched. This product lives on `live-sync` in `backup-sync-tool` and `box-rui-cam`. `box-rui-cam` `live-sync` deploys to production (`backup.rui.cam`) through Forge, so every push there is a production deploy.

## Product decisions (locked 2026-07-20)

| Decision | Choice |
| --- | --- |
| Sync model | Full multi-device live two-way from day one |
| Conflicts | Last-writer-wins (no conflict copies) |
| Metadata host | Laravel (pairing, sync metadata API, admin shelf, revoke) |
| Bytes host | One S3-compatible bucket: DigitalOcean Spaces in production, any S3 server (rclone) locally |
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
| Desktop secrets | Device token in DPAPI (no store keys) | Keychain (`cam.rui.backupsynctool`) |
| Control / metadata | Laravel at editable `pair_api_base` | same |
| File bytes | Signed URLs from Laravel, straight to the object store | same |

There is no bundled Syncthing, no WebDAV client, no Electron/webview/egui/nwg, no async runtime, and no AWS SDK. XD licence detection remains Windows-only.

### Three systems

| System | Responsibility |
| --- | --- |
| Laravel | Pairing/QR approve, device tokens, revoke, sync metadata (files, revisions, change cursor), 30-day version history, operator file shelf / backup health |
| Object store | Whole files under their real name and path; bucket versioning keeps history |
| Desktop | Watch selected folder, hash, upload changed files, download remote changes, apply last-writer-wins updates, report status |

`pair_api_base` is Laravel only. Desktop never chooses or exposes the storage vendor; Laravel’s `SPACES_*` env decides.

```text
[Win/Mac app] --pair / sync metadata / signed file URLs--> [Laravel]
       |                                                        |
       | PUT/GET files with signed URLs (no device key)        | one store key: signing, shelf, history
       v                                                        v
                     [S3 bucket: {customer}/{path}]
```

## Data model

### Whole files by real name

- Each file is one object. No chunking, no hash-named objects.
- Object key layout: `{bucket}/{destination.name}/{relative/path}` (forward slashes, no leading slash, no `..`), for example `box.rui.cam/ruis-macbook-pro-10/Invoices/2026/inv-001.pdf`. The bucket can be browsed, copied and recovered by hand.
- History is S3 bucket versioning. Each PUT makes a new object version, returned in the `x-amz-version-id` response header and stored as the revision's `version_id`. A store without versioning (local rclone e2e) returns no header: `version_id` is null and the latest object is used.
- One signed PUT carries the whole file (max 5 GiB). The desktop skips bigger files with a log line.

### File revision (metadata, Laravel)

A live file is one object plus:

- stable `file_id` (UUID; survives renames)
- relative path within the customer destination
- size, mtime (client hint), content sha256 of the full file, `version_id` (object version, nullable)
- `revision` (monotonic per `file_id`)
- `updated_at` (server time)
- `updated_by_device_uuid`
- `deleted_at` (tombstone when deleted)

### Last-writer-wins

When two devices mutate the same `file_id` (or same path for a new file) concurrently:

1. Laravel accepts the write with the higher server-assigned `revision` / later commit timestamp as authoritative.
2. The losing revision is retained as history for 30 days, then pruned.
3. Desktops do **not** create `.sync-conflict` copies.
4. The losing device replaces its local bytes with the winner on next pull.

Renames update path metadata for the same `file_id`: the desktop uploads the bytes at the new path and commits, and Laravel deletes the old key. Deletes remove the object (a delete marker under versioning) and set a tombstone and propagate to all devices; tombstones and prior revisions remain recoverable in Laravel for 30 days.

### Destinations and devices

- One `BackupDestination` (customer) owns one object-store prefix/bucket assignment.
- Each approved device receives a distinct device UUID and device token. It gets no object-store key.
- Laravel keeps one store key per install. It signs short-lived (1 h) PUT/GET URLs, each for one object key under the destination prefix, for a valid device token.
- Revoke: mark the device revoked. Its token then gets `401`, so it gets no more file URLs. URLs already signed expire within 1 h. Do not delete customer files.
- Re-pair of the same machine creates a new device row/token and revokes the previous active row for that machine.

## Configuration

Only `schema_version: 5` is accepted as paired. Any v4 (store keys on the device), v3 Syncthing, v2 S3, WebDAV, or older config may keep watch folder / `pair_api_base` hints but requires fresh pairing.

```json
{
  "schema_version": 5,
  "pair_api_base": "https://backup.rui.cam",
  "watch_folder": "C:\\XDSoftware\\backups",
  "device_token_enc": "DPAPI-or-keychain-handle",
  "device_uuid": "desktop-uuid",
  "destination_uuid": "customer-destination-uuid",
  "transport": "file_store",
  "destination_label": "XDPT.59655-Palmeira-Minimercado",
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

On macOS, secret fields are Keychain handles; ad-hoc dev signing must not prompt for a Keychain password. On Windows, DPAPI uses the established application entropy (`webdavsync-v1`). Never log device tokens or signed file URLs.

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
  "supported_transports": ["file_store"]
}
```

`machine_name` and `supported_transports: ["file_store"]` are required. Detected values are untrusted display hints.

Response includes `code`, `approve_url` (QR target), `poll_token`, `poll_interval_ms`, and `control_plane_url` (`APP_URL`, no trailing slash). Desktop logs `control_plane_url mismatch` if it disagrees with configured `pair_api_base`.

Admin approval selects/creates a `BackupDestination` and the device, then returns once via poll:

```json
{
  "status": "approved",
  "transport": "file_store",
  "device_uuid": "desktop-uuid",
  "device_token": "one-time-device-token",
  "destination_uuid": "customer-destination-uuid",
  "destination_label": "XDPT.59655-Palmeira-Minimercado"
}
```

Client rejects any transport other than `file_store` or missing fields. It protects the device token, atomically writes schema v6 (a v5 `chunk_store` config upgrades in place, no re-pair), and starts the sync engine. Failed/cancelled/rejected pairing must not replace an active assignment. Laravel keeps only the token hash.

Default `pair_api_base` = `https://backup.rui.cam` (editable + persisted: Windows **CONTROL PLANE URL** on blur + pair; macOS tray **Control plane URL…**).

## Sync protocol (desktop ↔ Laravel metadata)

Authenticated with `Authorization: Bearer <device_token>`. Revoked tokens receive `401` and the desktop shows reconnect/re-pair.

Minimum surface (names may be refined in Laravel; behavior is normative):

| Call | Purpose |
| --- | --- |
| `GET /api/sync/cursor` | Current destination change cursor / generation |
| `GET /api/sync/changes?since=` | Metadata changes since cursor (upserts, renames, tombstones) |
| `POST /api/sync/commit` | Propose file revision: path, `file_id`, size, content hash, `version_id` (from the PUT response), client mtime, base revision |
| `POST /api/sync/commit/batch` | Up to 200 commits in order; `results[i]` is item `i`'s commit or `{error}` (one bad item does not stop the rest) |
| `POST /api/sync/files/upload` | `{files:[{path,size,content_sha256}]}` (1..200) → `{urls, expires_in}`: one signed PUT URL per file, same order |
| `POST /api/sync/files/download` | `{files:[{path,version_id\|null}]}` (1..500) → `{urls, expires_in}`: one signed GET URL per file, same order; with a `version_id` the URL asks for that version |
| `POST /api/sync/restore` (admin/desktop optional) | Server-side CopyObject of an old `version_id` onto the live key, then commit with the new version (422 if the old revision has no version) |

Sync routes are rate-limited per device (`throttle:sync`, 600/min), not per IP: behind the Cloudflare tunnel many devices can share one IP.

File bytes go **only** to the object store, through the signed URLs (SigV4 query auth, `UNSIGNED-PAYLOAD`, 1 h). Laravel does not proxy bulk desktop transfers, so bytes never pass through PHP or Cloudflare. The desktop checks every downloaded file against its SHA-256. `commit`/`commit/batch` side effects: a delete removes the object, and a `file_id` committed under a new path removes the old key. S3 errors there are logged and never fail the commit. A `changes` payload is `{size, content_sha256, version_id, deleted, updated_by_device_uuid}`.

### Desktop sync loop

On launch (if paired), after approval, and after watch-path save:

1. Ensure local sync DB exists.
2. Scan / watch the selected folder. FS events wake the loop at once (after a 0.5 s settle); otherwise it polls `sync/cursor` every 4 s.
3. Push local changes in batches (≤200 files, ~256 MB): stream-hash files (never read whole into memory) → one `files/upload` (returns PUT URLs) → PUT each file streaming from disk with `Content-Length`, 8 parallel workers, and keep `x-amz-version-id` → one `sync/commit/batch` with `version_id`. A file that changed during the upload is not committed and retries next loop. A rename is an upload at the new path plus a commit; Laravel deletes the old key. Local deletes go in `commit/batch` too.
4. When the server cursor is ahead, pull `sync/changes` pages: skip a change when the local file already has its SHA-256; otherwise get GET URLs from `files/download` in groups (≤500 files, ~256 MB) just before use, stream the files in parallel into `.{cursor}.bst-tmp` files in the target folder (the scanner and watcher ignore `*.bst-tmp`), verify the SHA-256, then apply changes in cursor order with an atomic rename over the target. Only the last change per file in a page is applied. An expired URL fails like any transfer error; the next loop signs fresh ones.
5. Only the pull advances the local cursor (a commit's cursor can jump past another device's change). A failing remote change is retried 3 times, then skipped. File PUT/GET retry transient store errors in place.
6. Skip unchanged files by size + nanosecond mtime (whole seconds miss same-size edits made right after a sync).
7. Report status and activity through `AppCommand::EngineStatus` / `Activity`; auth failure sends `EngineFailed` (pair again). Offline is not credential failure.

Remote changes are not long-polled: on PHP-FPM each waiting device would hold a worker for the whole wait. A 4 s poll of the cheap cursor endpoint gives ~2 s average latency.

All approved devices may create, edit, rename, and delete. There is no `can_delete_files` flag.

## Laravel operator surface

- Pairing approve/deny with QR/`approve_url`.
- Device list + revoke.
- Destination list and per-customer shelf (browse live tree + 30-day history).
- Backup health derived from metadata (last activity, file counts, stale devices).
- Object store configured only in Laravel env (`SPACES_*`).

## Object store

One bucket (`SPACES_BUCKET`, made once by hand; dots are fine, URLs are path style). Each customer is `{destination.name}/` inside it, so approval creates no bucket. `SPACES_KEY` signs device file URLs and reads, deletes and restores for the shelf. No per-device keys, so the 200-key account limit does not apply. File history is bucket versioning plus a 30-day expiry of old versions: run `php artisan storage:versioning` once with a key that may change bucket settings.

Locally, `SPACES_ENDPOINT` points at any S3 server (the e2e uses rclone). Desktop only follows signed URLs, so a vendor change is a Laravel env change.

### Storage choice (research 2026-09-28)

| Option | Verdict |
| --- | --- |
| DigitalOcean Spaces | **Launch.** $5/mo incl. 250 GiB + 1 TB egress, then ~$20/TB. One shared bucket, so the 100-bucket account limit does not apply. The 200-key limit does not apply: devices get signed URLs |
| Backblaze B2 | Cheaper managed option (~$6.95/TB) if data grows. S3-compatible, so an env change |
| Own Hetzner dedicated server + ZFS + Garage | Cheapest per TB above ~15–20 TB; operator maintains disks/OS and a second copy |
| Hetzner Object Storage | Rejected for now: 100-bucket cap, 64 KB minimum billable object, ~100 ms small-object latency and NBG1 throttling incidents in 2026 |
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
7. No public object-store admin API exposure beyond the intended S3 endpoint.

## Code layout (desktop)

| Path | Scope |
| --- | --- |
| `src/sync/` | Shared sync engine: signed-URL whole-file transfers, metadata API client, local state, FS watcher |
| `src/pairing.rs`, `src/config.rs`, `src/secret.rs`, `src/updater.rs`, `src/app.rs`, `src/paths.rs`, `src/logs.rs` | Shared core |
| `src/win/` | Windows shell: Win32 UI (`ui.rs` + `ui/` shards), tray, XD licence detection |
| `src/macos/` | macOS shell: menubar, status window, LaunchAgent daemon, `host.rs` sync host |

## Implementation status (2026-09-29)

### Done

- Laravel (`box-rui-cam` `live-sync`): `file_store` pairing, one shared bucket, sync APIs (cursor / changes / files/upload / files/download / commit / restore), last-writer-wins, 30-day prune, shelf with history, restore and download.
- Desktop: schema v6 pairing (Win + Mac), in-process sync engine, whole-file sync by real name and path, FS watcher with mtime/size skip.
- Engine speed: batched `files/upload` + `commit/batch`, 8 parallel streaming file transfers, streamed hashing, skip download when the local hash matches, 4 s cursor poll, status and activity sent to both UIs.
- Signed file URLs: devices hold no store keys; revoke is the token alone. Signer checked against the AWS SigV4 example.
- Two-device e2e without Docker: `dev/e2e/two-device-sync.sh` (rclone S3 server + Laravel on scratch SQLite + `two_device_sync_e2e`); the desktop gets no S3 key. `E2E_API=https://backup.rui.cam` runs the same test against a deployed control plane and its real store: no local stack, and an admin approves the two printed codes into one new customer folder. 262 files + 9 MiB seed in ~3.2 s (debug build, single-threaded PHP dev server).
- Cleanup: Syncthing/WebDAV leftovers, the no-op installation repair feature and dead code removed; Windows code moved to `src/win/`.

### Roadmap (in order)

1. **Whole-file e2e** (local rclone, then production) and turn on bucket versioning on the Space (`php artisan storage:versioning`). Then **Spaces live e2e** against a real Space; then the two-device last-writer-wins smoke and the Win7 packaged smoke (operator). Spaces uses one shared bucket (`SPACES_BUCKET`, made once by hand); each destination is `{destination.name}/` inside it, so approval creates no bucket.
2. **One pairing flow.** Windows uses `start_pairing_cancellable` / `poll_pairing_cancellable`; macOS uses `start_pairing_result` / `poll_pairing_result` and its own status handling. Move both to the cancellable flow and one status mapper in `pairing.rs`.
3. CI job for `dev/e2e/two-device-sync.sh`.
4. Re-pair catch-up: a fresh device replays the whole change log page by page. Add a server snapshot of live tips if large destinations make that slow.

## Out of scope

- WebDAV and shared folder passwords
- Syncthing / CT 105 / sync provisioner
- Filestash as a product dependency
- Migrating old WebDAV or Garage customer data automatically
- Conflict-copy UX
- Per-device deletion permissions
- Desktop storage-vendor picker
