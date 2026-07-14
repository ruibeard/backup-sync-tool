#!/usr/bin/env bash
# Windows from Mac: push → VM 102 Win7 build → dist/windows/backupsynctool.exe
set -euo pipefail
cd "$(dirname "$0")"

HOST="${PROXMOX_HOST:-root@192.168.0.46}"
VM="${WIN10_VMID:-102}"
REPO='C:\Users\user\code\backup-sync-tool'
OUT=dist/windows
EXE=backupsynctool.exe
BRANCH="${1:-$(git rev-parse --abbrev-ref HEAD)}"
[[ -z "$(git status --porcelain)" ]] || { echo "dirty tree" >&2; exit 1; }

guest() {
  local b64; b64="$(printf '%s' "$2" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"
  ssh -o BatchMode=yes -o ConnectTimeout=15 "$HOST" \
    "qm guest exec $VM --timeout $1 -- powershell -NoProfile -EncodedCommand $b64"
}
gout() {
  printf '%s' "$1" | tr -d '\000\r' | python3 -c 'import sys,json,re;t=sys.stdin.read();m=re.search(r"\"out-data\"\s*:\s*\"((?:\\.|[^\"])*)\"",t);print((m.group(1) if m else t).encode().decode("unicode_escape"))' 2>/dev/null || printf '%s' "$1" | tr -d '\000\r'
}

# Compact Win7 guest build (deployed each run)
PS=$(cat <<'PS'
$ErrorActionPreference='Stop'; $env:PATH+=";$env:USERPROFILE\.cargo\bin"; Set-Location $PSScriptRoot
function W([int]$c){ Set-Content build-exitcode.txt "EXITCODE=$c" -Encoding ascii }
$t='x86_64-win7-windows-msvc'; $src="target\$t\release\backupsynctool.exe"
Remove-Item build-exitcode.txt -EA SilentlyContinue
Get-Process backupsynctool -EA SilentlyContinue | Stop-Process -Force; Start-Sleep 1
$ErrorActionPreference='Continue'
rustup toolchain install nightly; if($LASTEXITCODE){W 1; exit 1}
rustup component add rust-src --toolchain nightly; if($LASTEXITCODE){W 1; exit 1}
$env:RUSTFLAGS='-C target-feature=+crt-static'
cargo +nightly build -Z build-std=std,panic_abort --release --target $t
if($LASTEXITCODE -ne 0 -or -not (Test-Path $src)){ W 1; exit 1 }
$ErrorActionPreference='Stop'
$bad=@('GetSystemTimePreciseAsFileTime','WaitOnAddress','WakeByAddressAll','WakeByAddressSingle','ProcessPrng')
$dump=Get-Command dumpbin.exe -EA SilentlyContinue
if($dump){ $imp=& $dump.Source /imports $src 2>&1 | Out-String; foreach($b in $bad){ if($imp -match "\b$b\b"){ Write-Error "Win7-incompatible: $b"; W 1; exit 1 } } }
Copy-Item $src backupsynctool.exe -Force; W 0
PS
)

echo "push $BRANCH"
git push -u origin "HEAD:$BRANCH"
gout "$(guest 120 "\$ErrorActionPreference='Stop'; Set-Location '$REPO'; git fetch origin; git checkout $BRANCH; git reset --hard origin/$BRANCH; 'PULL_OK'")" >/dev/null

b64="$(printf '%s' "$PS" | base64 | tr -d '\n')"
gout "$(guest 60 "\$ErrorActionPreference='Stop'; Set-Location '$REPO'; [IO.File]::WriteAllText('_build_windows.ps1',[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$b64'))); 'OK'")" >/dev/null
gout "$(guest 60 "Set-Location '$REPO'; Remove-Item build-exitcode.txt,build-pid.txt -EA SilentlyContinue; \$p=Start-Process powershell -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','_build_windows.ps1' -WorkingDirectory '$REPO' -WindowStyle Hidden -PassThru; Set-Content build-pid.txt \$p.Id -Encoding ascii; 'STARTED'")" >/dev/null

echo "building on VM $VM..."
deadline=$((SECONDS + ${TIMEOUT_SECS:-2400}))
token=
while (( SECONDS < deadline )); do
  raw="$(guest 40 "Set-Location '$REPO'; if(Test-Path build-exitcode.txt){(Get-Content build-exitcode.txt -Raw).Trim()}elseif(Get-Process cargo,rustc -EA SilentlyContinue){'RUNNING'}else{'WAIT'}" 2>/dev/null || true)"
  token="$(gout "$raw" | grep -oE 'EXITCODE=[0-9]+|RUNNING|WAIT' | tail -1 || true)"
  echo "  $token"
  case "$token" in
    EXITCODE=0) break ;;
    EXITCODE=*) echo "build failed" >&2; exit 1 ;;
  esac
  sleep "${POLL_SECS:-15}"
done
[[ "$token" == EXITCODE=0 ]] || { echo "timeout" >&2; exit 1; }

mkdir -p "$OUT"
tmp=$(mktemp); trap 'rm -f "$tmp"' EXIT
size="$(gout "$(guest 60 "Set-Location '$REPO'; (Get-Item '$EXE').Length")" | grep -oE '[0-9]+' | tail -1)"
[[ -n "$size" && "$size" -gt 1000 ]] || { echo "bad size: $size" >&2; exit 1; }
off=0
while (( off < size )); do
  n=240000; (( off + n > size )) && n=$((size - off))
  chunk="$(gout "$(guest 120 "Set-Location '$REPO'; \$fs=[IO.File]::OpenRead('$EXE'); \$b=New-Object byte[] $n; \$null=\$fs.Seek($off,'Begin'); \$r=\$fs.Read(\$b,0,$n); \$fs.Close(); [Convert]::ToBase64String(\$b,0,\$r)")" | tr -d '[:space:]')"
  [[ -n "$chunk" ]] || { echo "chunk fail @$off" >&2; exit 1; }
  printf '%s' "$chunk" >> "$tmp"
  off=$((off + n))
done
python3 -c 'import sys,base64;sys.stdout.buffer.write(base64.b64decode(sys.stdin.read()))' <"$tmp" >"$OUT/$EXE"
[[ "$(wc -c <"$OUT/$EXE" | tr -d ' ')" == "$size" ]] || { echo "size mismatch" >&2; exit 1; }
head -c 2 "$OUT/$EXE" | grep -q MZ || { echo "not PE" >&2; exit 1; }
echo "ok $OUT/$EXE ($size bytes)"
