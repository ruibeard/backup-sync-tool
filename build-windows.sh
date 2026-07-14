#!/usr/bin/env bash
# From Mac: push branch → Proxmox VM 102 Win7 build → pull exe into dist/windows/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

PROXMOX_HOST="${PROXMOX_HOST:-root@192.168.0.46}"
VMID="${WIN10_VMID:-102}"
GUEST_REPO='C:\Users\user\code\backup-sync-tool'
GUEST_PS1='_build_windows.ps1'
OUT="dist/windows"
EXE_NAME="backupsynctool.exe"
CHUNK_BYTES=240000
POLL_SECS="${POLL_SECS:-15}"
TIMEOUT_SECS="${TIMEOUT_SECS:-2400}"
BRANCH="${1:-}"

if [[ -z "$BRANCH" ]]; then
  BRANCH="$(git rev-parse --abbrev-ref HEAD)"
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Dirty tree — commit first." >&2
  git status -sb >&2
  exit 1
fi

guest_ps() {
  local timeout="$1"
  local b64
  b64="$(printf '%s' "$2" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"
  ssh -o BatchMode=yes -o ConnectTimeout=15 "$PROXMOX_HOST" \
    "qm guest exec $VMID --timeout $timeout -- powershell -NoProfile -EncodedCommand $b64"
}

guest_out() {
  printf '%s' "$1" | tr -d '\000\r' | python3 -c 'import sys,json,re; t=sys.stdin.read();
m=re.search(r"\"out-data\"\s*:\s*\"((?:\\.|[^\"])*)\"", t)
print((m.group(1) if m else t).encode("utf-8").decode("unicode_escape"))' 2>/dev/null || printf '%s' "$1" | tr -d '\000\r'
}

# Guest-side Win7 release build (written to the VM each run — not a repo entry point).
GUEST_BUILD_PS1=$(cat <<'PSEOF'
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
    if ($dumpbin) { return (& $dumpbin.Source /imports $Path 2>&1 | Out-String) }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vsPath = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1)
        if ($vsPath) {
            $dumpbinPath = Get-ChildItem -Path (Join-Path $vsPath "VC\Tools\MSVC") -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -like "*\bin\Hostx64\x64\dumpbin.exe" } |
                Select-Object -First 1
            if ($dumpbinPath) { return (& $dumpbinPath.FullName /imports $Path 2>&1 | Out-String) }
        }
    }
    $llvmObjdump = Get-Command llvm-objdump.exe -ErrorAction SilentlyContinue
    if ($llvmObjdump) { return (& $llvmObjdump.Source -p $Path 2>&1 | Out-String) }
    $objdump = Get-Command objdump.exe -ErrorAction SilentlyContinue
    if ($objdump) { return (& $objdump.Source -p $Path 2>&1 | Out-String) }
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
Write-BuildExitCode 0
Write-Host "Done. Windows 7-compatible release build succeeded with 0 errors."
PSEOF
)

echo "==> push $BRANCH @ $(git rev-parse --short HEAD)"
git push -u origin "HEAD:$BRANCH"

echo "==> pull on VM $VMID"
pull_raw="$(guest_ps 120 "\$ErrorActionPreference='Stop'; Set-Location '$GUEST_REPO'; git fetch origin; git checkout $BRANCH; git reset --hard origin/$BRANCH; git log -1 --oneline; 'PULL_OK'")"
guest_out "$pull_raw" | tail -5

