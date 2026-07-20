# Build Win7-compatible app (in-process chunk sync engine) -> root + dist\windows\
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
# Stop leftover Syncthing from older installs if still running beside this tree.
$legacyEngine = @(
  [IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'syncthing.exe')),
  [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "$outDir\syncthing.exe"))
)
Stop-BundledProcess 'syncthing' $legacyEngine
Start-Sleep -Milliseconds 500

$ErrorActionPreference = 'Continue'
rustup toolchain install nightly
if ($LASTEXITCODE) { throw 'rustup nightly failed' }
rustup component add rust-src --toolchain nightly
if ($LASTEXITCODE) { throw 'rust-src failed' }
$env:RUSTFLAGS = '-C target-feature=+crt-static'
cargo +nightly build -Z build-std=std,panic_abort --release --target $target
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $src)) { throw 'cargo build failed' }
$ErrorActionPreference = 'Stop'

$dump = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
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
# Remove leftover Syncthing artifacts from older builds.
Remove-Item -Force syncthing.exe, "$outDir\syncthing.exe", "$outDir\syncthing-LICENSE.txt" -ErrorAction SilentlyContinue
$bundle = "$outDir\backupsynctool-windows-amd64.zip"
Remove-Item -Force $bundle -ErrorAction SilentlyContinue
Compress-Archive -Path "$outDir\backupsynctool.exe" -DestinationPath $bundle

if (-not $NoLaunch) {
  Start-Process -FilePath (Resolve-Path backupsynctool.exe) -WorkingDirectory $PSScriptRoot
  Start-Sleep -Milliseconds 750
  if (-not (Get-Process backupsynctool -ErrorAction SilentlyContinue)) { throw 'exe built but not running' }
}
Write-Host "ok backupsynctool.exe (Win7 audited, chunk sync engine, no Syncthing)"
