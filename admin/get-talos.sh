#!/bin/sh
# get-talos.sh — compose a Talos kit: the launchers of ONE release, verified, plus your
# content; or verify a kit that is already there. Runs on macOS and on Linux.
#
#   sh admin/get-talos.sh <kit folder> [--version TAG] [--content DIR]
#   sh admin/get-talos.sh <kit folder> --verify
#
#   <kit folder>    where the kit lives — the shared folder your team launches from, or a
#                   folder of your own you will double-click in
#   --version TAG   the release to fetch (v0.0.1-beta.42). Default: the `.talos-version`
#                   file beside the content (or in the current directory) if there is one,
#                   else the newest release
#   --content DIR   a folder holding `catalog/` and `bundles/`: YOUR content, copied into
#                   the kit in place of the socle. Without it, the kit gets the SOCLE of
#                   the same release — `talos-content.zip`, what a stranger receives
#   --verify        recompute every file in the kit against MANIFEST.sha256 and exit 1 on
#                   the first difference. Offline; run it on any synced replica
#
# On Windows, `get-talos.ps1` composes the very same kit — same layout, same manifest.
#
# ⭐ THE KIT IT COMPOSES:
#
#   Talos/
#     MacOS/     Talos.app
#     Windows/   Talos.exe
#     Linux/     Talos
#     catalog/   bundles/
#     .talos/    VERSION · KIT.txt · MANIFEST.sha256
#
# One folder per OS, so every launcher keeps its own standard name — `Talos` next to
# `Talos.exe` in one folder would show as two identical rows in an Explorer that hides
# extensions, and one of them does nothing when double-clicked. Whoever receives the kit
# opens the name of their OS and double-clicks. The metadata is machine-facing, never
# opened by hand, so it sits out of the way under `.talos/`.
#
# The same kit is composed whatever machine composes it: the folder does not record who
# made it. What each OS ships is only HOW the `.app` is unpacked (`ditto` on macOS, plain
# `unzip` elsewhere — the bundle holds four files and no symlink, so both are faithful)
# and WHICH launcher runs the check below.
#
# ⭐ THE KIT IS CHECKED BY THE ENGINE IT SHIPS, in the position it will be read from.
# After composing, the script runs `--check` with NO arguments on the launcher it has just
# placed: the engine resolves `catalog/` and `bundles/` itself, exactly as it will on a
# colleague's machine. That proves the CONTENT and the LAYOUT in one gesture instead of
# assuming the second. A kit that does not pass is not left behind — the check runs before
# the manifest is written, and a failure exits 1.
#
# ⭐ EVERY BYTE IS VERIFIED, twice. At download, each asset is checked against the sha256
# GitHub publishes for it in the release's API record, and a mismatch refuses the kit
# rather than composing it. After placing, the script writes what it put on disk:
#
#   .talos/VERSION           the tag, one line
#   .talos/KIT.txt           provenance — tag, date, and per asset the size + digest
#   .talos/MANIFEST.sha256   one line per FILE as it sits in the kit — every launcher,
#                            every file under `catalog/` and `bundles/` — `shasum -c` format
#
# The manifest is the part that matters on a shared drive: every machine runs its own
# synced replica of this folder, and the failure mode of a replica is not a tampered
# download, it is a HALF-SYNCED one — right file names, wrong bytes, on a colleague's
# machine. The published digest covers the .app's ZIP, which is gone once extracted, so
# the manifest is computed here over the extracted tree; `--verify` recomputes it from
# any replica, offline, with what the OS ships (`shasum` on macOS, `sha256sum` on Linux,
# `Get-FileHash` on Windows — one file, three readers).
#
# Plain `sh`, not nushell, on purpose: this is the tool that installs the tool that
# installs nushell. A bootstrap may lean only on what the OS ships — `sh`, `curl`,
# `unzip`, `shasum`/`sha256sum` — and nothing you install by hand.
#
# ⚠️ The newest tag is read from the releases list, not from GitHub's "latest": that
# link skips pre-releases, and every Talos release is a pre-release until v0.1.0.
#
# ⚠️ A running exe and a shared drive: Windows locks `Talos.exe` while it runs, so the
# sync cannot replace it on a machine where Talos is open — it retries, and the new exe
# lands at the next sync after Talos is closed. On a Mac, mark the kit folder "Always keep
# on this device" so a half-synced app is never launched — and `--verify` is how you know.
set -eu

