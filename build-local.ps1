param(
    [switch]$NoLaunch
)

$ErrorActionPreference = "Stop"
$env:PATH += ";$env:USERPROFILE\.cargo\bin"

Set-Location $PSScriptRoot

function Write-BuildExitCode([int]$Code) {
    Set-Content -Path (Join-Path $PSScriptRoot "build-exitcode.txt") -Value "EXITCODE=$Code" -Encoding ascii
}

$target = "x86_64-win7-windows-msvc"
$rootExe = "backupsynctool.exe"
$builtExe = "target\$target\release\backupsynctool.exe"
Remove-Item (Join-Path $PSScriptRoot "build-exitcode.txt") -ErrorAction SilentlyContinue

Write-Host "Stopping running Backup Sync Tool instance..."
$existing = Get-Process -Name "backupsynctool" -ErrorAction SilentlyContinue
if ($existing) {
    $existing | Stop-Process -Force
    $deadline = (Get-Date).AddSeconds(10)
    do {
        Start-Sleep -Milliseconds 250
        $existing = Get-Process -Name "backupsynctool" -ErrorAction SilentlyContinue
    } while ($existing -and (Get-Date) -lt $deadline)

    if ($existing) {
        Write-Error "Could not stop backupsynctool.exe before copying the new build."
        Write-BuildExitCode 1
        exit 1
    }
}

# Native tools write progress to stderr; do not treat that as terminating under Stop.
$previousErrorAction = $ErrorActionPreference
$ErrorActionPreference = "Continue"

Write-Host "Ensuring nightly Rust toolchain is installed..."
rustup toolchain install nightly
if ($LASTEXITCODE -ne 0) {
    $ErrorActionPreference = $previousErrorAction
    Write-Error "rustup toolchain install nightly failed"
    Write-BuildExitCode 1
    exit 1
}

Write-Host "Ensuring nightly rust-src is installed..."
rustup component add rust-src --toolchain nightly
if ($LASTEXITCODE -ne 0) {
    $ErrorActionPreference = $previousErrorAction
    Write-Error "rustup component add rust-src failed"
    Write-BuildExitCode 1
    exit 1
}

Write-Host "Building Windows 7-compatible release exe..."
$previousRustFlags = $env:RUSTFLAGS
if ([string]::IsNullOrWhiteSpace($env:RUSTFLAGS)) {
    $env:RUSTFLAGS = "-C target-feature=+crt-static"
} elseif ($env:RUSTFLAGS -notmatch "crt-static") {
    $env:RUSTFLAGS = "$env:RUSTFLAGS -C target-feature=+crt-static"
}

cargo +nightly build -Z build-std=std,panic_abort --release --target $target
$buildExit = $LASTEXITCODE
$env:RUSTFLAGS = $previousRustFlags
$ErrorActionPreference = $previousErrorAction
if ($buildExit -ne 0) {
    Write-Error "cargo build failed"
    Write-BuildExitCode 1
    exit 1
}

if (-not (Test-Path $builtExe)) {
    Write-Error "Build succeeded, but $builtExe was not found."
    Write-BuildExitCode 1
    exit 1
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
            Write-BuildExitCode 1
            exit 1
        }
    }

    Write-Host "Import check passed: no known Windows 8+ startup imports found."
} else {
    Write-Warning "Could not inspect imports. Install Visual Studio dumpbin or LLVM llvm-objdump to verify Windows 7 imports."
}

Copy-Item $builtExe $rootExe -Force
Write-Host "Copied $builtExe to repo root $rootExe."

if ($NoLaunch) {
    Write-Host "Skipping launch (-NoLaunch)."
    Write-BuildExitCode 0
    Write-Host "Done. Windows 7-compatible release build succeeded with 0 errors."
    exit 0
}

Write-Host "Launching backupsynctool.exe from repo root..."
$expectedPath = (Resolve-Path $rootExe).Path
Start-Process -FilePath $expectedPath -WorkingDirectory $PSScriptRoot
Start-Sleep -Milliseconds 500

$running = Get-Process -Name "backupsynctool" -ErrorAction SilentlyContinue | Where-Object {
    try {
        $_.MainModule.FileName -eq $expectedPath
    } catch {
        $false
    }
} | Select-Object -First 1

if (-not $running) {
    Write-Error "Build succeeded, but root backupsynctool.exe is not running."
    Write-BuildExitCode 1
    exit 1
}

Write-BuildExitCode 0
Write-Host "Done. Windows 7-compatible release build succeeded with 0 errors and Backup Sync Tool is running from repo root."
