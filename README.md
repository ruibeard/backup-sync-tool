# Backup Sync Tool

Native Windows and macOS clients for a small self-hosted Dropbox: live two-way folder sync with QR pairing, per-device credentials, revoke, and a Laravel admin shelf.

Laravel is the control and metadata plane (pairing, file revisions, 30-day history, browse/health). File bytes are stored whole, under their real name and path, in an S3-compatible object store (bucket versioning keeps history). Desktops hold no store keys: Laravel signs short-lived file URLs. The desktop never picks the storage vendor. Conflicts are last-writer-wins.

Technical contract: [SPEC.md](SPEC.md) (schema v5, roadmap and status).

## Operator smoke

1. Set Laravel `APP_URL` to the public control-plane URL.
2. Windows: `.\build-windows.ps1`, set **CONTROL PLANE URL** to that `APP_URL`, select the folder, pair, approve, confirm two-way sync.
3. macOS: `./build-macos.sh`, set tray **Control plane URL…** to the same `APP_URL`, pair, approve, confirm sync.
4. Confirm the Laravel shelf sees files; revoke a device and confirm it can no longer sync.
5. A `control_plane_url mismatch` log means the desktop URL and Laravel `APP_URL` disagree.

## Build

```bash
./build-macos.sh              # .app + launch
./build-macos.sh --package    # updater archive
./release.sh                  # requires the Windows distribution first
```

```powershell
.\build-windows.ps1
.\build-windows.ps1 -NoLaunch
```

Two-device sync test (needs `rclone`, `php`, `jq` and `../box-rui-cam`; no Docker):

```bash
./dev/e2e/two-device-sync.sh
```

| Platform | UI | Protected secrets |
| --- | --- | --- |
| Windows 7–11 | Native Win32 tray app | Device token via DPAPI |
| macOS | Native menu bar app / daemon | Device token via Keychain |

Configuration schema is v5. Older configs require fresh pairing.
