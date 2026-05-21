# Capture Backup Sync Tool via Windows PrintWindow (user32 + GDI).
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $PSScriptRoot "CaptureWindow.exe"
$cs = Join-Path $PSScriptRoot "CaptureWindow.cs"
$csc = Join-Path $env:WINDIR "Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$out = if ($args.Count -gt 0) { $args[0] } else { Join-Path $root "layout_h5_verify.png" }

if (-not (Test-Path $exe) -or (Test-Path $cs) -and ($cs.LastWriteTime -gt (Get-Item $exe).LastWriteTime)) {
    & $csc /nologo /reference:System.Drawing.dll /out:$exe $cs | Out-Null
}

& $exe $out
