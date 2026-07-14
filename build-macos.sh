#!/usr/bin/env bash
# macOS: release .app + pinned universal Syncthing -> dist/macos/
# Flags: --install --no-launch --package --identity=X
set -euo pipefail
cd "$(dirname "$0")"

VER="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
OUT=dist/macos
APP="$OUT/Backup Sync Tool.app"
ID=cam.rui.backupsynctool
INSTALL=0 LAUNCH=1 PACKAGE=0
SIGN="${MACOS_SIGN_IDENTITY:--}"

SYNCTHING_VERSION=v2.1.1
SYNCTHING_ARCHIVE="syncthing-macos-universal-${SYNCTHING_VERSION}.zip"
SYNCTHING_URL="https://github.com/syncthing/syncthing/releases/download/${SYNCTHING_VERSION}/${SYNCTHING_ARCHIVE}"
SYNCTHING_SHA256=72f17a0447ad5f3bc3dee7a98655b6e2892a7d39b7e4bcc225ee699543714ffd
CACHE=target/syncthing-cache/macos

for a in "$@"; do
  case "$a" in
    --install|-i) INSTALL=1 ;;
    --no-launch) LAUNCH=0 ;;
    --package|-p) PACKAGE=1; LAUNCH=0 ;;
    --identity=*) SIGN="${a#--identity=}" ;;
  esac
done

mkdir -p "$OUT" "$CACHE"
archive="$CACHE/$SYNCTHING_ARCHIVE"
if [[ -f "$archive" ]]; then
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  [[ "$actual" == "$SYNCTHING_SHA256" ]] || rm -f "$archive"
fi
if [[ ! -f "$archive" ]]; then
  curl --fail --location --silent --show-error "$SYNCTHING_URL" -o "$archive"
fi
actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
if [[ "$actual" != "$SYNCTHING_SHA256" ]]; then
  rm -f "$archive"
  echo "error: Syncthing checksum mismatch (expected $SYNCTHING_SHA256, got $actual)" >&2
  exit 1
fi

engine_unpack="$CACHE/unpacked"
rm -rf "$engine_unpack"
mkdir -p "$engine_unpack"
ditto -x -k "$archive" "$engine_unpack"
engine_src="$engine_unpack/syncthing-macos-universal-${SYNCTHING_VERSION}/syncthing"
engine_license="$engine_unpack/syncthing-macos-universal-${SYNCTHING_VERSION}/LICENSE.txt"
[[ -x "$engine_src" ]] || { echo "error: pinned archive has no Syncthing executable" >&2; exit 1; }
[[ -f "$engine_license" ]] || { echo "error: pinned archive has no Syncthing license" >&2; exit 1; }
engine_version="$($engine_src --version)"
[[ "$engine_version" == "syncthing ${SYNCTHING_VERSION} "* ]] || {
  echo "error: unexpected Syncthing version: $engine_version" >&2
  exit 1
}
engine_file="$(file "$engine_src")"
[[ "$engine_file" == *"universal binary"* && "$engine_file" == *"x86_64"* && "$engine_file" == *"arm64"* ]] || {
  echo "error: Syncthing engine is not universal x86_64 + arm64" >&2
  exit 1
}

# Preserve logs created beside the executable by older, unsealed builds before
# replacing the app bundle. Current builds write to Application Support.
LEGACY_LOGS="$APP/Contents/MacOS/logs"
STATE_LOGS="$HOME/Library/Application Support/BackupSyncTool/logs"
if [[ -d "$LEGACY_LOGS" ]]; then
  mkdir -p "$STATE_LOGS"
  cp -p "$LEGACY_LOGS"/*.log "$STATE_LOGS"/ 2>/dev/null || true
fi

cargo build --release
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -f target/release/backupsynctool "$OUT/backupsynctool"
cp -f "$OUT/backupsynctool" "$APP/Contents/MacOS/backupsynctool"
cp -f "$engine_src" "$OUT/syncthing"
cp -f "$engine_src" "$APP/Contents/Resources/syncthing"
cp -f "$engine_license" "$OUT/syncthing-LICENSE.txt"
cp -f "$engine_license" "$APP/Contents/Resources/syncthing-LICENSE.txt"
chmod +x "$OUT/backupsynctool" "$OUT/syncthing" \
  "$APP/Contents/MacOS/backupsynctool" "$APP/Contents/Resources/syncthing"
[[ -f assets/AppIcon.icns ]] && cp -f assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Backup Sync Tool</string>
  <key>CFBundleDisplayName</key><string>Backup Sync Tool</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleVersion</key><string>$VER</string>
  <key>CFBundleShortVersionString</key><string>$VER</string>
  <key>CFBundleExecutable</key><string>backupsynctool</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF

xattr -cr "$APP" 2>/dev/null || true
# Sign inside-out. Do not use --deep: it can conceal an incorrectly sealed
# nested engine and makes later signature failures difficult to diagnose.
codesign --force --sign "$SIGN" --timestamp=none "$APP/Contents/Resources/syncthing"
codesign --force --sign "$SIGN" --timestamp=none "$APP/Contents/MacOS/backupsynctool"
codesign --force --sign "$SIGN" --identifier "$ID" --timestamp=none "$APP"
codesign --verify --strict --verbose=2 "$APP"

LAUNCH_APP="$(cd "$OUT" && pwd)/Backup Sync Tool.app"
if [[ $PACKAGE -eq 1 ]]; then
  arch=$(uname -m); [[ $arch == arm64 ]] && arch=aarch64
  # Package the already-sealed application as one update unit. Auto-update can
  # then swap the complete bundle without invalidating its nested signatures.
  tar -C "$OUT" -czf "$OUT/backupsynctool-macos-${arch}.tar.gz" \
    "Backup Sync Tool.app"
fi
if [[ $INSTALL -eq 1 ]]; then
  rm -rf "/Applications/Backup Sync Tool.app"
  ditto "$APP" "/Applications/Backup Sync Tool.app"
  LAUNCH_APP="/Applications/Backup Sync Tool.app"
fi
if [[ $LAUNCH -eq 1 ]]; then
  pkill -x backupsynctool 2>/dev/null || true
  rm -f "$HOME/Library/Application Support/BackupSyncTool/backupsynctool.pid"
  open "$LAUNCH_APP"
  sleep 1
  pgrep -x backupsynctool >/dev/null || { echo "error: app did not start" >&2; exit 1; }
fi
echo "ok $LAUNCH_APP + Syncthing $SYNCTHING_VERSION (universal)"
