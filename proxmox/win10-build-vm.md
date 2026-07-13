# Win10 build VM (Proxmox) — compile host only

Compiles `backupsynctool.exe`. **App target stays Win7** (`x86_64-win7-windows-msvc` via `build-local.ps1`). This VM is not Garage and not Laravel.

## Identity

| Item | Value |
| --- | --- |
| Host | `root@192.168.0.46` (`balaco`) |
| VM | **102** / `win10-build` / guest `10.10.10.68` on `vmbr1` |
| Snapshot | `post-build-tools` (Git + VS 2022 Build Tools + rustup) |
| Win7 **test** VM | **100** — run exe there; do not compile there |

## Build

From a clean Mac/Linux checkout of this branch:

```bash
./build-windows.sh
```

That script: `git push` → SSH `root@192.168.0.46` → VM **102** `git pull` → `.\build-local.ps1 -NoLaunch` → polls until `build-exitcode.txt` → pulls `backupsynctool.exe` into `dist/windows/`.

Lower-level (build only, no fetch): `./push-build-win10.sh`.

Manual on the guest:

```powershell
cd C:\Users\user\code\backup-sync-tool
git fetch
git checkout s3-multipart-implementation
git pull
.\build-local.ps1
```

Must use Win7 target inside that script. Config beside root `backupsynctool.exe`.

After build: pair against `https://backup.rui.cam`, expect schema v2 approval with `s3_endpoint: https://s3.rui.cam` and `s3_region: garage`.

## Remote (optional)

```bash
ssh root@192.168.0.46
qm agent 102 ping
qm guest exec 102 -- powershell -NoProfile -Command "cd C:\Users\user\code\backup-sync-tool; .\build-local.ps1"
```

Bootstrap (new VM): `scripts/win10-build-bootstrap.ps1`.

## Other systems

- Pairing: `backup.rui.cam` (Laravel)
- Storage: `s3.rui.cam` (Garage) — host notes are in `proxmox/garage/README.md`
