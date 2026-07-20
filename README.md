# Backup Sync Tool

Native Windows and macOS clients for a small self-hosted Dropbox: live two-way folder sync with QR pairing, per-device credentials, revoke, and a Laravel admin shelf.

Laravel is the control and metadata plane (pairing, file revisions, 30-day history, browse/health). File bytes are content-addressed chunks in an S3-compatible object store. The desktop never picks the storage vendor. Conflicts are last-writer-wins.

Technical contract: [SPEC.md](SPEC.md) (architecture **Option H**, schema v4).

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

| Platform | UI | Protected secrets |
| --- | --- | --- |
| Windows 7–11 | Native Win32 tray app | Device token + chunk keys via DPAPI |
| macOS | Native menu bar app / daemon | Device token + chunk keys via Keychain |

Configuration schema is v4. Older Syncthing/S3/WebDAV configs require fresh pairing.
