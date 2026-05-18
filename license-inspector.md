# Licence Inspector

Small helper EXE that reads the local XD licence and derives the default remote folder name.

## Source

The C# project lives in `tools/license-inspector/` and is built to `license-inspector.exe` in the repo root.

```powershell
cd tools/license-inspector
dotnet publish -c Release -r win-x64 --self-contained false -p:PublishSingleFile=true -o ..\..
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

### 2. Human-readable summary (default)

Running without arguments prints the most useful fields first.

```powershell
& 'license-inspector.exe'
```

### 3. All fields

```powershell
& 'license-inspector.exe' --all
```

### 4. JSON output

```powershell
& 'license-inspector.exe' --json
```

## What it does

1. Loads `C:\XDSoftware\bin\xd\XDPeople.NET.dll`
2. Calls `XDPeople.Utils.XDLicence.LoadToPreview("C:\XDSoftware\cfg\xd.lic")`
3. Reflects over the returned object to read every property
4. Builds the default remote folder as `Number + "-" + slugified(ComercialName)`

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

- The helper EXE is framework-dependent (.NET 8) and needs the .NET runtime on the target machine.
- `XdLoadContext` is used so transitive XD dependencies are loaded from `C:\XDSoftware\bin\xd\`, while framework assemblies are resolved from the local .NET runtime.
