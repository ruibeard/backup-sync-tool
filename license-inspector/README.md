# License Inspector

Small local utility that reads the installed XD licence and derives the default remote folder name.

Build:

```powershell
dotnet publish .\license-inspector\license-inspector.csproj -c Release -r win-x64 --self-contained true -p:PublishAot=true -p:PublishSingleFile=false -p:DebugType=None -p:DebugSymbols=false -o .
```

Usage:

| Flag | Description |
|------|-------------|
| *(none)* | Print all available licence fields |
| `--json` | JSON output with all available licence fields |
| `--remote-folder` | Machine-readable folder name only |
| `--license <path>` | Licence file (default `C:\XDSoftware\cfg\xd.lic`) |
| `--xd-dir <path>` | XD binaries (default `C:\XDSoftware\bin\xd`) |
| `--pem <path>` | Public key file (default derived from XD/config paths) |

How it works:

The inspector reads `xd.lic` as JSON, reads `xd.pem`, decrypts available fields directly, and builds the folder slug from `Number` plus `ClientComercialName`. It does not load `XDPeople.NET.dll`.
