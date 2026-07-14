# Backup Sync Tool — Technical Spec v3

## Architecture

| Layer | Windows | macOS |
| --- | --- | --- |
| UI | Raw Win32 through `windows-rs` | Native menu bar app; `--daemon` for LaunchAgent |
| Sync engine | Bundled Syncthing v2.1.1 | Bundled Syncthing v2.1.1 |
| Engine control | Blocking REST/event API on private loopback | same |
| Desktop secrets | Device token in DPAPI | Device token in Keychain (`cam.rui.backupsynctool`) |
| Control plane | Laravel at editable `pair_api_base` | same |
| Always-online peer | CT 105 Syncthing hub | same |

Windows 7 SP1 x64 through Windows 11 is mandatory. Windows builds compile the pinned engine with the repository-pinned legacy-compatible Go toolchain. macOS packages the engine for the supported Intel/Apple Silicon target. Engine self-update and restart are disabled; only a tested Backup Sync Tool release may replace it.

There is no S3, WebDAV, object-storage transport, custom manifest, multipart uploader, lease, tombstone, or per-device deletion policy. Do not add an async runtime, Electron, webview, AWS SDK, or a second UI framework. XD licence detection remains Windows-only.

## Three systems

| System | Responsibility |
| --- | --- |
| Laravel control plane | Pair requests, admin approval, device/customer assignment, CT 105 provisioning |
| Desktop app | Native UI, private engine lifecycle, loopback API, selected folder |
| CT 105 | Always-online Syncthing hub and staggered recovery versions |

`pair_api_base` identifies Laravel only. A direct hub address may later use a domain such as `sync.rui.cam:22000`, but it is returned by approval and is never confused with the control-plane URL. The CT 105 GUI/API port is not public.

## Configuration

Only `schema_version: 3` is accepted as paired. v2 S3 and all older configurations preserve the selected watch folder/control-plane URL where possible but require fresh pairing.

```json
{
  "schema_version": 3,
  "pair_api_base": "https://backup.rui.cam",
  "watch_folder": "C:\\XDSoftware\\backups",
  "device_token_enc": "DPAPI...",
  "device_uuid": "desktop-uuid",
  "syncthing_device_id": "LOCAL-DEVICE-ID",
  "syncthing_hub_device_id": "CT105-DEVICE-ID",
  "syncthing_hub_addresses": ["tcp://sync.rui.cam:22000", "quic://sync.rui.cam:22000"],
  "syncthing_folder_id": "customer-folder-id",
  "syncthing_folder_label": "XDPT.59655-Palmeira-Minimercado",
  "server_approved_at": "1784050000",
  "start_with_windows": true,
  "auto_update": true
}
```

Paths:

| State | Windows | macOS |
| --- | --- | --- |
| Desktop config | beside `backupsynctool.exe` | `~/Library/Application Support/BackupSyncTool/backupsynctool.json` |
| Private engine home | `%LOCALAPPDATA%\BackupSyncTool\syncthing` | `~/Library/Application Support/BackupSyncTool/syncthing` |
| Logs | `logs\` beside executable | `~/Library/Application Support/BackupSyncTool/logs` |

The private engine's API key lives only in `syncthing/config.xml`. It must never enter desktop config, Laravel, logs, or pairing payloads. Desktop startup binds the engine GUI/API to `127.0.0.1:8385`, supplies the API key in `X-API-Key`, and sets a hidden GUI username/password derived from that unexposed key so a local browser cannot administer the engine. The app never opens a browser. A separately installed user Syncthing instance is not modified.

On macOS, `device_token_enc` is a Keychain handle. Ad-hoc development signing must not prompt for a Keychain password. On Windows it is DPAPI ciphertext using the established application entropy. Syncthing's private certificate and key are protected by the user's application-support directory permissions and must persist because they define the device ID.

## Pairing contract

Before starting a pair request, the desktop starts the private engine, waits for `/rest/system/status`, and obtains its certificate-derived `myID`. The device ID persists even when the temporary pairing-time process stops.

`POST /api/pair/start` includes the existing machine/XD hints plus:

```json
{
  "syncthing_device_id": "LOCAL-DEVICE-ID",
  "supported_transports": ["syncthing"]
}
```

Admin approval provisions the local device on the customer's CT 105 folder. Approved polling status must contain:

```json
{
  "status": "approved",
  "transport": "syncthing",
  "device_uuid": "desktop-uuid",
  "device_token": "one-time-visible-token",
  "syncthing_hub_device_id": "CT105-DEVICE-ID",
  "syncthing_hub_addresses": ["tcp://sync.rui.cam:22000"],
  "syncthing_folder_id": "customer-folder-id",
  "syncthing_folder_label": "Customer label"
}
```

The client rejects missing/invalid device IDs, folder IDs, labels, addresses, or a transport other than `syncthing`. It verifies that the running local ID matches the ID sent during pairing, installs the hub device and folder through `/rest/config`, triggers `/rest/db/scan`, atomically saves schema v3, and starts synchronization. Cancellation, rejection, expiry, malformed approval, and failed local activation do not replace an active assignment.

The optional `control_plane_url` in pair start response is compared with configured `pair_api_base`; mismatch is logged as `control_plane_url mismatch`. The UI continues to support **Change Server** and persists the normalized site root before creating a replacement request.

## Syncthing folder contract

Every desktop customer folder is configured as:

- `type: sendreceive`;
- local selected path, one local device, and the approved CT 105 device;
- filesystem watcher enabled plus periodic full rescan;
- approved hub addresses only (`tcp://`, `quic://`, `relay://`, or `dynamic`);
- no introducer or auto-accept behaviour;
- no desktop engine self-update.

