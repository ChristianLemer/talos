# Authoring bundles

This folder **is** the content Talos proposes. The engine (the exe) is generic and
knows nothing about your team; what makes a kit *yours* lives here. Drop folders in,
remove the ones you don't want, edit the YAML — no build step for the content, the
exe reads `bundles/` from disk beside it at runtime.

> New here? Copy an existing bundle folder, rename it, edit its `bundle.yaml`. The
> rest of this file is the reference for when you want to know exactly what a field does.

## The shape

```
bundles/
├── README.md          ← you are here
├── profiles.yaml      ← optional: named, additive selections (see bottom)
├── base/
│   └── bundle.yaml     ← one bundle = one folder with a bundle.yaml
├── terminal/
│   ├── bundle.yaml
│   └── starship.nu     ← a config-atom may ship files beside its manifest
└── …
```

A subfolder is a bundle **only** if it contains a `bundle.yaml`. Anything else is
ignored (that's why `profiles.yaml` can sit at the root without being mistaken for a
bundle). A broken YAML is skipped with a log line, not a crash — the panel opens with
whatever parsed.

## A bundle.yaml

```yaml
bundle: Editors            # display name (falls back to the folder name)
emoji: ✏️
description: Where you see and edit what the AI produces.
priority: 10               # screen order — and the order Apply acts in (low = first)
selectable: true           # can the whole card be toggled at once?
posture: opt-out           # DEFAULT posture inherited by every package below

packages:
  - name: Visual Studio Code
    description: Where you see and edit what the AI produces
    winget: Microsoft.VisualStudioCode   # the route + the id that route knows
    brew: visual-studio-code             # the id for the OTHER OS (see Routes)
    detect: code --version               # how to tell it's already present
```

**Posture is a bundle-level policy**, inherited uniformly by every package. A bundle
that mixes opt-in and opt-out packages makes its own card lie — if you need both,
make two bundles.

| Posture | Default | User can change? |
|---|---|---|
| `mandatory` | on | no (locked on) |
| `opt-out` | on | yes — decline it |
| `opt-in` | off | yes — add it |
| `forbidden` | off | no (locked off) |

## Routes — a package is a *need*, satisfied by one named route

Declare **one** route per package. On a system-manager route, declare *both* ids
(`winget` + `brew`); the engine picks the one native to the machine it runs on.

| Route | Field(s) | Notes |
|---|---|---|
| winget | `winget: Publisher.Id` | Windows system manager |
| brew | `brew: formula-or-cask` | macOS/Linux system manager |
| cargo | `cargo: crate` | cross-platform; reinstall = upgrade |
| npm | `npm: pkg` + optional `npmFlags:` | global install via `npm install -g` (ships with Node) |
| bun | `bun: pkg` | global install via `bun add -g`; declared by nothing today (npm wins if both) |
| run | `run:` + optional `runUninstall:` | raw command escape hatch |
| claude-plugin | `claude-plugin: plugin@marketplace` + optional `marketplace:` | Claude Code plugin |
| skill | `skill: source` + optional `skillName:` | cross-agent SKILL.md via `npx skills` |
| vscode-extension | `vscode-extension: publisher.name` | a VS Code extension |

**`detect:`** — how Talos knows a package is already present.
- A binary route: the command whose exit 0 = present, e.g. `detect: code --version`.
  Its output also yields the installed version (free, source-agnostic — the best signal).
- `claude-plugin` / `skill`: `detect:` is the **name the tool lists the item under**
  (not a PATH binary) — Talos parses `claude plugin list` / `npx skills list`.
- `vscode-extension`: **omit it.** The id you declared *is* the lookup name, so writing
  it twice is a second place to get it wrong. Talos reads the profile manifest
  (`~/.vscode/extensions/extensions.json`) — never `code --list-extensions`, whose output
  is polluted on Windows, and never the sub-directories, which **survive an uninstall**.

### `vscode-extension` — the host answers first

```yaml
  - name: Nushell language support
    vscode-extension: thenuprojectcontributors.vscode-nushell-lang
    requires: [Visual Studio Code, Nushell]
```

- **Write the id lowercase.** The marketplace page spells the publisher
  `TheNuProjectContributors`; the manifest lowercases every id. The lookup is
  case-insensitive so either works, but the lowercase form is what `code
  --list-extensions` prints.
- **Declare the host in `requires:`.** `~/.vscode/` is a *user* folder that survives
  uninstalling VS Code, so a manifest hit alone proves nothing: presence = **the host
  answered AND the manifest lists the id**. Without the host, the row reads
  indeterminate, not absent.
- **No `version:`** — `code --install-extension id@1.2.3` exits 1 once the gallery stops
  serving that version, so a pin here builds a command designed to fail.
- **No upgrade, deliberately.** VS Code auto-updates its own extensions
  (`extensions.autoUpdate` defaults to true), so Talos installs and removes; keeping
  them current is the editor's job.

**`requires:`** — names a package (or a bare command) that must be present first.
Resolved against the whole plan: a plugin that `requires: [Claude Code]` stays
indeterminate until the agent is present or wanted, and Apply installs the dependency
first (topological order). Example: `requires: [Nushell, Claude Code]`.

## Pinning a version — `version:`

Add `version:` to make an **exact** version the reference. Talos then compares what's
installed to the pin and acts by direction:

```yaml
  - name: Nushell
    brew: nushell
    winget: Nushell.Nushell
    detect: nu --version
    version: "0.113.1"       # the reference (quote it — YAML would read 1.20 as a float)
```