REPO="ChristianLemer/talos"
API="https://api.github.com/repos/$REPO/releases"
MAC_ASSET="Talos-macos-aarch64.app.zip"
WIN_ASSET="Talos.exe"
LNX_ASSET="Talos-linux-x86_64"
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
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) [ -z "$DEST" ] || { echo "one kit folder only" >&2; exit 2; }; DEST="$1"; shift ;;
    esac
done
[ -n "$DEST" ] || { echo "usage: get-talos.sh <kit folder> [--version TAG] [--content DIR] | --verify" >&2; exit 2; }

# What this OS ships to hash with. Both write `<hex>  <path>` and both read that back
# with `-c`, so the manifest one composes is the manifest the other verifies.
case "$(uname -s)" in
    Darwin) SHA="shasum -a 256"; HOST_OS="MacOS" ;;
    Linux)  SHA="sha256sum";     HOST_OS="Linux" ;;
    *) echo "get-talos.sh runs on macOS and Linux; on Windows use get-talos.ps1" >&2; exit 2 ;;
esac
sha256_of() { $SHA "$1" | cut -d' ' -f1; }

# ── --verify: the replica against its manifest, nothing else ──────────────────────────
if [ "$VERIFY" = 1 ]; then
    # `.talos/` is where a kit composed by this version keeps it; the root is where kits
    # composed before the OS folders kept it. Both are verifiable, neither is re-composed.
    MANIFEST=""
    for m in ".talos/MANIFEST.sha256" "MANIFEST.sha256"; do
        [ -f "$DEST/$m" ] && { MANIFEST="$m"; break; }
    done
    [ -n "$MANIFEST" ] || { echo "no MANIFEST.sha256 in $DEST — not a kit this script composed" >&2; exit 2; }
    cd "$DEST"
    VERSION_SEEN="$(cat .talos/VERSION 2>/dev/null || cat VERSION 2>/dev/null || echo '?')"
    if $SHA -c --quiet "$MANIFEST"; then
        echo "kit $VERSION_SEEN in $DEST: every file matches its manifest"
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
command -v unzip >/dev/null || { echo "unzip is required" >&2; exit 1; }

# The version: the flag, else the pin file beside the content (or in the cwd), else newest.
if [ -z "$TAG" ]; then
    PIN="${CONTENT:-.}/.talos-version"
    [ -f "$PIN" ] && TAG="$(tr -d '[:space:]' < "$PIN")"
fi
if [ -z "$TAG" ]; then
    # The first `tag_name` in the API's list is the newest release, pre-release or not.
    # To a FILE, not down a pipe: in `curl | grep -m1`, grep leaves on its first match and
    # curl dies of EPIPE with a noisy "(23) Failure writing output" — and, worse, a pipeline
    # reports only its LAST command, so `set -e` never sees a network failure there.
    TMP_TAG="$(mktemp)"
    curl -fsSL "$API?per_page=1" -o "$TMP_TAG"
    TAG="$(grep -m1 '"tag_name"' "$TMP_TAG" | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    rm -f "$TMP_TAG"
    [ -n "$TAG" ] || { echo "no release found on $REPO" >&2; exit 1; }
fi

mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"   # absolute: the check below runs from another directory
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
if [ "$(cat "$DEST/.talos/VERSION" 2>/dev/null || true)" = "$TAG" ] && [ -f "$DEST/.talos/MANIFEST.sha256" ] && [ -z "$CONTENT" ] && [ -d "$DEST/catalog" ]; then
    echo "kit already at $TAG (run with --verify to check the replica)"
    exit 0
fi

# What GitHub says each asset of this release weighs and hashes: one line per asset,
# `name size sha256:hex`, read from the same record a client would use to find the tag.
curl -fsSL "$API/tags/$TAG" -o "$TMP/release.json"
grep -o -E '"(name|size|digest)": *("[^"]*"|[0-9]+)' "$TMP/release.json" \
    | awk -F'": *' '{ gsub(/"/, "", $2)
                      if ($1 == "\"name") n = $2
                      else if ($1 == "\"size") s = $2
                      else if ($1 == "\"digest") print n, s, $2 }' > "$TMP/assets.txt"
[ -s "$TMP/assets.txt" ] || { echo "release $TAG publishes no asset digests — refusing to compose an unverifiable kit" >&2; exit 1; }

# Which assets: the three launchers always — the kit does not depend on who composed it;
# the socle only when no content is given.
ASSETS="$MAC_ASSET $WIN_ASSET $LNX_ASSET"
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

