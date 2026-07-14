#!/usr/bin/env bash
# Release: bump → needs macOS package + Windows exe already built → tag → GitHub
set -euo pipefail
cd "$(dirname "$0")"

[[ -z "$(git status --porcelain)" ]] || { echo "dirty tree" >&2; exit 1; }
command -v gh >/dev/null || { echo "need gh" >&2; exit 1; }

WIN=dist/windows/backupsynctool.exe
[[ -f "$WIN" ]] || {
  echo "missing $WIN — on a Windows machine run: .\\build-windows.ps1 -NoLaunch" >&2
  exit 1
}

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
OLD="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
IFS=. read -r MA MI PA <<<"$OLD"
NEW="$MA.$MI.$((PA + 1))"
TAG="v$NEW"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && { echo "tag exists: $TAG" >&2; exit 1; }

perl -i -pe "s/^version = \"\\d+\\.\\d+\\.\\d+\"/version = \"$NEW\"/" Cargo.toml
git add Cargo.toml
[[ -n "$(git status --porcelain Cargo.lock)" ]] && git add Cargo.lock
git commit -m "release: $TAG"

./build-macos.sh --package
[[ -n "$(git status --porcelain Cargo.lock)" ]] && { git add Cargo.lock; git commit -m "chore: sync Cargo.lock for $TAG"; }

MAC="$(ls dist/macos/backupsynctool-macos-*.tar.gz | head -1)"
[[ -f "$MAC" ]] || { echo "missing mac tarball" >&2; exit 1; }
[[ -f "$WIN" ]] || { echo "missing $WIN" >&2; exit 1; }

git tag "$TAG"
git push -u origin "HEAD:$BRANCH"
git push origin "$TAG"

if gh release view "$TAG" >/dev/null 2>&1; then
  gh release upload "$TAG" "$WIN" "$MAC" --clobber
else
  sleep 3
  gh release view "$TAG" >/dev/null 2>&1 \
    && gh release upload "$TAG" "$WIN" "$MAC" --clobber \
    || gh release create "$TAG" "$WIN" "$MAC" --generate-notes --title "$TAG"
fi
echo "ok $TAG"
