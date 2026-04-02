$env:PATH += ";$env:USERPROFILE\.cargo\bin"
Set-Location $PSScriptRoot
cargo build --release
Copy-Item "target\release\backupsynctool.exe" ".\backupsynctool.exe" -Force
$v = "v" + ([regex]::Match((Get-Content "Cargo.toml" -Raw), 'version\s*=\s*"([^"]+)"')).Groups[1].Value
git add backupsynctool.exe license-inspector.exe
git commit -m "release: $v"
git tag -f $v
