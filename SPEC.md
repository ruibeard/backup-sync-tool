# Backup Sync Tool — Technical Spec

**Architecture: live sync** — a small self-hosted Dropbox built from a control plane (Laravel) and a file plane (object store). Files are stored whole, under their real name and path. The bucket is the file index: Laravel's database holds only devices and pairing requests.

Greenfield product. There is no old data and no old client to keep compatible.

Branches: `main` (both repos) is the legacy WebDAV production build and stays untouched. This product lives on `live-sync` in `backup-sync-tool` and `box-rui-cam`. `box-rui-cam` `live-sync` deploys to production (`backup.rui.cam`) through Forge, so every push there is a production deploy.

## Product decisions (locked 2026-07-20)

| Decision | Choice |
| --- | --- |
| Sync model | Full multi-device live two-way from day one |
| Conflicts | Last-writer-wins (no conflict copies) |
| Control plane | Laravel (pairing, signing, admin file view, revoke) |
| Bytes host | One S3-compatible bucket: DigitalOcean Spaces in production, any S3 server (rclone) locally |
| Version retention | 30 days |
| Browse UI | Laravel customer page only (no Filestash requirement) |
| Windows | Win7 SP1 x64 through Win11 — hard release requirement |
| macOS | Separate native client; Win7 constraints do not apply to macOS builds |

## Architecture

| Layer | Windows | macOS |
| --- | --- | --- |
| UI | Raw Win32 through `windows-rs` | Native menu bar app; `--daemon` for LaunchAgent |
| Sync engine | In-process Rust sync engine (this repo) | same |
| HTTP | Blocking `ureq` | same |
| Desktop secrets | Device token in DPAPI (no store keys) | Keychain (`cam.rui.backupsynctool`) |
| Control plane | Laravel at editable `pair_api_base` | same |
| File bytes | Signed URLs from Laravel, straight to the object store | same |

There is no bundled Syncthing, no WebDAV client, no Electron/webview/egui/nwg, no async runtime, and no AWS SDK. XD licence detection remains Windows-only.

### Three systems

| System | Responsibility |
| --- | --- |
| Laravel | Pairing/QR approve, device tokens, revoke, bucket listing and URL signing, operator file view with 30-day version history. No file records |
| Object store | Whole files under their real name and path; bucket versioning keeps history |
| Desktop | Watch selected folder, list the bucket, compare with its last-synced state, upload/download/delete what changed, last-writer-wins, report status |

`pair_api_base` is Laravel only. Desktop never chooses or exposes the storage vendor; Laravel’s `SPACES_*` env decides.

```text
[Win/Mac app] --pair / list / signed file URLs--> [Laravel]
       |                                                        |
       | PUT/GET files with signed URLs (no device key)        | one store key: listing, signing, history
       v                                                        v
                     [S3 bucket: {customer}/{path}]
```

## Data model

### Whole files by real name

- Each file is one object. No chunking, no hash-named objects.
- Object key layout: `{bucket}/{customer}/{relative/path}` (forward slashes, no leading slash, no `..`), for example `box.rui.cam/ruis-macbook-pro-10/Invoices/2026/inv-001.pdf`. The bucket can be browsed, copied and recovered by hand.
- History is S3 bucket versioning. Each PUT makes a new object version. A store without versioning (local rclone e2e) keeps only the latest object.
- One signed PUT carries the whole file (max 5 GiB). The desktop skips bigger files with a log line.

### What the desktop remembers

Per path, at the last sync: the object's `etag`, and the local file's `size` and `mtime_ns` (JSON under app support, keyed by device UUID). A single-part ETag is the file's MD5. The state says which side changed since. No entry means the device has not seen the file.

### Last-writer-wins

For each path the device compares local file, bucket object and state:

| Local | Bucket | Action |
| --- | --- | --- |
| changed only | same | upload (skipped when the MD5 equals the ETag) |
| same | changed only | download, check MD5 against the ETag |
| changed | changed, or never synced | equal MD5: record it. Otherwise the later write wins: local mtime against the object's LastModified |
| deleted | unchanged | delete the object |
| unchanged | deleted | delete the local file |
| deleted | changed | download it again |
| changed | deleted | upload it again |

