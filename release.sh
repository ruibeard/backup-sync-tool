#!/usr/bin/env bash
# Release BOTH platforms: ./release.sh
# bump + push -> Windows pull + build (remote) -> copy bundle back -> notarized mac -> tag -> GitHub
set -euo pipefail
cd "$(dirname "$0")"

WIN_HOST="${WIN_HOST:-ra668xps17}"
WIN_REPO="${WIN_REPO:-C:/Users/rui.almeida/code/backup-sync-tool}"

[[ -z "$(git status --porcelain)" ]] || { echo "dirty tree" >&2; exit 1; }
command -v gh >/dev/null || { echo "need gh" >&2; exit 1; }

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
OLD="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
IFS=. read -r MA MI PA <<<"$OLD"
NEW="$MA.$MI.$((PA + 1))"
TAG="v$NEW"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && { echo "tag exists: $TAG" >&2; exit 1; }

perl -i -pe "s/^version = \"\d+\.\d+\.\d+\"/version = \"$NEW\"/" Cargo.toml
cargo update -w -q
git add Cargo.toml Cargo.lock
git commit -m "release: $TAG"
git push origin "HEAD:$BRANCH"

# Windows: same commit, built on the Windows machine, bundle copied back here.
WIN=dist/windows/backupsynctool.exe
WIN_BUNDLE=dist/windows/backupsynctool-windows-amd64.zip
rm -f "$WIN" "$WIN_BUNDLE"
mkdir -p dist/windows
ssh -o BatchMode=yes "$WIN_HOST" "cd '$WIN_REPO'; git checkout -q $BRANCH; git pull --ff-only -q; if (\$LASTEXITCODE) { exit 1 }; powershell -NoProfile -ExecutionPolicy Bypass -File .\\build-windows.ps1 -NoLaunch; exit \$LASTEXITCODE"
scp -q -o BatchMode=yes "$WIN_HOST:$WIN_REPO/$WIN" "$WIN_HOST:$WIN_REPO/$WIN_BUNDLE" dist/windows/
[[ -f "$WIN" && -f "$WIN_BUNDLE" ]] || { echo "missing Windows app bundle" >&2; exit 1; }

./build-macos.sh --package --notarize
[[ -n "$(git status --porcelain Cargo.lock)" ]] && { git add Cargo.lock; git commit -m "chore: sync Cargo.lock for $TAG"; }

MAC="$(ls dist/macos/backupsynctool-macos-*.tar.gz | head -1)"
[[ -f "$MAC" ]] || { echo "missing mac tarball" >&2; exit 1; }

# Only main publishes "latest" (what the updater installs). Other branches
# (live-sync) publish a pre-release so WebDAV customers do not auto-update.
PRE=(); [[ "$BRANCH" == main ]] || PRE=(--prerelease)

git tag "$TAG"
git push -u origin "HEAD:$BRANCH"
git push origin "$TAG"

if gh release view "$TAG" >/dev/null 2>&1; then
  gh release upload "$TAG" "$WIN" "$WIN_BUNDLE" "$MAC" --clobber
else
  sleep 3
  gh release view "$TAG" >/dev/null 2>&1 \
    && gh release upload "$TAG" "$WIN" "$WIN_BUNDLE" "$MAC" --clobber \
    || gh release create "$TAG" "$WIN" "$WIN_BUNDLE" "$MAC" --generate-notes --title "$TAG" "${PRE[@]}"
fi
echo "ok $TAG (windows + mac)"
