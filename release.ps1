param(
    [switch]$GitRelease
)

$ErrorActionPreference = "Stop"
$env:PATH += ";$env:USERPROFILE\.cargo\bin"

Set-Location $PSScriptRoot

Get-Process backupsynctool -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

cargo build --release
Copy-Item "target\release\backupsynctool.exe" ".\backupsynctool.exe" -Force

Write-Host "Built and copied backupsynctool.exe to repo root."

if (-not $GitRelease) {
    return
}

$v = "v" + ([regex]::Match((Get-Content "Cargo.toml" -Raw), 'version\s*=\s*"([^"]+)"')).Groups[1].Value
git add backupsynctool.exe license-inspector.exe Cargo.toml Cargo.lock

$hasStagedChanges = (git diff --cached --quiet); if ($LASTEXITCODE -eq 0) { $hasStagedChanges = $false } else { $hasStagedChanges = $true }
if ($hasStagedChanges) {
    git commit -m "release: $v"
}

git tag -f $v
Write-Host "Git release tagging complete: $v"
