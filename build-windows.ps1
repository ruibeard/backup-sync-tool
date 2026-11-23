# Build Win7-compatible app (in-process file sync engine) -> root + dist\windows\
param([switch]$NoLaunch)
$ErrorActionPreference = 'Stop'
$env:PATH += ";$env:USERPROFILE\.cargo\bin"
Set-Location $PSScriptRoot

$target = 'x86_64-win7-windows-msvc'
$src = "target\$target\release\backupsynctool.exe"
$outDir = 'dist\windows'

function Stop-BundledProcess([string]$Name, [string[]]$AllowedPaths) {
  Get-Process $Name -ErrorAction SilentlyContinue | ForEach-Object {
    try {
      $processPath = $_.Path
      if ($processPath -and ($AllowedPaths -contains [IO.Path]::GetFullPath($processPath))) {
        Stop-Process -Id $_.Id -Force
      }
    } catch { }
  }
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$rootApp = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'backupsynctool.exe'))
$distApp = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "$outDir\backupsynctool.exe"))
Stop-BundledProcess 'backupsynctool' @($rootApp, $distApp)
Start-Sleep -Milliseconds 500

$ErrorActionPreference = 'Continue'
rustup toolchain install nightly
if ($LASTEXITCODE) { throw 'rustup nightly failed' }
rustup component add rust-src --toolchain nightly
if ($LASTEXITCODE) { throw 'rust-src failed' }
$env:RUSTFLAGS = '-C target-feature=+crt-static --cfg windows_raw_dylib'
cargo +nightly build -Z build-std=std,panic_abort --release --target $target
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $src)) { throw 'cargo build failed' }
$ErrorActionPreference = 'Stop'

$dump = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
if (-not $dump) {
  # Plain shells (remote, release.sh) have no VS Developer PATH; find dumpbin via vswhere.
  $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
  if (Test-Path $vswhere) {
    $vs = & $vswhere -latest -products * -property installationPath
    $found = Get-ChildItem "$vs\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe" -ErrorAction SilentlyContinue | Select-Object -Last 1
    if ($found) { $env:PATH = "$($found.DirectoryName);$env:PATH"; $dump = Get-Command dumpbin.exe -ErrorAction SilentlyContinue }
  }
}
if (-not $dump) { throw 'dumpbin.exe is required for the Win7 import audit; run from a VS Developer PowerShell' }
$forbiddenImports = @(
  'GetSystemTimePreciseAsFileTime',
  'WaitOnAddress',
  'WakeByAddressAll',
  'WakeByAddressSingle',
  'ProcessPrng',
  'GetTempPath2W',
  'SetThreadDescription'
)
$imports = & $dump.Source /imports $src 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { throw "dumpbin import audit failed for $src" }
foreach ($import in $forbiddenImports) {
  if ($imports -match "\b$([regex]::Escape($import))\b") {
    throw "Win7-incompatible import in $src`: $import"
  }
}

Copy-Item $src backupsynctool.exe -Force
Copy-Item $src "$outDir\backupsynctool.exe" -Force
$bundle = "$outDir\backupsynctool-windows-amd64.zip"
Remove-Item -Force $bundle -ErrorAction SilentlyContinue
Compress-Archive -Path "$outDir\backupsynctool.exe" -DestinationPath $bundle

if (-not $NoLaunch) {
  Start-Process -FilePath (Resolve-Path backupsynctool.exe) -WorkingDirectory $PSScriptRoot
  Start-Sleep -Milliseconds 750
  if (-not (Get-Process backupsynctool -ErrorAction SilentlyContinue)) { throw 'exe built but not running' }
}
Write-Host "ok backupsynctool.exe (Win7 audited)"