# The .app. `ditto` on macOS keeps the bundle's metadata and drops nothing; `unzip`
# elsewhere is faithful too — measured on the real asset, it holds four files, no symlink,
# and restores the two exec bits. `xattr` only exists to clear the quarantine flag macOS
# puts on a download, so it has nothing to do on a Linux composer.
rm -rf "$TMP/mac" && mkdir -p "$TMP/mac"
if [ "$HOST_OS" = "MacOS" ]; then
    ditto -x -k "$TMP/$MAC_ASSET" "$TMP/mac"
    xattr -cr "$TMP/mac/Talos.app"
else
    unzip -q -o "$TMP/$MAC_ASSET" -d "$TMP/mac"
fi

rm -rf "$DEST/MacOS" "$DEST/Windows" "$DEST/Linux"
mkdir -p "$DEST/MacOS" "$DEST/Windows" "$DEST/Linux" "$DEST/.talos"
mv "$TMP/mac/Talos.app" "$DEST/MacOS/Talos.app"
mv "$TMP/$WIN_ASSET" "$DEST/Windows/Talos.exe"
mv "$TMP/$LNX_ASSET" "$DEST/Linux/Talos"
chmod +x "$DEST/Linux/Talos"

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

# Checked by the engine it ships, from where it will be read. NO arguments on purpose:
# the engine resolves the content itself — beside the exe, then one level up, which is
# what this layout is — so a wrong layout fails HERE and not on a colleague's machine.
case "$HOST_OS" in
    MacOS) CHECKER="$DEST/MacOS/Talos.app/Contents/MacOS/Talos" ;;
    Linux) CHECKER="$DEST/Linux/Talos" ;;
esac
# ⚠️ FROM AN EMPTY DIRECTORY, and that is the whole point. The engine's last resort is
# the current directory — the dev fallback, for `cargo run` at the repo root. Run the
# check from the repo and that fallback quietly answers with the REPO's catalog: the kit
# is never read, and a broken layout reports "0 errors". Measured, on the first run of
# this script. An empty cwd leaves the engine only the kit to find.
if ! ( cd "$TMP" && "$CHECKER" --check ); then
    # Two very different causes, and the fix is not the same. Ask the same engine the
    # same question with the paths SPELLED OUT: if it passes that way, the content is
    # sound and what it cannot do is FIND it — an engine older than the OS folders, which
    # only ever looked beside itself. Say which, rather than blame the content.
    if ( cd "$TMP" && "$CHECKER" --check "$DEST/catalog" "$DEST/bundles" ) >/dev/null 2>&1; then
        echo "$TAG ships an engine that predates the kit's OS folders: it reads the content" >&2
        echo "only from beside itself, so it cannot find catalog/ one level up. Compose with a" >&2
        echo "newer --version, or pin one in .talos-version. Kit NOT published." >&2
    else
        echo "the composed kit does not pass Talos --check — kit NOT published" >&2
    fi
    rm -rf "$DEST/.talos"
    exit 1
fi

# What is on disk now, file by file, in the format `-c` reads back — relative to the kit,
# so the manifest travels with the replica. The launchers and the content; never `.talos/`
# itself, which is this bookkeeping.
( cd "$DEST" && { for d in MacOS Windows Linux catalog bundles; do
        [ -d "$d" ] && find "$d" -type f
    done; } | LC_ALL=C sort | while IFS= read -r f; do $SHA "$f"; done ) > "$TMP/MANIFEST.sha256"
mv "$TMP/MANIFEST.sha256" "$DEST/.talos/MANIFEST.sha256"

printf '%s\n' "$TAG" > "$DEST/.talos/VERSION"
{
    echo "talos $TAG"
    echo "composed $(date -u +%Y-%m-%dT%H:%M:%SZ) by get-talos.sh on $(uname -s)"
    echo "release https://github.com/$REPO/releases/tag/$TAG"
    echo
    echo "assets as published (name size sha256), verified at download:"
    awk -v m="$MAC_ASSET" -v w="$WIN_ASSET" -v l="$LNX_ASSET" -v c="$CONTENT_ASSET" \
        '$1 == m || $1 == w || $1 == l || $1 == c { print "  " $0 }' "$TMP/assets.txt"
    echo
    if [ -n "$CONTENT" ]; then echo "content: $CONTENT (the integrator's)"; else echo "content: the socle of $TAG"; fi
    echo
    echo "files as placed: MANIFEST.sha256 — check any replica with: get-talos.sh <kit> --verify"
} > "$DEST/.talos/KIT.txt"
echo "kit at $TAG: MacOS/ + Windows/ + Linux/ + content, checked in place, manifest written"
echo "kit ready in $DEST"
