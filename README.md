# Backup Sync Tool

Native clients that upload a local backup folder directly to Garage S3 after an administrator approves the device in Laravel.

| Platform | UI | Secrets |
| --- | --- | --- |
| Windows 7–11 | Win32 tray app | DPAPI |
| macOS | Menu bar app (+ `--daemon`) | Keychain |

## Workflow

1. **Windows:** optional XD paths under `C:\XDSoftware\…` as pairing hints. **macOS:** user always chooses the watch folder (no XD).
2. Pairing opens an approve URL / code for `backup.rui.cam`.
3. An authenticated admin assigns a customer.
4. Laravel returns a one-time Garage key scoped to that customer bucket.
5. The app uploads directly to `s3.rui.cam`; Laravel never handles backup bytes.

Uploads are one-way and local deletions are not propagated. **Restore** downloads the complete approved customer bucket into a new, non-overwriting restore directory.

## Storage and safety

- One Garage bucket per customer; multiple approved devices intentionally share its object namespace.
- One revocable Garage key per device.
- Secrets: DPAPI (Windows) or Keychain (macOS).
- Config schema is v2. Legacy WebDAV configurations require fresh pairing.
- Small files stream through PutObject; large files use persistent resumable multipart state under the platform app-support directory.
- The upload manifest lives outside the watched folder (see [SPEC.md](SPEC.md)).

## Build

**Windows** (VM 102):

```powershell
.\build-local.ps1
```

Target: `x86_64-win7-windows-msvc`. Launch root `backupsynctool.exe` only.

**macOS** (this machine / worktree):

```bash
./build-macos.sh            # build + launch .app
./build-macos.sh --install  # also → /Applications, then launch that
./build-macos.sh --no-launch
```

Details + checklist: [SPEC.md](SPEC.md).
