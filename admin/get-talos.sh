#!/bin/sh
# get-talos.sh — refresh the Talos launchers in the shared kit folder from the
# newest GitHub release.
#
#   sh admin/get-talos.sh "/Users/me/OneDrive - Company/Talos"
#
# The kit lives on a shared OneDrive (see README, "Built to live on a shared
# OneDrive"): every machine runs its own synced replica of this folder, so
# refreshing it HERE refreshes it everywhere. This script is the only hand that
# touches the launchers. It never touches `catalog/` or `bundles/` — those are the
# integrator's content, edited in place.
#
# Plain `sh`, not nushell, on purpose: this is the tool that installs the tool
# that installs nushell. The bootstrap must lean only on what the OS ships —
# `sh`, `curl`-class tools, and here `gh`, which is the one thing you install
# by hand (the repo is private, so a download needs an authenticated client;
# `gh auth login` once).
#
# ⚠️ `gh release download` without a tag means "latest", and GitHub's "latest"
# skips pre-releases. Every Talos release is a pre-release so far, so the tag
# is looked up explicitly. Once a stable v0.1.0 exists this lookup still works.
#
# ⚠️ OneDrive and a running exe: Windows locks `Talos.exe` while it runs, so
# OneDrive cannot replace it on a machine where Talos is open — it retries later,
# and the new exe lands at the next sync after Talos is closed. Nothing to do
# but know it. The `.app` is a folder of files: on a Mac, mark the kit folder
# "Always keep on this device" so a half-synced app is never launched.
#
# macOS only for now (`ditto`, `xattr`). Linux is not shipped into the kit: the
# audience runs Mac and Windows, and a bare `Talos` file beside `Talos.exe` would
# only confuse the folder.
set -eu

DEST="${1:?usage: get-talos.sh <kit folder>}"
REPO="ChristianLemer/talos"
MAC_ASSET="Talos-macos-aarch64.app.zip"
WIN_ASSET="Talos.exe"

command -v gh >/dev/null || { echo "gh is required (brew install gh; gh auth login)" >&2; exit 1; }

TAG="$(gh release list -R "$REPO" --limit 1 --json tagName -q '.[0].tagName')"
[ -n "$TAG" ] || { echo "no release found on $REPO" >&2; exit 1; }
if [ "$(cat "$DEST/VERSION" 2>/dev/null || true)" = "$TAG" ]; then
    echo "already at $TAG"
    exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
gh release download "$TAG" -R "$REPO" --dir "$TMP" \
    --pattern "$MAC_ASSET" --pattern "$WIN_ASSET"

# macOS: ditto keeps the .app's symlinks and permissions (plain unzip mangles them);
# the app is unsigned, so drop the quarantine flag or Gatekeeper refuses it.
ditto -x -k "$TMP/$MAC_ASSET" "$TMP"
xattr -cr "$TMP/Talos.app"

mkdir -p "$DEST"
rm -rf "$DEST/Talos.app"
mv "$TMP/Talos.app" "$DEST/Talos.app"
mv "$TMP/$WIN_ASSET" "$DEST/$WIN_ASSET"
printf '%s\n' "$TAG" > "$DEST/VERSION"

echo "Talos $TAG installed in $DEST (Talos.app + Talos.exe; catalog/ and bundles/ untouched)"
