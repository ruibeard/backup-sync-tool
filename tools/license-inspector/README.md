# License Inspector

Small local utility that asks the installed XD code to parse `xd.lic`.

It calls:

```text
XDPeople.Utils.XDLicence.LoadToPreview(...)
```

Default paths:

- XD binaries: `C:\XDSoftware\bin\xd`
- Licence file: `C:\XDSoftware\cfg\xd.lic`

Usage:

```powershell
dotnet run --project tools/license-inspector -- --json
dotnet run --project tools/license-inspector -- --all
dotnet run --project tools/license-inspector -- --remote-folder
dotnet run --project tools/license-inspector -- --license "C:\path\to\xd.lic"
dotnet run --project tools/license-inspector -- --xd-dir "C:\XDSoftware\bin\xd"
```

Notes:

- The tool prefers framework assemblies from the local .NET runtime instead of old `System.*` DLLs shipped with XD.
- `--json` prints every public property from `XDPeople.License.LicenceData`.
- `--remote-folder` prints the derived default remote folder, for example `XDPT.59655-Palmeira-Minimercado`.
- Plain output prints the most useful customer and licence fields first.
