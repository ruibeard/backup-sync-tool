# Backup Sync Tool

Native Windows 7–11 tray app that uploads a local backup folder directly to Garage S3 storage after an administrator approves the device in Laravel.

## Workflow

1. The app uses `C:\XDSoftware\backups` when available, and reads the XD licence number/name from `C:\XDSoftware\cfg` as an optional pairing hint.
2. **Connect Server** opens the QR/code window.
3. An authenticated admin explicitly assigns an existing customer or creates a new one at `backup.rui.cam`.
4. Laravel returns a one-time Garage key scoped to that customer bucket.
5. The app uploads directly to `s3.rui.cam`; Laravel never handles backup bytes.

Uploads are one-way and local deletions are not propagated. **Restore** downloads the complete approved customer bucket into a new, non-overwriting restore directory selected by the user.

## Storage and safety

- One Garage bucket per customer; multiple approved devices intentionally share its object namespace.
- One revocable Garage key per device.
- S3 secret and device token are DPAPI encrypted.
- Config schema is v2. Legacy WebDAV and experimental MinIO configurations require fresh pairing.
- Small files stream through PutObject; large files use persistent resumable multipart state under `%LOCALAPPDATA%\BackupSyncTool`.
- The upload manifest also lives under `%LOCALAPPDATA%\BackupSyncTool`, outside the watched folder.

## Build

Run on the Windows build VM from the repository root:

```powershell
.\build-local.ps1
```

The only supported target is `x86_64-win7-windows-msvc`. Never launch from `target/debug` or `target/release`; the script copies and launches the root `backupsynctool.exe`.

Technical contract: [SPEC.md](SPEC.md).
