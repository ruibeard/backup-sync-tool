# Win10 build VM (Proxmox) — compile host only

Used by `./build-windows.sh`. App target stays Win7 (`x86_64-win7-windows-msvc`). Not Garage / not Laravel.

| Item | Value |
| --- | --- |
| Host | `root@192.168.0.46` (`balaco`) |
| VM | **102** / `win10-build` / `10.10.10.68` on `vmbr1` |
| Snapshot | `post-build-tools` |
| Win7 test | VM **100** (run exe; do not compile) |

```bash
./build-windows.sh   # push → pull on 102 → Win7 build → dist/windows/backupsynctool.exe
```

Bootstrap: `scripts/win10-build-bootstrap.ps1`. Pairing `backup.rui.cam` · storage `s3.rui.cam`.
