$cmake = "C:\Program Files\CMake\bin\cmake.exe"
$out   = "build\Release\WebDavSync.exe"

Write-Host "Configuring..." -ForegroundColor Cyan
& $cmake -S . -B build -A Win32
if ($LASTEXITCODE -ne 0) { Write-Host "Configure failed." -ForegroundColor Red; exit 1 }

Write-Host "Building..." -ForegroundColor Cyan
& $cmake --build build --config Release
if ($LASTEXITCODE -ne 0) { Write-Host "Build failed." -ForegroundColor Red; exit 1 }

Write-Host "Done -> $out" -ForegroundColor Green
