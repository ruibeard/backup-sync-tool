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
- Secrets: DPAPI (Windows) or Keychain with open ACL on macOS (`-A`; see [SPEC.md](SPEC.md)).
- Config schema is v2. Legacy WebDAV configurations require fresh pairing.
- Small files stream through PutObject; large files use persistent resumable multipart state under the platform app-support directory.
- The upload manifest lives outside the watched folder (see [SPEC.md](SPEC.md)).

## Build

**Windows** (from Mac → Proxmox VM 102):

```bash
./build-windows.sh            # push branch, build on VM, pull exe → dist/windows/
```

On the VM itself: `.\build-local.ps1` (Win7 target `x86_64-win7-windows-msvc`).

**macOS** (this machine):

```bash
./build-macos.sh              # ad-hoc sign + launch .app (no Keychain password)
./build-macos.sh --install    # also → /Applications, then launch that
./build-macos.sh --no-launch  # build only
./build-macos.sh --package    # build + updater tarball (no launch)
```

Icon masters: `assets/originals/*.svg`. Run `python3 assets/render-icons.py` after SVG edits.

## Release

From a clean Mac checkout (ships both assets):

```bash
./release.sh
```

Bumps patch in `Cargo.toml`, builds macOS tarball + Windows exe, tags `vX.Y.Z`, pushes, uploads to GitHub Releases. Details: [SPEC.md](SPEC.md).
