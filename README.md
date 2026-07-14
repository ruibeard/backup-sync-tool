# Backup Sync Tool

Native clients that upload a local backup folder directly to Garage S3 after an administrator approves the device in Laravel.

## Build

Three scripts (clean tree for Windows/release):

```bash
./build-macos.sh                 # .app + launch
./build-macos.sh --package       # updater tarball
./build-windows.sh               # → dist/windows/backupsynctool.exe (via Proxmox VM 102)
./release.sh                     # bump, both platforms, tag, GitHub
```

Mac flags: `--install` `/Applications`, `--no-launch`, `--identity=…` (default ad-hoc). Windows details: [proxmox/win10-build-vm.md](proxmox/win10-build-vm.md).

**Smoke:** set Control plane URL to Laravel `APP_URL`, pair, confirm status. Report failures.

| Platform | UI | Secrets |
| --- | --- | --- |
| Windows 7–11 | Win32 tray app | DPAPI |
| macOS | Menu bar app (+ `--daemon`) | Keychain |

## Workflow

1. **Windows:** optional XD paths under `C:\XDSoftware\…` as pairing hints. **macOS:** user always chooses the watch folder (no XD).
2. Pairing talks to the control plane (`pair_api_base`, default `https://backup.rui.cam` — editable + persisted).
3. An authenticated admin assigns a customer.
4. Laravel returns a one-time Garage key scoped to that customer bucket (approve only).
5. The app uploads directly to `s3.rui.cam`; Laravel never handles backup bytes.

Uploads are one-way and local deletions are not propagated. **Restore** downloads the complete approved customer bucket into a new, non-overwriting restore directory.

## Storage and safety

- One Garage bucket per customer; multiple approved devices intentionally share its object namespace.
- One revocable Garage key per device.
- Secrets: DPAPI (Windows) or Keychain with open ACL on macOS (`-A`; see [SPEC.md](SPEC.md)).
- Config schema is v2. Legacy WebDAV configurations require fresh pairing.
- Small files stream through PutObject; large files use persistent resumable multipart state under the platform app-support directory.
- The upload manifest lives outside the watched folder (see [SPEC.md](SPEC.md)).

Icon masters: `assets/originals/*.svg`. After SVG edits: `python3 assets/render-icons.py`. Details: [SPEC.md](SPEC.md).
