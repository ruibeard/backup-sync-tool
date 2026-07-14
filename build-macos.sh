#!/usr/bin/env bash
# macOS: release .app → dist/macos/ (ad-hoc sign). Flags: --install --no-launch --package --identity=X
set -euo pipefail
cd "$(dirname "$0")"

VER="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
OUT=dist/macos
APP="$OUT/Backup Sync Tool.app"
ID=cam.rui.backupsynctool
INSTALL=0 LAUNCH=1 PACKAGE=0
SIGN="${MACOS_SIGN_IDENTITY:--}"

for a in "$@"; do
  case "$a" in
    --install|-i) INSTALL=1 ;;
    --no-launch) LAUNCH=0 ;;
    --package|-p) PACKAGE=1; LAUNCH=0 ;;
    --identity=*) SIGN="${a#--identity=}" ;;
  esac
done

cargo build --release
mkdir -p "$OUT" "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -f target/release/backupsynctool "$OUT/backupsynctool"
cp -f "$OUT/backupsynctool" "$APP/Contents/MacOS/backupsynctool"
chmod +x "$OUT/backupsynctool" "$APP/Contents/MacOS/backupsynctool"
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
codesign --force --deep --sign "$SIGN" --identifier "$ID" --timestamp=none "$APP"
cp -f "$APP/Contents/MacOS/backupsynctool" "$OUT/backupsynctool"

LAUNCH_APP="$(cd "$OUT" && pwd)/Backup Sync Tool.app"
if [[ $PACKAGE -eq 1 ]]; then
  arch=$(uname -m); [[ $arch == arm64 ]] && arch=aarch64
  tar -C "$OUT" -czf "$OUT/backupsynctool-macos-${arch}.tar.gz" backupsynctool
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
echo "ok $LAUNCH_APP"