echo "==> deploy guest build script"
PS1_B64="$(printf '%s' "$GUEST_BUILD_PS1" | base64 | tr -d '\n')"
deploy_raw="$(guest_ps 60 "\$ErrorActionPreference='Stop'; Set-Location '$GUEST_REPO'; [IO.File]::WriteAllText((Join-Path (Get-Location) '$GUEST_PS1'), [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$PS1_B64'))); 'DEPLOY_OK'")"
guest_out "$deploy_raw" | grep DEPLOY || guest_out "$deploy_raw"

echo "==> start build"
start_raw="$(guest_ps 60 "\$ErrorActionPreference='Continue'; Set-Location '$GUEST_REPO'; Remove-Item build-exitcode.txt,build-status.txt,build-pid.txt -ErrorAction SilentlyContinue; \$p = Start-Process -FilePath powershell.exe -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$GUEST_PS1') -WorkingDirectory '$GUEST_REPO' -WindowStyle Hidden -PassThru; Set-Content -Encoding ascii build-pid.txt \$p.Id; 'STARTED_PID=' + \$p.Id")"
guest_out "$start_raw" | grep STARTED || guest_out "$start_raw"

echo "==> wait (timeout ${TIMEOUT_SECS}s)"
deadline=$((SECONDS + TIMEOUT_SECS))
token=""
while (( SECONDS < deadline )); do
  raw="$(guest_ps 40 "\$ErrorActionPreference='Continue'; Set-Location '$GUEST_REPO'; if (Test-Path build-exitcode.txt) { ((Get-Content build-exitcode.txt -Raw) -replace \"\`0\",\"\").Trim() } elseif (Get-Process cargo,rustc -EA SilentlyContinue) { 'RUNNING' } elseif (Test-Path build-pid.txt) { \$id = 0; [int]::TryParse((((Get-Content build-pid.txt -Raw) -replace \"\`0\",\"\").Trim()), [ref]\$id) | Out-Null; if (\$id -gt 0 -and (Get-Process -Id \$id -EA SilentlyContinue)) { 'RUNNING' } else { 'STALE' } } else { 'NO_PID' }" 2>/dev/null || true)"
  text="$(guest_out "$raw")"
  token="$(printf '%s' "$text" | grep -oE 'EXITCODE=[0-9]+|RUNNING|STALE|NO_PID' | tail -1 || true)"
  echo "  $(date +%H:%M:%S) ${token:-$text}"

  case "$token" in
    EXITCODE=0)
      echo "==> BUILD OK"
      guest_out "$(guest_ps 30 "Set-Location '$GUEST_REPO'; (Get-Item backupsynctool.exe).FullName; (Get-Item backupsynctool.exe).Length; (Get-Item backupsynctool.exe).LastWriteTime.ToString('s'); git log -1 --oneline")"
      break
      ;;
    EXITCODE=*)
      echo "==> BUILD FAILED ($token)" >&2
      guest_out "$(guest_ps 60 "Set-Location '$GUEST_REPO'; Get-Content build-exitcode.txt -EA SilentlyContinue; if (Test-Path target\\x86_64-win7-windows-msvc\\release\\backupsynctool.exe) { 'exe present' } else { 'exe missing' }")" >&2 || true
      exit 1
      ;;
  esac
  sleep "$POLL_SECS"
done

if [[ "$token" != "EXITCODE=0" ]]; then
  echo "Timed out after ${TIMEOUT_SECS}s" >&2
  exit 1
fi

echo "==> fetch $EXE_NAME → $OUT/"
mkdir -p "$OUT"
TMP_B64="$(mktemp)"
trap 'rm -f "$TMP_B64"' EXIT

size_raw="$(guest_ps 60 "\$ErrorActionPreference='Stop'; Set-Location '$GUEST_REPO'; if (-not (Test-Path '$EXE_NAME')) { throw 'missing $EXE_NAME' }; (Get-Item '$EXE_NAME').Length")"
size="$(guest_out "$size_raw" | grep -oE '[0-9]+' | tail -1)"
if [[ -z "$size" || "$size" -lt 1000 ]]; then
  echo "error: could not read guest exe size (got: $size)" >&2
  exit 1
fi
echo "  guest size: $size bytes"

offset=0
while (( offset < size )); do
  take=$CHUNK_BYTES
  if (( offset + take > size )); then
    take=$((size - offset))
  fi
  chunk_raw="$(guest_ps 120 "\$ErrorActionPreference='Stop'; Set-Location '$GUEST_REPO'; \$fs=[IO.File]::OpenRead((Join-Path (Get-Location) '$EXE_NAME')); \$buf=New-Object byte[] $take; \$null=\$fs.Seek($offset,'Begin'); \$n=\$fs.Read(\$buf,0,$take); \$fs.Close(); if (\$n -le 0) { throw 'read failed' }; [Convert]::ToBase64String(\$buf,0,\$n)")"
  chunk="$(guest_out "$chunk_raw" | tr -d '[:space:]')"
  if [[ -z "$chunk" || "$chunk" == *"throw"* ]]; then
    echo "error: empty/invalid chunk at offset $offset" >&2
    exit 1
  fi
  printf '%s' "$chunk" >> "$TMP_B64"
  offset=$((offset + take))
  echo "  fetched $offset / $size"
done

python3 -c 'import sys,base64; sys.stdout.buffer.write(base64.b64decode(sys.stdin.read()))' \
  < "$TMP_B64" > "$OUT/$EXE_NAME"
actual="$(wc -c < "$OUT/$EXE_NAME" | tr -d ' ')"
if [[ "$actual" != "$size" ]]; then
  echo "error: size mismatch local=$actual guest=$size" >&2
  exit 1
fi
if ! head -c 2 "$OUT/$EXE_NAME" | grep -q 'MZ'; then
  echo "error: fetched file is not a PE executable" >&2
  exit 1
fi

echo "Built: $ROOT/$OUT/$EXE_NAME ($actual bytes)"