| Installed vs pin | What happens |
|---|---|
| below the pin | **upgrade** to the pin — runs in Apply |
| at the pin | satisfied (green), nothing to do |
| above the pin | **downgrade** button on the row — **manual only, never in Apply** |
| absent | install at the pin |

Downgrade is deliberately excluded from Apply: it's the only destructive path
(uninstall + reinstall), so it's a single deliberate click on the row, never batched.

**Per-route reality:** winget / cargo / bun pin natively (`--version` / `@version`).
**brew has no version flag** — a pin resolves to the versioned formula `id@version`
(e.g. `nushell@0.113.1`), which exists *only* if the tap ships it. If it doesn't, a
pinned install/downgrade fails at run time and the row simply stays as it was (Talos
re-scans and shows the real state — it never lies about what's on the machine). While
you sit *at* the pin, nothing runs, so an unavailable versioned formula is harmless
until you actually drift.

There's no "floor" mode — an exact pin is exact.

### `version:` takes three forms, and the two words are not decoration

| written | meaning | what Apply does |
|---|---|---|
| `version: "0.113.1"` | **exactly that one** | converges to it (upgrade / downgrade / satisfied) |
| `version: "latest"` | **I decided: the newest** | upgrades when the manager reports it outdated |
| `version: "pending"` | **nobody has decided yet** | **nothing** — the row is held out of the batch |
| *(omitted)* | nobody wrote anything | same as `latest` |

⭐ **`latest` is not redundant with omitting the field.** It is the difference between an
intention and an absence of decision — and in a catalogue meant to be copied from, a
maintainer must be able to say "yes, newest here" rather than merely leave a blank.

**`pending` is a HOLD, not a blindfold.** The row still *shows* the newer version, greyed,
with the word `arbitration` beside it; the **per-row button still offers the update**. What
disappears is only the batched action in Apply. Use it when a package costs enough to
install that somebody should decide, and nobody has yet. Replace it with `latest` or an
exact version once the decision is made — then
`grep -c 'version: "pending"' catalog/*.yaml` is the work still left.

⚠️ **Anything else is REFUSED at load**, named in the log, and the package behaves as if
nothing was declared. That guard is not politeness: a word has no leading digits, so the
version comparison reads it as `0.0.0`, concludes the machine is *above* the pin, and
offers a **downgrade** — the only destructive path there is. A typo used to be enough.

⚠️ **An extension route (`claude-plugin`, `skill`, `vscode-extension`) carries no pin at
all**, keyword included. None of them builds an upgrade command, so a pin would produce an
action with nothing to run — a row skipped in silence, which this engine refuses.

⚠️ Quote the value. Not because the parser needs it (the field is typed as a string, so
`1.10` will not be read as a float here), but because the field now holds two natures —
a number and a word — and the quotes say which one you meant.

## Config-atoms — shipping a config, not a package

A package with a `run:`/`check:` pair and no install route is a **config-atom**: it
converges a file instead of installing software. The pattern (see `terminal/`):

```yaml
  - name: Starship config
    description: A sensible Starship prompt config the family ships
    run: nu "{dir}/starship.nu" apply    # apply = open | patch | save
    check: nu "{dir}/starship.nu" check  # dry-run: exit 0 = converged, non-zero = drifted
    requires: [Starship]
```

- `{dir}` expands to this bundle's own folder (absolute), so an atom can call a script
  it ships beside the manifest — quotes never have to survive the shell command line.
- `check:` is the apply *replayed in memory without saving*, so detection can't drift
  from application. "Would applying change something?" → non-zero → the row reads absent
  → Apply reconverges. No sentinel, no state DB.
- Write the patch so it **upserts only your keys** (a record→record transform): the
  user's own edits pass through untouched. That's why config-atoms usually have no
  uninstall — a leftover config is inert, and removing it could destroy the user's edits.

## Rare fields

- **`version-regex:`** — override how the version is pulled from `detect`/route output,
  when the per-route default gets it wrong. Capture group 1 (or the whole match) is the
  version. Pure JS regex, no shell. Add it only when the *displayed* version is wrong —
  that's the diagnostic that tells you which package needs one.
- **`skillName:`** — the list-name for a `skill` when it differs from the display `name`
  (used to target uninstall/upgrade).
- **`marketplace:`** — for `claude-plugin`, the source registered (`claude plugin
  marketplace add`) before install. Omit when the plugin comes from an already-known
  marketplace. A local path with spaces is fine — it's quoted at the call site.

## profiles.yaml — named, additive selections

Optional, single file at the root of `bundles/`. A profile is a **button**: applying it
pulls its packages "in" as a group. Purely additive — it never forces anything out, and
a manual "out" always wins (that's what turns a profile "hollow"). Several can be active
at once; their pulls union. Profiles reference packages by their `name`.

```yaml
columns: 2                 # grid width of the profile bar

profiles:
  - profile: Essential AI
    emoji: 🌱
    usage: Talk to an AI agent and read its work in a proper editor.
    highlights: [Claude Code, VS Code]   # 2-3 names to advertise the profile
    description: The minimum — an AI agent you can talk to, and a place to read it.
    packages:
      # Bun is not listed — Claude Code pulls it in via `requires: Bun`.
      - Claude Code
      - Visual Studio Code
```

An unknown package name in a profile simply pulls nothing — harmless, so profiles stay
decoupled from the exact bundle set.
