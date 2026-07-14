# Backup Sync Tool

Native Windows and macOS clients that run a private bundled Syncthing engine and pair it with the always-online CT 105 backup hub.

The Laravel control plane approves devices and customer folders. Backup bytes move directly between Syncthing peers; Laravel never handles file data. Every approved device is `sendreceive`, so creates, edits, renames, conflicts, and deletions propagate in every direction. CT 105 keeps staggered file versions for recovery.

## Operator smoke

1. Set Laravel `APP_URL` to the public control-plane URL.
2. Windows: `.\build-windows.ps1`, set **CONTROL PLANE URL** to that `APP_URL`, select the folder, pair, approve, and confirm CT 105 connects and synchronizes.
3. macOS: `./build-macos.sh`, set tray **Control plane URL…** to the same `APP_URL`, select the folder, pair, approve, and confirm synchronization.
4. A `control_plane_url mismatch` log means the desktop URL and Laravel `APP_URL` disagree.

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

Both builds bundle the project-pinned Syncthing v2.1.1 engine. Windows 7 SP1 x64 remains supported through the pinned legacy-compatible Go toolchain; Syncthing self-update is disabled so the tested engine cannot replace itself.

| Platform | UI | Protected secret |
| --- | --- | --- |
| Windows 7–11 | Native Win32 tray app | Device token via DPAPI |
| macOS | Native menu bar app / daemon | Device token via Keychain |

Configuration schema is v3. Any S3, WebDAV, or earlier configuration requires fresh pairing. The private Syncthing API is loopback-only and its API key stays inside Syncthing's application-support directory.

Technical contract and checklists: [SPEC.md](SPEC.md).
