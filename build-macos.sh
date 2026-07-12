#!/usr/bin/env bash
# Build signed .app, then relaunch (like build-local.ps1).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

VERSION="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
OUT="dist/macos"
BIN="$OUT/backupsynctool"
APP="$OUT/Backup Sync Tool.app"
INSTALL=0
LAUNCH=1

for arg in "$@"; do
  case "$arg" in
    --install|-i) INSTALL=1 ;;
    --no-launch) LAUNCH=0 ;;
    -h|--help)
      echo "Usage: ./build-macos.sh [--install] [--no-launch]"
      echo "  Build release .app under dist/macos/ and launch it"
      echo "  --install    also copy to /Applications (launch that copy)"
      echo "  --no-launch  build only"
      exit 0
      ;;
  esac
done

echo "Building backupsynctool --release (v${VERSION})..."
cargo build --release

mkdir -p "$OUT"
cp -f "target/release/backupsynctool" "$BIN"
chmod +x "$BIN"
codesign --force --sign - "$BIN"
xattr -cr "$BIN" 2>/dev/null || true

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -f "$BIN" "$APP/Contents/MacOS/backupsynctool"
chmod +x "$APP/Contents/MacOS/backupsynctool"
[[ -f assets/AppIcon.icns ]] && cp -f assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Backup Sync Tool</string>
  <key>CFBundleDisplayName</key><string>Backup Sync Tool</string>
  <key>CFBundleIdentifier</key><string>cam.rui.backupsynctool</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>backupsynctool</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"
xattr -cr "$APP" 2>/dev/null || true

APP_ABS="$(cd "$OUT" && pwd)/Backup Sync Tool.app"
LAUNCH_APP="$APP_ABS"
echo "Built: $APP_ABS"

if [[ "$INSTALL" -eq 1 ]]; then
  DEST="/Applications/Backup Sync Tool.app"
  rm -rf "$DEST"
  cp -R "$APP" "$DEST"
  codesign --force --deep --sign - "$DEST"
  xattr -cr "$DEST" 2>/dev/null || true
  LAUNCH_APP="$DEST"
  echo "Installed: $DEST"
fi

if [[ "$LAUNCH" -eq 1 ]]; then
  pkill -x backupsynctool 2>/dev/null || true
  rm -f "${HOME}/Library/Application Support/BackupSyncTool/backupsynctool.pid"
  open "$LAUNCH_APP"
  sleep 1
  if pgrep -x backupsynctool >/dev/null; then
    echo "Running: $(pgrep -lf backupsynctool)"
  else
    echo "error: build ok but app did not start" >&2
    exit 1
  fi
else
  echo "Skipped launch (--no-launch)"
  echo "Launch: open \"$LAUNCH_APP\""
fi
