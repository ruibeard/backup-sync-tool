#!/usr/bin/env bash
# Push current branch → Proxmox VM 102 pull → build-local.ps1 -NoLaunch → wait.
set -euo pipefail

PROXMOX_HOST="${PROXMOX_HOST:-root@192.168.0.46}"
VMID="${WIN10_VMID:-102}"
GUEST_REPO='C:\Users\user\code\backup-sync-tool'
POLL_SECS="${POLL_SECS:-15}"
TIMEOUT_SECS="${TIMEOUT_SECS:-2400}"
BRANCH="${1:-}"

cd "$(cd "$(dirname "$0")/.." && pwd)"

if [[ -z "$BRANCH" ]]; then
  BRANCH="$(git rev-parse --abbrev-ref HEAD)"
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Dirty tree — commit first." >&2
  git status -sb >&2
  exit 1
fi

echo "==> push $BRANCH @ $(git rev-parse --short HEAD)"
git push -u origin "HEAD:$BRANCH"

# Run PowerShell inside guest. $timeout is qm guest-exec wait seconds.
guest_ps() {
  local timeout="$1"
  local b64
  b64="$(printf '%s' "$2" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"
  ssh -o BatchMode=yes -o ConnectTimeout=15 "$PROXMOX_HOST" \
    "qm guest exec $VMID --timeout $timeout -- powershell -NoProfile -EncodedCommand $b64"
}

echo "==> pull on VM $VMID"
guest_ps 120 "\$ErrorActionPreference='Stop'; Set-Location '$GUEST_REPO'; git fetch origin; git checkout $BRANCH; git reset --hard origin/$BRANCH; git log -1 --oneline; 'PULL_OK'"

echo "==> start build"
guest_ps 60 "\$ErrorActionPreference='Continue'; Set-Location '$GUEST_REPO'; Remove-Item build-exitcode.txt,build-local.log,build-pid.txt -ErrorAction SilentlyContinue; \$p = Start-Process -FilePath powershell.exe -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','build-local.ps1','-NoLaunch') -WorkingDirectory '$GUEST_REPO' -WindowStyle Hidden -PassThru; Set-Content build-pid.txt \$p.Id; 'STARTED_PID=' + \$p.Id"

echo "==> wait (timeout ${TIMEOUT_SECS}s)"
deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  raw="$(guest_ps 40 "\$ErrorActionPreference='Continue'; Set-Location '$GUEST_REPO'; if (Test-Path build-exitcode.txt) { 'DONE ' + (Get-Content build-exitcode.txt -Raw).Trim() } elseif (Test-Path build-pid.txt) { \$id=[int]((Get-Content build-pid.txt -Raw).Trim()); if (Get-Process -Id \$id -EA SilentlyContinue) { 'RUNNING pid=' + \$id } elseif (Get-Process cargo,rustc -EA SilentlyContinue) { 'RUNNING compiler' } else { 'STALE' } } else { 'NO_PID' }" 2>/dev/null || true)"
  line="$(printf '%s' "$raw" | tr -d '\r' | grep -E 'DONE |RUNNING |STALE|NO_PID|PULL_OK|STARTED' | tail -1 || true)"
  echo "  $(date +%H:%M:%S) ${line:-$raw}"

  if printf '%s' "$raw" | grep -q 'DONE EXITCODE=0'; then
    echo "==> BUILD OK"
    guest_ps 30 "Set-Location '$GUEST_REPO'; Get-Item backupsynctool.exe | Format-List FullName,Length,LastWriteTime | Out-String; git log -1 --oneline"
    exit 0
  fi
  if printf '%s' "$raw" | grep -q 'DONE EXITCODE='; then
    echo "==> BUILD FAILED" >&2
    guest_ps 60 "Set-Location '$GUEST_REPO'; Get-Content build-local.log -Tail 50 -EA SilentlyContinue" >&2 || true
    exit 1
  fi
  sleep "$POLL_SECS"
done

echo "Timed out after ${TIMEOUT_SECS}s" >&2
exit 1
