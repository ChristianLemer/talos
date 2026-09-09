#!/bin/sh
# get-talos.sh — compose a Talos kit: the launchers of ONE release, verified, plus your
# content; or verify a kit that is already there.
#
#   sh admin/get-talos.sh <kit folder> [--version TAG] [--content DIR]
#   sh admin/get-talos.sh <kit folder> --verify
#
#   <kit folder>    where the kit lives — the shared OneDrive folder your team launches
#                   from, or a folder of your own you will double-click in
#   --version TAG   the release to fetch (v0.0.1-beta.42). Default: the `.talos-version`
#                   file beside the content (or in the current directory) if there is one,
#                   else the newest release
#   --content DIR   a folder holding `catalog/` and `bundles/`: YOUR content, copied into
#                   the kit in place of the socle. Without it, the kit gets the SOCLE of
#                   the same release — `talos-content.zip`, what a stranger receives
#   --verify        recompute every file in the kit against MANIFEST.sha256 and exit 1 on
#                   the first difference. Offline; run it on any synced replica
#
# Three ways to use it, one script:
#
#   · Discovering Talos: a folder that works, ready to double-click.
#         sh get-talos.sh ~/Talos
#     The launchers and the socle of the newest release — Git, Node, the agent, the
#     editor, uv, and three optional bundles — verified, checked, in one folder.
#
#   · A content repo (the integrator's case). Your repo holds `catalog/`, `bundles/` and a
#     one-line `.talos-version`. From its root:
#         sh get-talos.sh "/Users/me/OneDrive - Company/Talos" --content .
#     Bumping Talos is a commit that changes `.talos-version`; publishing is this line.
#
#   · A shared folder whose content is edited in place: run with --content pointing at
#     the kit itself, and only the launchers move.
#
# ⭐ THE KIT IS CHECKED BY THE ENGINE IT SHIPS. After composing, the script runs
# `Talos.app/Contents/MacOS/Talos --check catalog bundles` on the folder: the content is
# validated by the exact binary that will read it, and a kit that does not pass is not
# left behind — the check runs before the manifest is written, and a failure exits 1.
#
# ⭐ EVERY BYTE IS VERIFIED, twice. At download, each asset is checked against the sha256
# GitHub publishes for it in the release's API record, and a mismatch refuses the kit
# rather than composing it. After placing, the script writes what it put on disk:
#
#   VERSION           the tag, one line
#   KIT.txt           provenance — tag, date, and per asset the size + published digest
#   MANIFEST.sha256   one line per FILE as it sits in the kit — `Talos.exe`, every file
#                     inside `Talos.app/`, `catalog/` and `bundles/` — in `shasum -c` format
#
# The manifest is the part that matters on a shared OneDrive: every machine runs its own
# synced replica of this folder, and the failure mode of a replica is not a tampered
# download, it is a HALF-SYNCED one — right file names, wrong bytes, on a colleague's
# machine. The published digest covers the .app's ZIP, which is gone once extracted, so
# the manifest is computed here over the extracted tree; `--verify` recomputes it from
# any replica, offline, with the `shasum` macOS ships.
#
# Plain `sh`, not nushell, on purpose: this is the tool that installs the tool that
# installs nushell. A bootstrap may lean only on what the OS ships — `sh`, `curl`,
# `shasum`, `ditto`, `xattr` — and nothing you install by hand.
#
# ⚠️ The newest tag is read from the releases list, not from GitHub's "latest": that
# link skips pre-releases, and every Talos release is a pre-release until v0.1.0.
#
# ⚠️ OneDrive and a running exe: Windows locks `Talos.exe` while it runs, so OneDrive
# cannot replace it on a machine where Talos is open — it retries, and the new exe lands
# at the next sync after Talos is closed. On a Mac, mark the kit folder "Always keep on
# this device" so a half-synced app is never launched — and `--verify` is how you know.
#
# macOS only (`ditto`, `xattr`). Linux is not put in the kit: the audience runs Mac and
# Windows, and a bare `Talos` file beside `Talos.exe` would only confuse the folder.
set -eu

REPO="ChristianLemer/talos"
API="https://api.github.com/repos/$REPO/releases"
MAC_ASSET="Talos-macos-aarch64.app.zip"
WIN_ASSET="Talos.exe"
CONTENT_ASSET="talos-content.zip"

DEST=""
TAG=""
CONTENT=""
VERIFY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --version) TAG="${2:?--version needs a tag}"; shift 2 ;;
        --content) CONTENT="${2:?--content needs a folder}"; shift 2 ;;
        --verify) VERIFY=1; shift ;;
        -h|--help) sed -n '2,60p' "$0"; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) [ -z "$DEST" ] || { echo "one kit folder only" >&2; exit 2; }; DEST="$1"; shift ;;
    esac
done
[ -n "$DEST" ] || { echo "usage: get-talos.sh <kit folder> [--version TAG] [--content DIR] | --verify" >&2; exit 2; }

sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

# ── --verify: the replica against its manifest, nothing else ──────────────────────────
if [ "$VERIFY" = 1 ]; then
    [ -f "$DEST/MANIFEST.sha256" ] || { echo "no MANIFEST.sha256 in $DEST — not a kit this script composed" >&2; exit 2; }
    cd "$DEST"
    if shasum -a 256 -c --quiet MANIFEST.sha256; then
        echo "kit $(cat VERSION 2>/dev/null || echo '?') in $DEST: every file matches its manifest"
        exit 0
    else
        echo "kit in $DEST: the lines above differ from the manifest — a half-synced replica, or a hand edit" >&2
        exit 1
    fi
fi

