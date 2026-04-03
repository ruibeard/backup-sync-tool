use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

const XD_ROOT: &str = r"C:\XDSoftware";
const DEFAULT_WATCH_FOLDER: &str = r"C:\XDSoftware\backups";
const XD_DLL_PATH: &str = r"C:\XDSoftware\bin\xd\XDPeople.NET.dll";
const XD_LICENSE_PATH: &str = r"C:\XDSoftware\cfg\xd.lic";
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn default_watch_folder() -> Option<String> {
    let path = Path::new(DEFAULT_WATCH_FOLDER);
    path.is_dir().then(|| path.display().to_string())
}

pub fn detect_default_remote_folder() -> Option<String> {
    if !Path::new(XD_ROOT).is_dir()
        || !Path::new(XD_DLL_PATH).is_file()
        || !Path::new(XD_LICENSE_PATH).is_file()
    {
        return None;
    }

    let output = powershell_detection_command().output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn powershell_detection_command() -> Command {
    let script = format!(
        concat!(
            "$ErrorActionPreference='Stop';",
            "if (-not (Test-Path '{}')) {{ exit 1 }};",
            "if (-not (Test-Path '{}')) {{ exit 1 }};",
            "[AppDomain]::CurrentDomain.add_AssemblyResolve({{",
            "param($sender,$args) ",
            "$name = ([Reflection.AssemblyName]$args.Name).Name; ",
            "$candidate = Join-Path 'C:\\XDSoftware\\bin\\xd' ($name + '.dll'); ",
            "if (Test-Path $candidate) {{ return [Reflection.Assembly]::LoadFrom($candidate) }}; ",
            "return $null",
            "}});",
            "$asm = [Reflection.Assembly]::LoadFrom('{}');",
            "$type = $asm.GetType('XDPeople.Utils.XDLicence', $true);",
            "$method = $type.GetMethod('LoadToPreview', [Reflection.BindingFlags]'Public, NonPublic, Static', $null, [Type[]]@([string]), $null);",
            "if ($null -eq $method) {{ exit 1 }};",
            "$lic = $method.Invoke($null, @('{}'));",
            "if ($null -eq $lic) {{ exit 1 }};",
            "$number = [string]$lic.Number;",
            "$name = [string]$lic.ComercialName;",
            "if ([string]::IsNullOrWhiteSpace($number)) {{ exit 1 }};",
            "$normalized = $name.Normalize([Text.NormalizationForm]::FormD);",
            "$chars = New-Object System.Collections.Generic.List[char];",
            "$previousDash = $false;",
            "foreach ($ch in $normalized.ToCharArray()) {{",
            "  $category = [Globalization.CharUnicodeInfo]::GetUnicodeCategory($ch);",
            "  if ($category -eq [Globalization.UnicodeCategory]::NonSpacingMark) {{ continue }};",
            "  if ([char]::IsLetterOrDigit($ch)) {{ $chars.Add($ch); $previousDash = $false; continue }};",
            "  if (-not $previousDash) {{ $chars.Add('-'); $previousDash = $true }}",
            "}};",
            "$slug = (-join $chars.ToArray()).Trim('-');",
            "if ([string]::IsNullOrWhiteSpace($slug)) {{ Write-Output $number }} else {{ Write-Output ($number + '-' + $slug) }}"
        ),
        XD_DLL_PATH, XD_LICENSE_PATH, XD_DLL_PATH, XD_LICENSE_PATH
    );

    let mut cmd = Command::new("powershell");
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(script)
        .creation_flags(CREATE_NO_WINDOW);
    cmd
}
