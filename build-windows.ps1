# Build Win7-compatible app + pinned Syncthing engine -> root + dist\windows\
param([switch]$NoLaunch)
$ErrorActionPreference = 'Stop'
$env:PATH += ";$env:USERPROFILE\.cargo\bin"
Set-Location $PSScriptRoot

$target = 'x86_64-win7-windows-msvc'
$src = "target\$target\release\backupsynctool.exe"
$outDir = 'dist\windows'
$cacheDir = 'target\syncthing-cache\windows'

$syncthingVersion = 'v2.1.1'
$syncthingSourceUrl = "https://github.com/syncthing/syncthing/releases/download/$syncthingVersion/syncthing-source-$syncthingVersion.tar.gz"
$syncthingSourceSha256 = '418a99452f484abf30e403b769d0cf914a038142cd1a7e10be85b68f45d9f42a'
$goLegacyVersion = '1.26.5-1'
$goLegacyArchive = "go-legacy-win7-$goLegacyVersion.windows_amd64.zip"
$goLegacyUrl = "https://github.com/thongtech/go-legacy-win7/releases/download/v$goLegacyVersion/$goLegacyArchive"
$goLegacySha256 = 'c9d0c79dc2b408a4ea580b62a3d093a4219f9ff95316ef891dc987827e6900e3'

function Get-PinnedFile([string]$Url, [string]$Path, [string]$Sha256) {
  if (Test-Path $Path) {
    $actual = (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLowerInvariant()
    if ($actual -ne $Sha256) { Remove-Item -Force $Path }
  }
  if (-not (Test-Path $Path)) {
    Write-Host "download $Url"
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Path
  }
  $actual = (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLowerInvariant()
  if ($actual -ne $Sha256) {
    Remove-Item -Force $Path -ErrorAction SilentlyContinue
    throw "checksum mismatch for $Url (expected $Sha256, got $actual)"
  }
}

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

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
New-Item -ItemType Directory -Force -Path $cacheDir, $outDir | Out-Null

$rootApp = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'backupsynctool.exe'))
$distApp = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "$outDir\backupsynctool.exe"))
$rootEngine = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'syncthing.exe'))
$distEngine = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "$outDir\syncthing.exe"))
Stop-BundledProcess 'backupsynctool' @($rootApp, $distApp)
Stop-BundledProcess 'syncthing' @($rootEngine, $distEngine)
Start-Sleep -Milliseconds 500

$goArchivePath = Join-Path $cacheDir $goLegacyArchive
Get-PinnedFile $goLegacyUrl $goArchivePath $goLegacySha256
$goRoot = Join-Path $cacheDir 'go-legacy-win7'
Remove-Item -Recurse -Force $goRoot -ErrorAction SilentlyContinue
Expand-Archive -Path $goArchivePath -DestinationPath $cacheDir -Force
$goExe = Join-Path $goRoot 'bin\go.exe'
if (-not (Test-Path $goExe)) { throw "legacy Go archive did not contain $goExe" }
$goVersion = (& $goExe version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $goVersion -notmatch '\bgo1\.26\.5\b.*\bwindows/amd64\b') {
  throw "unexpected go-legacy-win7 toolchain: $goVersion"
}

$sourceArchive = Join-Path $cacheDir "syncthing-source-$syncthingVersion.tar.gz"
Get-PinnedFile $syncthingSourceUrl $sourceArchive $syncthingSourceSha256
$sourceRoot = Join-Path $cacheDir 'source'
Remove-Item -Recurse -Force $sourceRoot -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $sourceRoot | Out-Null
$tar = Get-Command tar.exe -ErrorAction SilentlyContinue
if (-not $tar) { $tar = Get-Command tar -ErrorAction SilentlyContinue }
if (-not $tar) { throw 'tar is required to extract the pinned Syncthing source archive' }
& $tar.Source -xzf $sourceArchive -C $sourceRoot
if ($LASTEXITCODE -ne 0) { throw 'Syncthing source extraction failed' }
$syncthingSource = Join-Path $sourceRoot 'syncthing'
if (-not (Test-Path (Join-Path $syncthingSource 'build.go'))) {
  throw 'pinned Syncthing archive has an unexpected layout'
}

$oldPath = $env:PATH
$oldGoRoot = $env:GOROOT
$oldGoPath = $env:GOPATH
$oldCgo = $env:CGO_ENABLED
try {
  $env:GOROOT = $goRoot
  $env:GOPATH = [IO.Path]::GetFullPath((Join-Path $cacheDir 'gopath'))
  $env:PATH = "$(Join-Path $goRoot 'bin');$oldPath"
  $env:CGO_ENABLED = '0'
  Push-Location $syncthingSource
  try {
    & $goExe run build.go -goos windows -goarch amd64 -no-upgrade -version $syncthingVersion build syncthing
    if ($LASTEXITCODE -ne 0) { throw 'Syncthing build failed' }
  } finally {
    Pop-Location
  }
} finally {
  $env:PATH = $oldPath
  $env:GOROOT = $oldGoRoot
  $env:GOPATH = $oldGoPath
  $env:CGO_ENABLED = $oldCgo
}

$engineSrc = Join-Path $syncthingSource 'syncthing.exe'
if (-not (Test-Path $engineSrc)) { throw 'Syncthing build produced no syncthing.exe' }
$engineLicenseSrc = Join-Path $syncthingSource 'LICENSE'
if (-not (Test-Path $engineLicenseSrc)) { throw 'Syncthing source archive omitted LICENSE' }
$engineVersion = (& $engineSrc --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $engineVersion -notmatch '^syncthing v2\.1\.1\b' -or $engineVersion -notmatch '\bnoupgrade\b') {
  throw "unexpected Syncthing build (must be v2.1.1 with self-upgrade disabled): $engineVersion"
}

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
foreach ($binary in @($src, $engineSrc)) {
  $imports = & $dump.Source /imports $binary 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0) { throw "dumpbin import audit failed for $binary" }
  foreach ($import in $forbiddenImports) {
    if ($imports -match "\b$([regex]::Escape($import))\b") {
      throw "Win7-incompatible import in $binary`: $import"
    }
  }
}

Copy-Item $src backupsynctool.exe -Force
Copy-Item $src "$outDir\backupsynctool.exe" -Force
Copy-Item $engineSrc syncthing.exe -Force
Copy-Item $engineSrc "$outDir\syncthing.exe" -Force
Copy-Item $engineLicenseSrc "$outDir\syncthing-LICENSE.txt" -Force
$bundle = "$outDir\backupsynctool-windows-amd64.zip"
Remove-Item -Force $bundle -ErrorAction SilentlyContinue
Compress-Archive -Path "$outDir\backupsynctool.exe", "$outDir\syncthing.exe", "$outDir\syncthing-LICENSE.txt" -DestinationPath $bundle

if (-not $NoLaunch) {
  Start-Process -FilePath (Resolve-Path backupsynctool.exe) -WorkingDirectory $PSScriptRoot
  Start-Sleep -Milliseconds 750
  if (-not (Get-Process backupsynctool -ErrorAction SilentlyContinue)) { throw 'exe built but not running' }
}
Write-Host "ok backupsynctool.exe + syncthing.exe $syncthingVersion (Win7 audited)"
