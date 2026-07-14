# Backup Sync Tool

Native Win/Mac clients that upload a local backup folder directly to Garage S3 after an administrator approves the device in Laravel.

**Control plane URL** (`pair_api_base`) = which Laravel to pair with. Must match that install’s `APP_URL`. Default `https://backup.rui.cam`; editable + persisted. Storage (`s3_*`) comes from approve only — this app does not pick the storage provider.

## YOU DO — operator smoke

1. Set Laravel `APP_URL` to the public control-plane URL.
2. **Windows:** `./build-windows.sh` → run `dist/windows/backupsynctool.exe` → set **CONTROL PLANE URL** to that `APP_URL` (saves on blur and on pair) → pair → QR/status shows that server → approve → upload. Report failures.
3. **Mac:** `./build-macos.sh` → tray **Control plane URL…** → same `APP_URL` → pair → confirm. Report failures.
4. Log line `control_plane_url mismatch` means desktop URL and Laravel `APP_URL` disagree — fix one.

## Build

Three scripts (clean tree for Windows/release):

```bash
./build-macos.sh                 # .app + launch
./build-macos.sh --package       # updater tarball
./build-windows.sh               # → dist/windows/backupsynctool.exe (via Proxmox VM 102)
./release.sh                     # bump, both platforms, tag, GitHub
```

Mac flags: `--install` `/Applications`, `--no-launch`, `--identity=…` (default ad-hoc). Windows details: [proxmox/win10-build-vm.md](proxmox/win10-build-vm.md).

| Platform | UI | Secrets |
| --- | --- | --- |
| Windows 7–11 | Win32 tray app | DPAPI |
| macOS | Menu bar app (+ `--daemon`) | Keychain |

## Workflow

1. **Windows:** optional XD paths under `C:\XDSoftware\…` as pairing hints. **macOS:** user always chooses the watch folder (no XD).
2. Set **Control plane URL** / `pair_api_base` to the Laravel `APP_URL` (shown during pair).
3. An authenticated admin assigns a customer.
4. Laravel returns a one-time Garage key scoped to that customer bucket (approve only).
5. The app uploads directly to the approve `s3_endpoint`; Laravel never handles backup bytes.

Uploads are one-way and local deletions are not propagated. **Restore** downloads the complete approved customer bucket into a new, non-overwriting restore directory.

## Storage and safety

- One Garage bucket per customer; multiple approved devices intentionally share its object namespace.
- One revocable Garage key per device.
- Secrets: DPAPI (Windows) or Keychain with open ACL on macOS (`-A`; see [SPEC.md](SPEC.md)).
- Config schema is v2. Legacy WebDAV configurations require fresh pairing.
- Small files stream through PutObject; large files use persistent resumable multipart state under the platform app-support directory.
- The upload manifest lives outside the watched folder (see [SPEC.md](SPEC.md)).

Icon masters: `assets/originals/*.svg`. After SVG edits: `python3 assets/render-icons.py`. Details: [SPEC.md](SPEC.md).
