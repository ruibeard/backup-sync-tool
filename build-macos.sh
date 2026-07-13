#!/usr/bin/env bash
# Build signed .app, then relaunch (like build-local.ps1).
# Signs with a stable local identity so Keychain stops re-prompting after each rebuild.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

VERSION="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
OUT="dist/macos"
BIN="$OUT/backupsynctool"
APP="$OUT/Backup Sync Tool.app"
BUNDLE_ID="cam.rui.backupsynctool"
INSTALL=0
LAUNCH=1
PACKAGE=0

for arg in "$@"; do
  case "$arg" in
    --install|-i) INSTALL=1 ;;
    --no-launch) LAUNCH=0 ;;
    --package|-p)
      PACKAGE=1
      LAUNCH=0
      ;;
    -h|--help)
      echo "Usage: ./build-macos.sh [--install] [--no-launch] [--package]"
      echo "  Build release .app under dist/macos/ and launch it"
      echo "  --install    also copy to /Applications (launch that copy)"
      echo "  --no-launch  build only"
      echo "  --package    build + write updater tarball (implies --no-launch)"
      echo ""
      echo "Signing: uses identity \"Backup Sync Tool Dev\" (auto-created once in login keychain)."
      echo "Override with MACOS_SIGN_IDENTITY=..."
      exit 0
      ;;
  esac
done

SIGN_IDENTITY="$(./scripts/ensure-macos-sign-identity.sh)"
echo "Signing as: $SIGN_IDENTITY"

sign_path() {
  local path="$1"
  codesign --force --sign "$SIGN_IDENTITY" --identifier "$BUNDLE_ID" --timestamp=none "$path"
}

echo "Building backupsynctool --release (v${VERSION})..."
cargo build --release

mkdir -p "$OUT"
cp -f "target/release/backupsynctool" "$BIN"
chmod +x "$BIN"
sign_path "$BIN"
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
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
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

# Sign nested binary first, then the .app bundle (stable identity → stable Keychain ACL).
sign_path "$APP/Contents/MacOS/backupsynctool"
codesign --force --deep --sign "$SIGN_IDENTITY" --identifier "$BUNDLE_ID" --timestamp=none "$APP"
xattr -cr "$APP" 2>/dev/null || true

APP_ABS="$(cd "$OUT" && pwd)/Backup Sync Tool.app"
LAUNCH_APP="$APP_ABS"
echo "Built: $APP_ABS"
codesign -dv --verbose=2 "$APP" 2>&1 | grep -E 'Authority|Identifier|Signature' || true

if [[ "$PACKAGE" -eq 1 ]]; then
  ARCH="$(uname -m)"
  if [[ "$ARCH" == "arm64" ]]; then
    ARCH="aarch64"
  fi
  # Updater asset is the bare binary (signed).
  TGZ="$OUT/backupsynctool-macos-${ARCH}.tar.gz"
  tar -C "$OUT" -czf "$TGZ" backupsynctool
  echo "Packaged: $(cd "$OUT" && pwd)/backupsynctool-macos-${ARCH}.tar.gz"
fi

if [[ "$INSTALL" -eq 1 ]]; then
  DEST="/Applications/Backup Sync Tool.app"
  rm -rf "$DEST"
  cp -R "$APP" "$DEST"
  codesign --force --deep --sign "$SIGN_IDENTITY" --identifier "$BUNDLE_ID" --timestamp=none "$DEST"
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
  echo "Skipped launch (--no-launch/--package)"
  echo "Launch: open \"$LAUNCH_APP\""
fi
