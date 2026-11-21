#!/usr/bin/env bash
# Release: ./release.sh --bump → build Windows at that version → stage dist/windows → ./release.sh (notarized mac + tag + GitHub)
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${1:-}" == "--bump" ]]; then
  OLD="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
  IFS=. read -r MA MI PA <<<"$OLD"
  NEW="$MA.$MI.$((PA + 1))"
  perl -i -pe "s/^version = \"\d+\.\d+\.\d+\"/version = \"$NEW\"/" Cargo.toml
  cargo update -w -q
  git add Cargo.toml Cargo.lock
  git commit -m "release: v$NEW"
  git push origin HEAD
  echo "bumped to $NEW — now build Windows (.\\build-windows.ps1 -NoLaunch), stage dist/windows, run ./release.sh"
  exit 0
fi

[[ -z "$(git status --porcelain)" ]] || { echo "dirty tree" >&2; exit 1; }
command -v gh >/dev/null || { echo "need gh" >&2; exit 1; }

WIN=dist/windows/backupsynctool.exe
WIN_BUNDLE=dist/windows/backupsynctool-windows-amd64.zip
[[ -f "$WIN" && -f "$WIN_BUNDLE" ]] || {
  echo "missing Windows app bundle — on a Windows machine run: .\\build-windows.ps1 -NoLaunch" >&2
  exit 1
}

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
# Version comes from Cargo.toml. Bump it (./release.sh --bump) BEFORE the
# Windows build, so the Windows exe and the tag report the same version.
NEW="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
TAG="v$NEW"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && { echo "tag exists: $TAG (run ./release.sh --bump, then rebuild Windows)" >&2; exit 1; }

./build-macos.sh --package --notarize
[[ -n "$(git status --porcelain Cargo.lock)" ]] && { git add Cargo.lock; git commit -m "chore: sync Cargo.lock for $TAG"; }

MAC="$(ls dist/macos/backupsynctool-macos-*.tar.gz | head -1)"
[[ -f "$MAC" ]] || { echo "missing mac tarball" >&2; exit 1; }
[[ -f "$WIN" && -f "$WIN_BUNDLE" ]] || { echo "missing Windows app bundle" >&2; exit 1; }

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
echo "ok $TAG"
