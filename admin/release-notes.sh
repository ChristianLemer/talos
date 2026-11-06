#!/bin/sh
# release-notes.sh — the body of a release page, on stdout (#5). The release workflow runs it
# once every build is in; run it locally to read what a tag's page will say:
#
#   sh admin/release-notes.sh v0.1.0
#
# Three parts. What Talos is, which file is for which machine, and the one command — fixed,
# because most people who land on a release page never read the README. Then this version's
# section of CHANGELOG.md (`## <version>`), when it has one. Then the commit subjects since
# the previous tag, so a beta with no changelog section still says what it carries.
set -eu

TAG="${1:?usage: release-notes.sh <tag>}"
VER="${TAG#v}"
REPO="ChristianLemer/talos"
DL="https://github.com/$REPO/releases/download/$TAG"

cat <<EOF
**Talos** shows what a machine really has, lets you choose what should be there, and puts it
there — a graphical installer for a team's tools that shows what it does. A team receives a
**kit**: these launchers beside its own \`catalog/\` and \`bundles/\`, in one folder it launches
from a shared drive.

## Compose a kit from this release

\`\`\`bash
# macOS or Linux
curl -fsSL $DL/get-talos.sh | sh -s -- ./Talos --version $TAG
\`\`\`

\`\`\`powershell
# Windows (Windows PowerShell 5.1 is enough)
& ([scriptblock]::Create((irm $DL/get-talos.ps1))) -Kit .\\Talos -Version $TAG
\`\`\`

The script verifies every file against this page's checksums and composes the kit with this
release's example content; \`--content <folder>\` (\`-Content\`) puts yours in its place.

## Which file

| File | For |
|---|---|
| \`Talos-$VER-aarch64-apple-darwin.app.zip\` | macOS, Apple silicon |
| \`Talos-$VER-x86_64-pc-windows-msvc.exe\` | Windows, x64 |
| \`Talos-$VER-x86_64-unknown-linux-gnu\` | Linux, x64 |
| \`talos-content-$VER.zip\` | the example content of this version: \`catalog/\`, \`bundles/\` |
| \`get-talos.sh\`, \`get-talos.ps1\` | compose a kit (above) |

The launchers are not signed yet.
EOF

# This version's section of the changelog: from its `## ` heading to the next one.
if [ -f CHANGELOG.md ]; then
    SECTION="$(awk -v v="$VER" '
        /^## / { on = ($0 ~ "^## \\[?" v "\\]?([ ]|$)") ; if (on) next }
        on' CHANGELOG.md)"
    if [ -n "$(printf '%s' "$SECTION" | tr -d '[:space:]')" ]; then
        printf '\n## What changed\n%s\n' "$SECTION"
    fi
fi

# The commits since the previous tag; all of them for the first one.
PREV="$(git describe --tags --abbrev=0 "$TAG^" 2>/dev/null || true)"
if [ -n "$PREV" ]; then
    printf '\n## Commits since %s\n\n' "$PREV"
    git log --no-merges --format='- %s (%h)' "$PREV..$TAG"
else
    printf '\n## Commits\n\n'
    git log --no-merges --format='- %s (%h)' "$TAG"
fi
