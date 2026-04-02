# Check Licence

This repo now uses a small helper EXE to read the local XD licence and derive the default remote folder name.

Current helper:

```text
C:\Users\rui.almeida\Code\backup-sync-tool\license-inspector.exe
```

Example usage:

```powershell
& 'C:\Users\rui.almeida\Code\backup-sync-tool\license-inspector.exe' --remote-folder
```

Expected output on this machine:

```text
XDPT.59655-Palmeira-Minimercado
```

What the helper is doing:

- It loads `C:\XDSoftware\bin\xd\XDPeople.NET.dll`
- It calls the XD method `XDPeople.Utils.XDLicence.LoadToPreview("C:\XDSoftware\cfg\xd.lic")`
- It reads the returned licence data
- It builds the default remote folder as:
  `LicenceNumber + "-" + ComercialName`

Relevant local XD paths:

```text
C:\XDSoftware\cfg\xd.lic
C:\XDSoftware\bin\xd\XDPeople.NET.dll
C:\XDSoftware\backups
```

What was used before:

- During investigation, the decrypt step was first proven with a temporary C# inspector.
- The actual call inside that temporary tool was:

```csharp
var asm = loadContext.LoadFromAssemblyPath(@"C:\XDSoftware\bin\xd\XDPeople.NET.dll");
var xdLicenceType = asm.GetType("XDPeople.Utils.XDLicence", throwOnError: true)!;
var loadToPreview = xdLicenceType.GetMethod("LoadToPreview", BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static);
var licenceData = loadToPreview.Invoke(null, new object?[] { @"C:\XDSoftware\cfg\xd.lic" });
```

Current app behavior:

- The Rust app checks for `C:\XDSoftware\backups` and uses it as the local watch folder if it exists and no folder is saved yet.
- The Rust app invokes `license-inspector.exe --remote-folder` when `remote_folder` is empty.
- If detection succeeds, it prefills the remote folder automatically.

Notes:

- The helper EXE is intentionally kept as a standalone binary.
- There is no C# project kept in the repo anymore.
- This tiny EXE is framework-dependent, so it needs the .NET runtime on the target machine.