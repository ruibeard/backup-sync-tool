# DEPRECATED for dual-platform releases: use ./release.sh from macOS instead.
# This script remains for Windows-only emergency bumps on VM 102.
$ErrorActionPreference = "Stop"
$env:PATH += ";$env:USERPROFILE\.cargo\bin"

Set-Location $PSScriptRoot

$target = "x86_64-win7-windows-msvc"
$builtExe = "target\$target\release\backupsynctool.exe"

# Bump patch version
$toml = Get-Content "Cargo.toml" -Raw
$m = [regex]::Match($toml, 'version\s*=\s*"(\d+)\.(\d+)\.(\d+)"')
$major = $m.Groups[1].Value
$minor = $m.Groups[2].Value
$patch = [int]$m.Groups[3].Value + 1
$newVersion = "$major.$minor.$patch"
$toml = $toml -replace '(?m)^version\s*=\s*"\d+\.\d+\.\d+"', "version = `"$newVersion`""
Set-Content "Cargo.toml" $toml -NoNewline
Write-Host "Bumped version to $newVersion"

# Kill running instance
Get-Process backupsynctool -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

# Build Windows 7-compatible release exe. This keeps a single public
# backupsynctool.exe asset that works on Windows 7 SP1 x64 through Windows 11.
rustup toolchain install nightly
if ($LASTEXITCODE -ne 0) {
    Write-Error "rustup toolchain install nightly failed"; exit 1
}

rustup component add rust-src --toolchain nightly
if ($LASTEXITCODE -ne 0) {
    Write-Error "rustup component add rust-src failed"; exit 1
}

$previousRustFlags = $env:RUSTFLAGS
if ([string]::IsNullOrWhiteSpace($env:RUSTFLAGS)) {
    $env:RUSTFLAGS = "-C target-feature=+crt-static"
} elseif ($env:RUSTFLAGS -notmatch "crt-static") {
    $env:RUSTFLAGS = "$env:RUSTFLAGS -C target-feature=+crt-static"
}

cargo +nightly build -Z build-std=std,panic_abort --release --target $target
$buildExit = $LASTEXITCODE
$env:RUSTFLAGS = $previousRustFlags
if ($buildExit -ne 0)
{
    Write-Error "cargo build failed"; exit 1
}

if (-not (Test-Path $builtExe)) {
    Write-Error "Build succeeded, but $builtExe was not found."; exit 1
}

function Get-ImportTableText {
    param([string]$Path)

    $dumpbin = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    if ($dumpbin) {
        return (& $dumpbin.Source /imports $Path 2>&1 | Out-String)
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vsPath = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1)
        if ($vsPath) {
            $dumpbinPath = Get-ChildItem -Path (Join-Path $vsPath "VC\Tools\MSVC") -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -like "*\bin\Hostx64\x64\dumpbin.exe" } |
                Select-Object -First 1
            if ($dumpbinPath) {
                return (& $dumpbinPath.FullName /imports $Path 2>&1 | Out-String)
            }
        }
    }

    $llvmObjdump = Get-Command llvm-objdump.exe -ErrorAction SilentlyContinue
    if ($llvmObjdump) {
        return (& $llvmObjdump.Source -p $Path 2>&1 | Out-String)
    }

    $objdump = Get-Command objdump.exe -ErrorAction SilentlyContinue
    if ($objdump) {
        return (& $objdump.Source -p $Path 2>&1 | Out-String)
    }

    return $null
}

$imports = Get-ImportTableText $builtExe
if ($imports) {
    $blockedImports = @(
        "GetSystemTimePreciseAsFileTime",
        "WaitOnAddress",
        "WakeByAddressAll",
        "WakeByAddressSingle",
        "ProcessPrng"
    )

    foreach ($blockedImport in $blockedImports) {
        if ($imports -match "\b$([regex]::Escape($blockedImport))\b") {
            Write-Error "Windows 7 incompatible import found: $blockedImport"
            exit 1
        }
    }

    Write-Host "Import check passed: no known Windows 8+ startup imports found."
} else {
    Write-Warning "Could not inspect imports. Install Visual Studio dumpbin or LLVM llvm-objdump to verify Windows 7 imports."
}

Copy-Item $builtExe ".\backupsynctool.exe" -Force
Write-Host "Built backupsynctool.exe"

# Commit everything, tag, push
$v = "v$newVersion"
git add -A
git commit -m "release: $v"
if ($LASTEXITCODE -ne 0)
{
    Write-Error "git commit failed"; exit 1
}

git rev-parse -q --verify "refs/tags/$v" *> $null
if ($LASTEXITCODE -eq 0)
{
    Write-Error "Tag $v already exists locally"
    exit 1
}

git tag $v
if ($LASTEXITCODE -ne 0)
{
    Write-Error "git tag $v failed"; exit 1
}

git push origin main
if ($LASTEXITCODE -ne 0)
{
    Write-Error "git push origin main failed"; exit 1
}

# The workflow triggers on tag pushes, so push the tag explicitly and verify it exists remotely.
git push origin $v
if ($LASTEXITCODE -ne 0)
{
    Write-Error "git push origin $v failed"; exit 1
}

$remoteTag = git ls-remote --tags origin "refs/tags/$v"
if (-not $remoteTag)
{
    Write-Error "Remote tag $v was not found after push; GitHub Actions will not create the release."
    exit 1
}

Write-Host "Done. Pushed main and tag $v"
Write-Host "GitHub Actions should create the release at: https://github.com/ruibeard/backup-sync-tool/releases/tag/$v"
