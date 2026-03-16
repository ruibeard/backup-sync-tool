# WebDavSync

Minimal native Windows tray app for one-way syncing a local folder to a WebDAV server.

## Scope

- Portable single `WebDavSync.exe`
- Config stored in `config.json` next to the exe
- DPAPI-protected password storage
- One-way sync from one local folder to one WebDAV URL
- Optional remote delete mirroring

## Build

```powershell
cmake -S . -B build -A Win32
cmake --build build --config Release
```

The output binary is `build/Release/WebDavSync.exe`.

## Runtime files

On first save, the app writes:

- `config.json`
- `logs/YYYY-MM-DD.log`

## Notes

- This first cut uses periodic scanning instead of filesystem event hooks.
- Passwords are protected with Windows DPAPI, not hashed.
- The uploader currently uses basic-auth WebDAV requests over WinHTTP.
