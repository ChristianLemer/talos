#!/bin/sh
# get-talos.sh — compose a Talos kit: the launchers of ONE release, plus your content.
#
#   sh admin/get-talos.sh <kit folder> [--version TAG] [--content DIR]
#
#   <kit folder>    where the kit lives — the shared OneDrive folder your team launches
#                   from, or a local dist/ you will copy yourself
#   --version TAG   the release to fetch (v0.0.1-beta.37). Default: the `.talos-version`
#                   file in the current directory if there is one, else the newest release
#   --content DIR   a folder holding `catalog/` and `bundles/`: they are copied into the
#                   kit, replacing what was there. Without it the kit's content is left
#                   untouched — the launchers are refreshed, the content is yours
#
# Two ways to use it, one script:
#
#   · A content repo (the integrator's case). Your repo holds `catalog/`, `bundles/` and a
#     one-line `.talos-version`. From its root:
#         sh get-talos.sh "/Users/me/OneDrive - Company/Talos" --content .
#     Bumping Talos is a commit that changes `.talos-version`; publishing is this line.
#     Run `Talos --check catalog bundles` first (or in your pipeline) — it refuses what
#     the runtime would silently skip.
#
#   · A shared folder with no repo. The content is edited in place on the share:
#         sh get-talos.sh "/Users/me/OneDrive - Company/Talos"
#     refreshes Talos.app and Talos.exe to the newest release and touches nothing else.
#
# The kit lives on a shared OneDrive (README, "Built to live on a shared OneDrive"):
# every machine runs its own synced replica of this folder, so refreshing it HERE
# refreshes it everywhere.
#
# Plain `sh`, not nushell, on purpose: this is the tool that installs the tool that
# installs nushell. A bootstrap may lean only on what the OS ships — `sh`, `ditto`,
# `xattr` — plus `gh`, the one thing you install by hand: the repo is private, so a
# download needs an authenticated client (`brew install gh; gh auth login`, once).
#
# ⚠️ `gh release download` without a tag means "latest", and GitHub's "latest" skips
# pre-releases. Every Talos release is a pre-release so far, so the newest tag is looked
# up explicitly. Once a stable v0.1.0 exists this lookup still works.
#
# ⚠️ OneDrive and a running exe: Windows locks `Talos.exe` while it runs, so OneDrive
# cannot replace it on a machine where Talos is open — it retries, and the new exe lands
# at the next sync after Talos is closed. Nothing to do but know it. The `.app` is a
# folder of files: on a Mac, mark the kit folder "Always keep on this device" so a
# half-synced app is never launched.
#
# macOS only for now (`ditto`, `xattr`). Linux is not put in the kit: the audience runs
# Mac and Windows, and a bare `Talos` file beside `Talos.exe` would only confuse the folder.
set -eu

REPO="ChristianLemer/talos"
MAC_ASSET="Talos-macos-aarch64.app.zip"
WIN_ASSET="Talos.exe"

DEST=""
TAG=""
CONTENT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --version) TAG="${2:?--version needs a tag}"; shift 2 ;;
        --content) CONTENT="${2:?--content needs a folder}"; shift 2 ;;
        -h|--help) sed -n '2,50p' "$0"; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) [ -z "$DEST" ] || { echo "one kit folder only" >&2; exit 2; }; DEST="$1"; shift ;;
    esac
done
[ -n "$DEST" ] || { echo "usage: get-talos.sh <kit folder> [--version TAG] [--content DIR]" >&2; exit 2; }
if [ -n "$CONTENT" ]; then
    [ -d "$CONTENT/catalog" ] && [ -d "$CONTENT/bundles" ] \
        || { echo "$CONTENT has no catalog/ + bundles/" >&2; exit 2; }
fi

command -v gh >/dev/null || { echo "gh is required (brew install gh; gh auth login)" >&2; exit 1; }

# The version: the flag, else the pin file beside the content (or in the cwd), else newest.
if [ -z "$TAG" ]; then
    PIN="${CONTENT:-.}/.talos-version"
    [ -f "$PIN" ] && TAG="$(tr -d '[:space:]' < "$PIN")"
fi
if [ -z "$TAG" ]; then
    TAG="$(gh release list -R "$REPO" --limit 1 --json tagName -q '.[0].tagName')"
    [ -n "$TAG" ] || { echo "no release found on $REPO" >&2; exit 1; }
fi

mkdir -p "$DEST"
if [ "$(cat "$DEST/VERSION" 2>/dev/null || true)" = "$TAG" ]; then
    echo "launchers already at $TAG"
else
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    gh release download "$TAG" -R "$REPO" --dir "$TMP" \
        --pattern "$MAC_ASSET" --pattern "$WIN_ASSET"
    # ditto keeps the .app's symlinks and permissions (plain unzip mangles them); the app
    # is unsigned, so drop the quarantine flag or Gatekeeper refuses it.
    ditto -x -k "$TMP/$MAC_ASSET" "$TMP"
    xattr -cr "$TMP/Talos.app"
    rm -rf "$DEST/Talos.app"
    mv "$TMP/Talos.app" "$DEST/Talos.app"
    mv "$TMP/$WIN_ASSET" "$DEST/$WIN_ASSET"
    printf '%s\n' "$TAG" > "$DEST/VERSION"
    echo "launchers at $TAG (Talos.app + Talos.exe)"
fi

if [ -n "$CONTENT" ]; then
    # The content is REPLACED, not merged: a package deleted from the repo must leave the
    # kit too, or the kit keeps proposing what the integrator removed.
    rm -rf "$DEST/catalog" "$DEST/bundles"
    cp -R "$CONTENT/catalog" "$DEST/catalog"
    cp -R "$CONTENT/bundles" "$DEST/bundles"
    echo "content from $CONTENT"
fi
echo "kit ready in $DEST"
