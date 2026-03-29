$env:PATH += ";$env:USERPROFILE\.cargo\bin"
Set-Location "$PSScriptRoot\rust"
cargo build --release
Copy-Item "target\release\backupsynctool.exe" "..\backupsynctool.exe" -Force
Set-Location $PSScriptRoot
$v = "v" + ([regex]::Match((Get-Content "rust\Cargo.toml" -Raw), 'version\s*=\s*"([^"]+)"')).Groups[1].Value
git add backupsynctool.exe
git commit -m "release: $v"
git tag -f $v