An empty listing after a non-empty sync never deletes local files; they upload again. Desktops do **not** create `.sync-conflict` copies. The overwritten side stays in the bucket's version history for 30 days (the local loser is not kept). A rename is a delete plus an upload. A file put into the bucket by hand syncs to every device.

### Customers and devices

- A customer is one folder `{name}/` in the shared bucket. Laravel stores only the name, on each device (`devices.customer`).
- Each approved device receives a distinct device UUID and device token. It gets no object-store key.
- Laravel keeps one store key per install. It signs short-lived (1 h) GET/PUT/DELETE URLs, each for one object key under the customer prefix, for a valid device token.
- Revoke: mark the device revoked. Its token then gets `401`, so it gets no more file URLs. URLs already signed expire within 1 h. Do not delete customer files.
- Re-pair of the same machine creates a new device row/token and revokes the previous active row for that machine.

## Configuration

The desktop is paired when the config has a device token, device UUID and customer.

```json
{
  "pair_api_base": "https://backup.rui.cam",
  "watch_folder": "C:\\XDSoftware\\backups",
  "device_token_enc": "DPAPI-or-keychain-handle",
  "device_uuid": "desktop-uuid",
  "customer": "xdpt-59655-palmeira-minimercado",
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
  "suggested_customer": "XDPT.59655-Palmeira-Minimercado"
}
```

`machine_name` is required. Detected values are untrusted display hints.

Response includes `code`, `approve_url` (QR target), `poll_token`, `poll_interval_ms`, and `control_plane_url` (`APP_URL`, no trailing slash). Desktop logs `control_plane_url mismatch` if it disagrees with configured `pair_api_base`.

Admin approval names the customer folder and creates the device, then returns once via poll:

```json
{
  "status": "approved",
  "device_uuid": "desktop-uuid",
  "device_token": "one-time-device-token",
  "customer": "xdpt-59655-palmeira-minimercado"
}
```

Client rejects missing fields. It protects the device token, atomically writes the config, and starts the sync engine. Failed/cancelled/rejected pairing must not replace an active assignment. Laravel keeps only the token hash.

Default `pair_api_base` = `https://backup.rui.cam` (editable + persisted: Windows **CONTROL PLANE URL** on blur + pair; macOS tray **Control plane URL…**).

## Sync protocol (desktop ↔ Laravel)

Authenticated with `Authorization: Bearer <device_token>`. Revoked tokens receive `401` and the desktop shows reconnect/re-pair.

| Call | Purpose |
| --- | --- |
| `GET /api/sync/files` | Every object under the device's customer folder: `{files:[{path,size,etag,modified}]}` |
| `POST /api/sync/sign` | `{method: GET\|PUT\|DELETE, paths:[…]}` (1..500) → `{urls, expires_in}`: one presigned URL per path, same order |

Sync routes are rate-limited per device (`throttle:sync`, 600/min), not per IP: behind the Cloudflare tunnel many devices can share one IP.

File bytes go **only** to the object store, through the signed URLs (SigV4 query auth, `UNSIGNED-PAYLOAD`, 1 h). Laravel does not proxy bulk desktop transfers, so bytes never pass through PHP or Cloudflare.

### Desktop sync loop

On launch (if paired), after approval, and after watch-path save:

1. Scan / watch the selected folder. FS events wake the loop at once (after a 0.5 s settle); otherwise it lists the bucket every 4 s.
2. Each round: `GET /api/sync/files`. A quiet round (no FS event, listing equal to the state) stops here. Otherwise scan the folder and decide every path (table above). Files present on both sides with a change get an MD5 (streamed, 4 workers).
3. Upload in batches (≤200 files, ~256 MB): one `sign` for PUT URLs, then 8 parallel streaming PUTs with `Content-Length`. The response ETag goes to the state. A file that changed during the upload is not recorded and retries next round.
4. Download in the same batches: one `sign` for GET URLs, 8 parallel streams into `.{name}.bst-tmp` files in the target folder (the scanner and watcher ignore `*.bst-tmp`), MD5 and size check, then an atomic rename. If the user edited the file meanwhile, the download is dropped and retried.
5. Deletes: presigned DELETE for the bucket, plain removal for local files.
6. Skip unchanged files by size + nanosecond mtime.
7. Report status and activity through `AppCommand::EngineStatus` / `Activity`; auth failure sends `EngineFailed` (pair again). Offline is not credential failure. PUT/GET retry transient store errors in place.

