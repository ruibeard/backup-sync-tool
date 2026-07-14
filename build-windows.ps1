# Build Win7-compatible exe on this machine → backupsynctool.exe + dist\windows\
param([switch]$NoLaunch)
$ErrorActionPreference = 'Stop'
$env:PATH += ";$env:USERPROFILE\.cargo\bin"
Set-Location $PSScriptRoot

$target = 'x86_64-win7-windows-msvc'
$src = "target\$target\release\backupsynctool.exe"
$outDir = 'dist\windows'

Get-Process backupsynctool -EA SilentlyContinue | Stop-Process -Force
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

$dump = Get-Command dumpbin.exe -EA SilentlyContinue
if ($dump) {
  $imp = & $dump.Source /imports $src 2>&1 | Out-String
  foreach ($b in @('GetSystemTimePreciseAsFileTime','WaitOnAddress','WakeByAddressAll','WakeByAddressSingle','ProcessPrng')) {
    if ($imp -match "\b$([regex]::Escape($b))\b") { throw "Win7-incompatible import: $b" }
  }
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null
Copy-Item $src backupsynctool.exe -Force
Copy-Item $src "$outDir\backupsynctool.exe" -Force

if (-not $NoLaunch) {
  Start-Process -FilePath (Resolve-Path backupsynctool.exe) -WorkingDirectory $PSScriptRoot
  Start-Sleep -Milliseconds 500
  if (-not (Get-Process backupsynctool -EA SilentlyContinue)) { throw 'exe built but not running' }
}
Write-Host "ok backupsynctool.exe"
