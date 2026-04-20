$ErrorActionPreference = "Stop"
$env:PATH += ";$env:USERPROFILE\.cargo\bin"

Set-Location $PSScriptRoot

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

# Build
cargo build --release
if ($LASTEXITCODE -ne 0)
{
    Write-Error "cargo build failed"; exit 1
}
Copy-Item "target\release\backupsynctool.exe" ".\backupsynctool.exe" -Force
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