Every customer folder on CT 105 is `sendreceive` and uses staggered file versioning. All devices may originate and receive creates, edits, renames, conflicts, and deletions. Syncthing's conflict naming/retention behaviour is authoritative. There is no `can_delete_files` switch and `ignoreDelete` must not be enabled.

CT 105 must keep a complete synchronized copy. Operators recover deleted/replaced files from CT 105 staggered versions. The old whole-customer S3 restore operation does not exist; desktop restore UI must not claim that it downloads an object-store snapshot.

## Engine lifecycle and status

The desktop launches only the bundled engine using the equivalent of:

```text
syncthing serve --home=<private-home> --no-browser --no-restart --no-upgrade --gui-address=127.0.0.1:8385
```

Stdout/stderr are forwarded into application logs. Startup is bounded; an absent binary, early exit, missing API key, REST failure, or identity mismatch is a visible error. Normal app shutdown requests `/rest/system/shutdown`, waits briefly, then kills only the child it owns if necessary.

Before normal pairing/sync startup, both native shells verify that the engine executable and `syncthing-LICENSE.txt` are present (and executable on Unix). Missing bundle files trigger a retryable same-version repair download from the latest GitHub release. This bypasses the usual “latest version equals current version” result so a legacy single-file updater cannot strand a v3 desktop without its engine. Repair does not require pairing: Windows shows a **Retry repair** action, and macOS exposes **Repair Installation…** in the menu bar after a visible failure.

Desktop status is derived from `/rest/db/status`, `/rest/system/connections`, and `/rest/events`. At minimum the UI distinguishes disconnected/reconnect-required, scanning, syncing, idle, and failure, and may show needed files/bytes. The engine owns transfer retries and offline convergence.

## XD detection

Windows optionally checks:

- `C:\XDSoftware`
- `C:\XDSoftware\backups`
- `C:\XDSoftware\cfg\xd.lic`
- `C:\XDSoftware\cfg\xd.pem`

It sends detected licence/customer values only as pairing hints. Manual selection and macOS never pretend to be XD detection. Pairing remains available when detection fails.

## Build and release

Only these entry points are supported: `./build-macos.sh`, `.\build-windows.ps1`, and `./release.sh`.

| Script | Contract |
| --- | --- |
| `./build-macos.sh` | Build release app, copy pinned engine to `Contents/Resources/syncthing`, sign nested executable then app, and package the sealed `.app` as one updater archive; launch unless `--no-launch` |
| `.\build-windows.ps1` | Build Win7 desktop plus pinned Win7-compatible `syncthing.exe`, stage app/engine/license together, emit the updater ZIP, and launch unless `-NoLaunch` |
| `./release.sh` | Require staged Windows desktop and engine, package macOS, bump/tag/upload without moving existing tags |

Never launch from `target/debug` or `target/release`. A build is successful only with zero errors and a running packaged app, or staged executable pair when no-launch was requested.

Operator smoke after relevant builds:

1. Confirm Laravel `APP_URL` and desktop Control plane URL match.
2. Pair Windows 7, current Windows, and macOS with CT 105.
3. Verify initial convergence, edits, concurrent conflicts, renames, offline changes, and deletions from every device.
4. Verify CT 105 staggered-version recovery.
5. Verify quit/relaunch keeps the same local device ID and does not show the engine GUI or a Keychain password prompt.
6. Verify no public API port, no engine self-update, and useful logs for process/API failures.

Windows 7 is a release blocker, not an optional compatibility target.

## Implementation handoff — 2026-07-15

The S3 transport has been replaced in the desktop and Laravel repositories. The desktop now bundles and supervises Syncthing, pairing schema v3 carries Syncthing assignments, Laravel provisions customer/device shares through a narrow external service, and the old object-storage upload, restore, scan, browser, credential, and lease paths have been removed. The macOS package has been built and signature-verified. Rust and targeted Laravel tests pass. A real Windows 7 build and the live CT 105/Laravel smoke test remain operator work.