All approved devices may create, edit, rename, and delete. There is no `can_delete_files` flag.

## Laravel operator surface

- Pairing approve/deny with QR/`approve_url`.
- Device list + revoke.
- Customer list and per-customer page: bucket files, per-file version history (30 days), download, restore.
- Device last-seen times.
- Object store configured only in Laravel env (`SPACES_*`).

## Object store

One bucket (`SPACES_BUCKET`, made once by hand; dots are fine, URLs are path style). Each customer is `{customer}/` inside it, so approval creates no bucket. `SPACES_KEY` signs device file URLs and lists, reads and restores for the customer page. No per-device keys, so the 200-key account limit does not apply. File history is bucket versioning plus a 30-day expiry of old versions: run `php artisan storage:versioning` once with a key that may change bucket settings.

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
2. Pair Win7, current Windows, and macOS to one disposable customer.
3. Verify two-way creates, edits, renames, offline edits, and deletes from every device.
4. Concurrent edit → last-writer-wins; loser converges to winner; loser version visible in the file's history.
5. Revoke one device → listing and signing calls fail; other devices and data remain.
6. Laravel customer page shows files and history without SSH/Filestash.
7. No public object-store admin API exposure beyond the intended S3 endpoint.

## Code layout (desktop)

| Path | Scope |
| --- | --- |
| `src/sync/` | Shared sync engine: bucket listing compare, signed-URL whole-file transfers, API client, local state, FS watcher |
| `src/pairing.rs`, `src/config.rs`, `src/secret.rs`, `src/updater.rs`, `src/app.rs`, `src/paths.rs`, `src/logs.rs` | Shared core |
| `src/win/` | Windows shell: Win32 UI (`ui.rs` + `ui/` shards), tray, XD licence detection |
| `src/macos/` | macOS shell: menubar, status window, LaunchAgent daemon, `host.rs` sync host |

## Implementation status (2026-09-29)

### Done

- Laravel (`box-rui-cam` `live-sync`): pairing, one shared bucket, `sync/files` and `sync/sign`, customer page with bucket listing, per-file history, download and restore. Tables: `devices`, `pairing_requests` only.
- Desktop: pairing (Win + Mac), in-process sync engine, whole-file sync by real name and path, three-way compare against the bucket listing, FS watcher.
- Signed file URLs: devices hold no store keys; revoke is the token alone. Signer checked against the AWS SigV4 example.
- Two-device e2e without Docker: `dev/e2e/two-device-sync.sh` (rclone S3 server + Laravel on scratch SQLite + `two_device_sync_e2e`). `E2E_API=https://backup.rui.cam` runs the same test against a deployed control plane and its real store: no local stack, and an admin approves the two printed codes into one new customer folder. 262 files + 9 MiB seed in ~1.6 s (debug build, single-threaded PHP dev server).

### Roadmap (in order)

1. **Whole-file e2e** (local rclone, then production) and turn on bucket versioning on the Space (`php artisan storage:versioning`). Then **Spaces live e2e** against a real Space; then the two-device last-writer-wins smoke and the Win7 packaged smoke (operator). Spaces uses one shared bucket (`SPACES_BUCKET`, made once by hand); each customer is `{customer}/` inside it.
2. **One pairing flow.** Windows uses `start_pairing_cancellable` / `poll_pairing_cancellable`; macOS uses `start_pairing_result` / `poll_pairing_result` and its own status handling. Move both to the cancellable flow and one status mapper in `pairing.rs`.
3. CI job for `dev/e2e/two-device-sync.sh`.

## Out of scope

- WebDAV and shared folder passwords
- Syncthing / CT 105 / sync provisioner
- Filestash as a product dependency
- Migrating old WebDAV or Garage customer data automatically
- Conflict-copy UX
- Per-device deletion permissions
- Desktop storage-vendor picker