if [ -n "$CONTENT" ]; then
    [ -d "$CONTENT/catalog" ] && [ -d "$CONTENT/bundles" ] \
        || { echo "$CONTENT has no catalog/ + bundles/" >&2; exit 2; }
fi
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v shasum >/dev/null || { echo "shasum is required" >&2; exit 1; }

# The version: the flag, else the pin file beside the content (or in the cwd), else newest.
if [ -z "$TAG" ]; then
    PIN="${CONTENT:-.}/.talos-version"
    [ -f "$PIN" ] && TAG="$(tr -d '[:space:]' < "$PIN")"
fi
if [ -z "$TAG" ]; then
    # The first `tag_name` in the API's list is the newest release, pre-release or not.
    TAG="$(curl -fsSL "$API?per_page=1" \
        | grep -m1 '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    [ -n "$TAG" ] || { echo "no release found on $REPO" >&2; exit 1; }
fi

mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
if [ "$(cat "$DEST/VERSION" 2>/dev/null || true)" = "$TAG" ] && [ -f "$DEST/MANIFEST.sha256" ] && [ -z "$CONTENT" ] && [ -d "$DEST/catalog" ]; then
    echo "kit already at $TAG (run with --verify to check the replica)"
    exit 0
fi

# What GitHub says each asset of this release weighs and hashes: one line per asset,
# `name size sha256:hex`, read from the same record a client would use to find the tag.
curl -fsSL "$API/tags/$TAG" \
    | grep -o -E '"(name|size|digest)": *("[^"]*"|[0-9]+)' \
    | awk -F'": *' '{ gsub(/"/, "", $2)
                      if ($1 == "\"name") n = $2
                      else if ($1 == "\"size") s = $2
                      else if ($1 == "\"digest") print n, s, $2 }' > "$TMP/assets.txt"
[ -s "$TMP/assets.txt" ] || { echo "release $TAG publishes no asset digests — refusing to compose an unverifiable kit" >&2; exit 1; }

# Which assets: the two launchers always; the socle only when no content is given.
ASSETS="$MAC_ASSET $WIN_ASSET"
[ -n "$CONTENT" ] || ASSETS="$ASSETS $CONTENT_ASSET"
for asset in $ASSETS; do
    want="$(awk -v a="$asset" '$1 == a { sub(/^sha256:/, "", $3); print $3 }' "$TMP/assets.txt")"
    [ -n "$want" ] || { echo "release $TAG has no digest for $asset — refusing" >&2; exit 1; }
    curl -fsSL -o "$TMP/$asset" "https://github.com/$REPO/releases/download/$TAG/$asset"
    got="$(sha256_of "$TMP/$asset")"
    if [ "$got" != "$want" ]; then
        echo "$asset: sha256 $got does not match the published $want — refusing" >&2
        exit 1
    fi
    echo "verified $asset"
done

# ditto keeps the .app's symlinks and permissions (plain unzip mangles them); the app
# is unsigned, so drop the quarantine flag or Gatekeeper refuses it.
ditto -x -k "$TMP/$MAC_ASSET" "$TMP"
xattr -cr "$TMP/Talos.app"

rm -rf "$DEST/Talos.app"
mv "$TMP/Talos.app" "$DEST/Talos.app"
mv "$TMP/$WIN_ASSET" "$DEST/$WIN_ASSET"

# The content: yours, or the socle of this release. REPLACED, not merged — a package
# deleted from the source must leave the kit too, or the kit keeps proposing what the
# author removed.
rm -rf "$DEST/catalog" "$DEST/bundles"
if [ -n "$CONTENT" ]; then
    cp -R "$CONTENT/catalog" "$DEST/catalog"
    cp -R "$CONTENT/bundles" "$DEST/bundles"
    echo "content from $CONTENT"
else
    unzip -q -o "$TMP/$CONTENT_ASSET" -d "$DEST"
    echo "content: the socle of $TAG"
fi

# Checked by the engine it ships: the Mac binary just placed reads the kit's content
# the strict way. A kit that does not pass is not left behind for a colleague to open.
if ! "$DEST/Talos.app/Contents/MacOS/Talos" --check "$DEST/catalog" "$DEST/bundles"; then
    echo "the content does not pass Talos --check — kit NOT published" >&2
    rm -f "$DEST/VERSION" "$DEST/MANIFEST.sha256"
    exit 1
fi

# What is on disk now, file by file, in the format `shasum -c` reads back — relative
# to the kit, so the manifest travels with the replica.
( cd "$DEST" && { echo "$WIN_ASSET"; find Talos.app catalog bundles -type f; } | LC_ALL=C sort \
    | while IFS= read -r f; do shasum -a 256 "$f"; done ) > "$TMP/MANIFEST.sha256"
mv "$TMP/MANIFEST.sha256" "$DEST/MANIFEST.sha256"

printf '%s\n' "$TAG" > "$DEST/VERSION"
{
    echo "talos $TAG"
    echo "composed $(date -u +%Y-%m-%dT%H:%M:%SZ) by get-talos.sh"
    echo "release https://github.com/$REPO/releases/tag/$TAG"
    echo
    echo "assets as published (name size sha256), verified at download:"
    awk -v m="$MAC_ASSET" -v w="$WIN_ASSET" -v c="$CONTENT_ASSET" '$1 == m || $1 == w || $1 == c { print "  " $0 }' "$TMP/assets.txt"
    echo
    if [ -n "$CONTENT" ]; then echo "content: $CONTENT (the integrator's)"; else echo "content: the socle of $TAG"; fi
    echo
    echo "files as placed: MANIFEST.sha256 — check any replica with: get-talos.sh <kit> --verify"
} > "$DEST/KIT.txt"
echo "kit at $TAG: Talos.app + Talos.exe + content, checked, manifest written"
echo "kit ready in $DEST"
