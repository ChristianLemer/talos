---
description: Scaffold a Talos content repo — catalog/, bundles/, .talos-version pinned at the newest release, a CI check, the field reference, and an AGENTS.md so any agent can maintain it.
argument-hint: "[folder — defaults to the current one]"
allowed-tools: Bash, Read, Write, Edit
---

# Scaffold a Talos content repo

Create, in **`$1`** (or the current folder if that is empty), the repo an integrator keeps:
two flat folders of YAML, a one-line pin, a check that runs in CI, and the documents that
let a stranger — human or agent — pick it up.

Work through the steps in order. Announce what you are creating, then create it; do not
ask for confirmation of a step the user already asked for by running this command.

## 1 — Look before you write

```bash
ls -a
```

⚠️ **Never overwrite.** If `catalog/`, `bundles/` or `.talos-version` already exists, stop
and say what is there: this folder is already a content repo, and the user wants
`talos-content` (the skill), not a scaffold. If some *other* files are present that is
fine — a content repo can live beside them.

## 2 — Pin the engine

Read the newest release tag. GitHub's `latest` link **skips pre-releases**, and every
Talos release is a pre-release until v0.1.0, so read the list:

```bash
curl -fsSL https://api.github.com/repos/ChristianLemer/talos/releases \
  | grep -m1 '"tag_name"' | cut -d'"' -f4
```

Write it, one line with a trailing newline, to `.talos-version`. If the call fails
(offline, rate-limited), say so and write nothing — a wrong pin is worse than none, and
the user can supply the tag.

## 3 — The content

Create `catalog/` and `bundles/` with this starter. It is deliberately small: four
packages that show the two system-manager ids, the npm route, an extension route, and a
`requires:` chain — enough that `--check` has something real to say.

`catalog/git.yaml`
```yaml
# Git — THE SYSTEM-MANAGER ROUTE. Declare BOTH ids: the engine picks the one native to
# the machine it runs on. `detect:` is the command whose exit 0 means present; its output
# is also where the installed version is read from.
name: Git
description: Version control — the ground everything else stands on
winget: Git.Git
brew: git
detect: git --version
version: latest
category: [foundation]
```

`catalog/node.yaml`
```yaml
# Node.js — what the npm route stands on. Named here so other packages can `requires:` it
# BY NAME; the file stem is only the id. Current, not LTS: two Node installs confuse the
# PATH. `uac: true` is a SEED — observed on Windows, where this install elevates.
name: Node.js
description: JavaScript runtime — npm, npx and every plugin hook lean on it
winget: OpenJS.NodeJS
brew: node
detect: node --version
version: latest
category: [foundation]
uac: true
```

`catalog/claude-code.yaml`
```yaml
# Claude Code — THE NPM ROUTE: installed globally, so it requires Node BY NAME. A missing
# requirement takes the row out of the perimeter, with the reason shown.
name: Claude Code
description: Anthropic's coding agent, on the command line
npm: "@anthropic-ai/claude-code"
detect: claude --version
version: latest
category: [agents]
requires:
  - Node.js

# THE DOCTOR: this package may be launched from the Doctor tab as a rescue session.
# `clean.env` — a fresh directory through CLAUDE_CONFIG_DIR — is MEASURED on macOS: no
# plugins, no hooks, no memory, and auth survives (credentials come from the keychain).
# A capability, never a category.
doctor:
  clean: { env: CLAUDE_CONFIG_DIR }
```

`catalog/visual-studio-code.yaml`
```yaml
# Visual Studio Code — where you see and edit what the agent produces.
# `uac: true` is a SEED: observed on Windows, where this install demands elevation.
name: Visual Studio Code
description: Where you see and edit what the agent produces
winget: Microsoft.VisualStudioCode
brew: visual-studio-code
detect: code --version
version: latest
category: [editors]
uac: true
```

`bundles/base.yaml`
```yaml
# Base — a BUNDLE is a card: activating it pulls its packages "in" as a group, BY NAME.
# Purely additive — it never forces anything out, and a manual "out" always wins.
bundle: Base
emoji: 🧱
usage: An agent, an editor, and what they stand on.
highlights: [Claude Code, Visual Studio Code]
description: The foundation — Git, Node, the agent, the editor.
packages:
  - Git
  - Node.js
  - Claude Code
  - Visual Studio Code
```

Tell the user, at the end, that the **socle** — the full 19-package lesson set every route
is shown by — is `talos-content.zip` on any release, if they would rather start from that
and prune.

## 4 — The reference, copied not paraphrased

```bash
cp "${CLAUDE_PLUGIN_ROOT}/skills/talos-content/references/fields.md" REFERENCE.md
```

Then prepend to `REFERENCE.md`, above its first line, exactly:

