#!/usr/bin/env bash
# Dual-platform release from Mac: bump → build macOS + Windows → tag → GitHub release.
# Assets: dist/windows/backupsynctool.exe + dist/macos/backupsynctool-macos-*.tar.gz
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Dirty tree — commit or stash first." >&2
  git status -sb >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh CLI required (brew install gh && gh auth login)" >&2
  exit 1
fi

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
echo "==> release on branch $BRANCH @ $(git rev-parse --short HEAD)"

# Bump patch in Cargo.toml (same scheme as release.ps1)
VER_LINE="$(grep -E '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | head -1)"
if [[ -z "$VER_LINE" ]]; then
  echo "error: could not parse version from Cargo.toml" >&2
  exit 1
fi
OLD_VER="$(printf '%s' "$VER_LINE" | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')"
MAJOR="$(printf '%s' "$OLD_VER" | cut -d. -f1)"
MINOR="$(printf '%s' "$OLD_VER" | cut -d. -f2)"
PATCH="$(printf '%s' "$OLD_VER" | cut -d. -f3)"
NEW_VER="${MAJOR}.${MINOR}.$((PATCH + 1))"
TAG="v${NEW_VER}"

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "error: tag $TAG already exists locally" >&2
  exit 1
fi

perl -i -pe "s/^version = \"\\d+\\.\\d+\\.\\d+\"/version = \"$NEW_VER\"/" Cargo.toml
echo "==> bumped $OLD_VER → $NEW_VER"

git add Cargo.toml
# Cargo.lock may change with version embeds on some setups; include if dirty.
if [[ -n "$(git status --porcelain Cargo.lock)" ]]; then
  git add Cargo.lock
fi
git commit -m "release: $TAG"

echo "==> macOS package"
./build-macos.sh --package

MAC_TGZ="$(ls -1 dist/macos/backupsynctool-macos-*.tar.gz | head -1)"
if [[ -z "$MAC_TGZ" || ! -f "$MAC_TGZ" ]]; then
  echo "error: macOS tarball missing under dist/macos/" >&2
  exit 1
fi

echo "==> Windows build + fetch"
./build-windows.sh "$BRANCH"

WIN_EXE="dist/windows/backupsynctool.exe"
if [[ ! -f "$WIN_EXE" ]]; then
  echo "error: missing $WIN_EXE" >&2
  exit 1
fi

echo "==> tag + push"
git tag "$TAG"
git push -u origin "HEAD:$BRANCH"
git push origin "$TAG"

echo "==> GitHub release $TAG"
# GHA may create an empty notes-only release; upload assets either way.
if gh release view "$TAG" >/dev/null 2>&1; then
  gh release upload "$TAG" "$WIN_EXE" "$MAC_TGZ" --clobber
else
  # Wait briefly for workflow race, then create if still missing.
  sleep 3
  if gh release view "$TAG" >/dev/null 2>&1; then
    gh release upload "$TAG" "$WIN_EXE" "$MAC_TGZ" --clobber
  else
    gh release create "$TAG" "$WIN_EXE" "$MAC_TGZ" --generate-notes --title "$TAG"
  fi
fi

echo "Done. $TAG"
echo "https://github.com/ruibeard/backup-sync-tool/releases/tag/$TAG"
