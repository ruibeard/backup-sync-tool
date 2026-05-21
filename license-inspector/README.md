# License Inspector

Diagnostic NativeAOT helper for XD licence fields and remote-folder slug. 
**The Rust app uses native detection in `src/xd.rs`** — this exe is not required at runtime.

## Build

```powershell
dotnet publish .\license-inspector\license-inspector.csproj -c Release -r win-x64 --self-contained true -p:PublishAot=true -p:PublishSingleFile=false -p:DebugType=None -p:DebugSymbols=false -o .
```

Output: repo-root `license-inspector.exe`.

## Usage

| Flag | Output |
| --- | --- |
| `--remote-folder` | Machine-readable folder only (e.g. `XDPT.59655-Palmeira-Minimercado`) |
| `--json` | All fields as JSON |
| *(none)* | Human-readable fields |
| `--license <path>` | Licence file (default `C:\XDSoftware\cfg\xd.lic`) |
| `--pem <path>` | Public key (default `C:\XDSoftware\cfg\xd.pem`) |

## Algorithm

1. Read `xd.lic` JSON + `xd.pem`
2. Decrypt fields (raw RSA blocks; no `XDPeople.NET.dll`)
3. Folder = `{Number}-{slug(ClientComercialName)}`

Reference source: `license-inspector/Program.cs`. Rust parity test: `src/xd.rs` `native_detection_matches_license_inspector_when_available`.