```markdown
<!-- Copied verbatim by /talos:init from the talos plugin. Re-run the copy to refresh. -->
```

⚠️ **Do not rewrite, summarise or reorder it.** One authored source, one verbatim copy —
that is the only reason the copy can be trusted months later. If the `cp` fails because
`CLAUDE_PLUGIN_ROOT` is unset, say so and point at
`https://github.com/ChristianLemer/talos/blob/beta/plugin/skills/talos-content/references/fields.md`
rather than reconstructing the file from memory.

## 5 — `AGENTS.md`

Write it so an agent arriving with no plugin can still work. Keep it short — the detail is
in `REFERENCE.md`:

```markdown
# Working in this repo

This is a **Talos content repo**. It holds no code. `catalog/` and `bundles/` are the YAML
a Talos kit reads from disk at runtime, and `.talos-version` names the engine that reads
them. Changing what Talos proposes is a commit here.

## The loop

Write → `Talos --check catalog bundles` → fix → **until it says `0 errors`**. Never hand
back content that has not been through it, and never resolve a finding by deleting the
file that produced it. `--check` is stricter than the runtime on purpose: at runtime a
broken file is skipped so one bad row cannot take down the screen; at authoring time that
same silence is a package that quietly stopped existing.

On Windows, redirect the output — the release exe is a GUI program and prints to a file:
`Talos.exe --check catalog bundles > check.txt`. The exit code is the contract.

## The doctrine

1. **Detect, don't remember.** `detect:` is observed now. No sentinel, no state database.
2. **Every command is non-interactive.** The row terminal is display-only; a command that
   prompts deadlocks the step forever. `brew --yes`, `winget --accept-*`, `npx --yes`.
3. **Names, not ids.** `packages:`, `needs:` and `requires:` resolve against `name:` —
   never the file stem, never the route id.
4. **One route per package**, except the two system managers: declare both `winget:` and
   `brew:` and the engine picks the native one.
5. **Capability, not category.** `category:` is display only and decides nothing. Anything
   the engine acts on has its own key — `doctor:` is the worked example.
6. **MEASURED means measured.** `uac:`, `"403":`, `slow:` and `doctor.clean` are claims
   about a real machine. Declare them from an observation you actually made.
7. **A sidecar is not a package.** `catalog/` holds `.yaml` and the `.nu` scripts a
   config-atom calls. Count YAML.

## The field reference

`REFERENCE.md` — every key, what it means, and the traps. It is a **verbatim copy** from
the talos plugin; do not edit it, refresh it.

For the full authoring skill: `claude plugin marketplace add github:ChristianLemer/talos`
then `claude plugin install talos@talos`.
```

## 6 — The check, in CI

`.github/workflows/talos-check.yml`

```yaml
# The content is checked by the engine it is pinned to. `--check` reads the same files the
# same way the app would and refuses what the runtime silently skips; the exit code is the
# contract.
name: talos-check

on:
  push:
  pull_request:
  workflow_dispatch:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5

      # The Linux binary is the Tauri app: it links WebKitGTK at load time, so even
      # `--check` — which opens no window — will not start without it. MEASURED with `ldd`
      # on the published binary: libwebkit2gtk-4.1, libsoup-3.0 and libjavascriptcoregtk-4.1,
      # and the first pulls the other two.
      - name: Runtime libs
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-0

      - name: Fetch the pinned engine
        run: |
          TAG=$(cat .talos-version)
          curl -fsSL -o talos \
            "https://github.com/ChristianLemer/talos/releases/download/$TAG/Talos-linux-x86_64"
          chmod +x talos

      - name: Check
        run: ./talos --check catalog bundles
```

## 7 — `README.md` and the repo

Write a short `README.md`: what this repo is (the content half of a Talos kit), the pinned
version, the loop, and the one line that publishes it —
`sh get-talos.sh "<kit folder>" --content .`. Point at `AGENTS.md` and `REFERENCE.md`.

Then, if this is not already a git repo, `git init`. Do **not** commit: leave the tree for
the user to read first.

## 8 — Prove it

Find a Talos binary — on PATH, or `./Talos`, or `Talos.app/Contents/MacOS/Talos`, or the
kit folder the user names. If you find one:

```bash
Talos --check catalog bundles
```

Report the output. It must say `0 errors`; if it does not, **fix the content and run it
again** — that is the whole point of the scaffold.

If no binary is reachable, say so plainly, and give the one line that fetches one for this
OS from the pinned tag. Do not claim the scaffold checks out when you have not run it.

## Finally

Tell the user, in a few lines: the tag pinned, the files created, whether `--check` ran and
what it said, and the two next moves — edit `catalog/` (the `talos-content` skill is the
guide), and `sh get-talos.sh "<kit folder>" --content .` to publish (the `talos-kit`
skill).
