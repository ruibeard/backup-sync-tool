# Licence Inspector

Small helper EXE that reads the local XD licence and derives the default remote folder name.

## Source

The C# project lives in `license-inspector/` and is built to `license-inspector.exe` in the repo root.

```powershell
dotnet publish .\license-inspector\license-inspector.csproj -c Release -r win-x64 --self-contained true -p:PublishAot=true -p:PublishSingleFile=false -p:DebugType=None -p:DebugSymbols=false -o .
```

## Modes

### 1. Machine-readable (used by the Rust app)

```powershell
& 'license-inspector.exe' --remote-folder
```

Output:

```text
XDPT.59655-Palmeira-Minimercado
```

### 2. Human-readable details (default)

Running without arguments prints all available licence fields, with the most useful fields first.

```powershell
& 'license-inspector.exe'
```

### 3. JSON output

```powershell
& 'license-inspector.exe' --json
```

## What it does

1. Reads `C:\XDSoftware\cfg\xd.lic` as JSON.
2. Reads `C:\XDSoftware\cfg\xd.pem`.
3. Decrypts available fields directly.
4. Builds the default remote folder as `Number + "-" + slugified(ComercialName)`.

## Relevant local XD paths

```text
C:\XDSoftware\cfg\xd.lic
C:\XDSoftware\bin\xd\XDPeople.NET.dll
C:\XDSoftware\backups
```

## App integration

- The Rust app checks for `C:\XDSoftware\backups` and uses it as the local watch folder if it exists and no folder is saved yet.
- The Rust app invokes `license-inspector.exe --remote-folder` when `remote_folder` is empty.
- If detection succeeds, it prefills the remote folder automatically.

## Notes

- The helper EXE is published with NativeAOT and does not need the .NET runtime on the target machine.
- It does not load `XDPeople.NET.dll`.
