#!/usr/bin/env bash
# Build Windows exe on Proxmox VM 102, then pull it into dist/windows/.
# Requires clean git tree (push-build-win10.sh pushes the branch first).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

PROXMOX_HOST="${PROXMOX_HOST:-root@192.168.0.46}"
VMID="${WIN10_VMID:-102}"
GUEST_REPO='C:\Users\user\code\backup-sync-tool'
OUT="dist/windows"
EXE_NAME="backupsynctool.exe"
CHUNK_BYTES=240000

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

echo "==> remote Windows build (VM $VMID)"
./push-build-win10.sh "$@"

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
# PE MZ header
if ! head -c 2 "$OUT/$EXE_NAME" | grep -q 'MZ'; then
  echo "error: fetched file is not a PE executable" >&2
  exit 1
fi

echo "Built: $ROOT/$OUT/$EXE_NAME ($actual bytes)"