No production infrastructure was changed. In particular, no Forge environment, CT 105 configuration, DNS, router/firewall rule, old S3 key, or stored S3 data was touched.

### Tomorrow: operator installation sequence

Use these as three different endpoints; never substitute one for another:

| Purpose | Suggested endpoint | Exposure |
| --- | --- | --- |
| Laravel pairing/control plane | `https://backup.rui.cam` | Public HTTPS |
| Syncthing device traffic | `sync.rui.cam:22000` | Public TCP and UDP to CT 105 |
| Narrow share/unshare provisioner | `https://sync-provision.rui.cam` or a private-tunnel URL | Reachable by Laravel only |

Do **not** publish Syncthing GUI/API port `8384`, its REST API key, or the desktop-private port `8385`. Laravel must call the narrow provisioner rather than CT 105's Syncthing API directly.

1. Take a CT 105 configuration/data snapshot and record the existing hub device ID. The expected ID from the previous setup is `XLTL234-AJRMJV6-W3LNSHR-2YXZD7D-AOWWFDQ-NQ3X6HK-AJ6IS5K-B6MWKAH`; confirm it instead of changing it blindly.
2. Create DNS for `sync.rui.cam` pointing to the public address that reaches CT 105. Forward TCP `22000` and UDP `22000` to CT 105. Do not forward `8384` or `8385`.
3. Install the narrow provisioner beside CT 105 or behind a private tunnel. It must implement the authenticated, idempotent `POST /v1/folders/share` and `DELETE /v1/folders/unshare` contract in `box-rui-cam/BACKUP_SYNC_COMMUNICATION_SPEC.md`. It must configure hub folders as `sendreceive` with staggered versioning, and unshare a device without deleting customer files or history. These commits define and consume that contract but do not contain the CT 105 provisioner service itself; if it does not already exist, stop here and implement/deploy it before pairing.
4. Give the provisioner a long random bearer token. Restrict the endpoint by firewall, tunnel, or access policy so only Laravel can reach it. Test unauthorized requests are rejected.
5. In the Laravel operator environment, set `APP_URL=https://backup.rui.cam`, `SYNCTHING_PROVISIONER_ENDPOINT=<provisioner base URL>`, and `SYNCTHING_PROVISIONER_TOKEN=<same token>`. Do not place the Syncthing GUI URL or API key in Laravel.
6. Deploy the Laravel changes using the normal operator process, run the database migrations, clear/rebuild Laravel configuration cache, and confirm the pairing page loads at the public `APP_URL`. The agent must not access Forge or production environment values.
7. On a Windows 7 SP1 x64 test machine with Rust/Visual Studio prerequisites, run `.\build-windows.ps1`. Require zero build errors, a passing forbidden-import audit, and the packaged `dist\windows\backupsynctool.exe`, `syncthing.exe`, `syncthing-LICENSE.txt`, and updater ZIP. Do not release if the Windows 7 build or launch fails.
8. On macOS, run `./build-macos.sh`. Confirm the packaged app launches, no Terminal or Syncthing GUI appears, no Keychain password prompt appears, and `codesign --verify --strict "dist/macos/Backup Sync Tool.app"` succeeds.
9. In each desktop app, set **Control plane URL** to exactly `https://backup.rui.cam`, select a new disposable test folder, start pairing, approve it in Laravel, and verify the returned hub address is `sync.rui.cam:22000` rather than a control-plane or GUI address.
10. Pair at least Windows 7, a current Windows machine, and macOS to the same disposable customer. Confirm all three retain different stable local Syncthing device IDs while sharing the same customer folder ID.
11. Test convergence in both directions: create and edit a file on each device, rename a file, make one device offline and reconnect it, and create a concurrent-edit conflict. Wait for all devices and CT 105 to become idle after each case.
12. Test deletion propagation explicitly. Delete a disposable file on Windows, then a different file on macOS. Confirm both deletions reach every device and CT 105. There is intentionally no per-device deletion permission.
13. From CT 105 staggered versions, restore one deleted file and one earlier file version. Confirm the recovered files synchronize back to every device. Do not use the removed S3 restore workflow.
14. Quit and relaunch every desktop app. Confirm the local device IDs do not change, sync resumes, the engine remains private, and logs contain no `control_plane_url mismatch`.
15. Only after the complete smoke test, run `./release.sh` from macOS with the verified Windows bundle already in `dist/windows/`. This is the only supported release path.
16. Separately revoke the obsolete Garage/S3 access keys and decide the retention/deletion date for old object-storage data. This is irreversible infrastructure work and is intentionally outside these code commits.

If tomorrow stops before completion, record the last successful numbered step and the exact desktop, Laravel, provisioner, or CT 105 log error. Do not work around a failure by exposing port `8384`, enabling Syncthing self-update, changing a device ID, or disabling deletion propagation.
