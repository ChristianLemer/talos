# Authoring content — `catalog/` and `bundles/`

These two folders **are** the content Talos proposes. The engine (the exe) is generic
and knows nothing about your team; what makes a kit *yours* lives here. Edit the YAML —
no build step for the content, the exe reads both folders from disk beside it at
runtime. `Talos --check catalog bundles` tells you before the kit ships whether every
file parses and every name resolves.

> New here? Copy a file from `catalog/`, rename it, edit it. Then list its `name:` in
> a bundle. The rest of this file is the reference for when you want to know exactly
> what a field does.

## The shape

```
catalog/
├── git.yaml            ← one package = one file; the stem is its id
├── jq.yaml
├── wt-default.yaml
├── wt-default.nu       ← a config-atom may ship a file beside it (see Config-atoms)
└── …
bundles/
├── README.md           ← you are here
├── base.yaml           ← one bundle = one file: a named card that pulls packages IN
├── documents.yaml      ← `needs: [Base]` — activating it activates Base too
└── …
```

Packages live in the **flat catalog**, once each. Bundles do not own packages: they
**reference** them by `name:`. Several bundles may pull the same package; a package in
no bundle still shows in the Catalog view, just not pulled by any card.

At runtime a broken file is skipped with a log line, not a crash — the panel opens with
whatever parsed. That leniency is for the person in front of the screen; at authoring
time run `--check`, which refuses the same file by name.

## A catalog file — one package

```yaml
name: Visual Studio Code               # what bundles and `requires:` refer to
description: Where you see and edit what the AI produces
winget: Microsoft.VisualStudioCode     # the route + the id that route knows
brew: visual-studio-code               # the id for the OTHER OS (see Routes)
detect: code --version                 # how to tell it's already present
version: latest                        # see Pinning a version
requires: [Git]                        # other packages, by name (see below)
category: [editors]                    # display grouping only
```

`requires:` names packages this one is only offered with; an unmet requirement takes
the row out of the perimeter, with the reason shown. Names, not ids: `--check` refuses
a name the catalog does not declare.

## A bundle file — one card

```yaml
bundle: Manuals
emoji: 📚
usage: Teach the agent how to work, and keep a second one at hand.
highlights: [Superpowers, astral]      # 2-3 names to advertise the card
description: Plugins and skills that shape how the agent works, plus Copilot CLI.
needs: [Base]                          # bundles this one stands on (real cascade)
packages:
  - Superpowers
  - astral
```

A bundle is a **button**: activating it pulls its packages "in" as a group. Purely
additive — it never forces anything out, and a manual "out" always wins. Several can
be active at once; their pulls union. `needs:` chains bundles: the shipped chain is
Base ← Manuals, Plus, Terminal — each of the three `needs: [Base]`.

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
| nu-plugin | `nu-plugin: name` + optional `plugin-release: owner/repo@tag` | a nushell plugin |

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

### `nu-plugin` — the rung follows the source

```yaml
  - name: Polars for Nushell
    description: DataFrames in nushell — the plugin a data manager wants nu for
    nu-plugin: polars
    requires: [Nushell]
```

```yaml
  - name: Excel for Nushell
    description: Read and write .xlsx files from nushell
    nu-plugin: xlsx
    plugin-release: ChristianLemer/nu_plugin_xlsx@HEAD
    requires: [Nushell]
```

- **Write the plugin NAME, never the binary filename.** `nu-plugin: polars` declares the
  plugin named `polars`, and the engine derives `nu_plugin_polars` or `nu_plugin_polars.exe`
  — that `nu_plugin_` prefix is a **nushell requirement** (validated in hard code), not a
  convention. The source name is what you write.
- **The source axis determines the rung.** This is the first route in the catalogue whose
  install rung **depends on the package, not the route name**:
  - **No `plugin-release:` = bundled with nushell** — the binary ships beside `nu`, so
    `plugin add nu_plugin_polars` resolves via `NU_PLUGIN_DIRS` (which contains the
    directory holding `nu`) and engages **no network at all**. ⚡ rung, measured.
  - **`plugin-release: owner/repo@ref` = fetched from the project's release** — the
    sidecar `catalog/nu-plugin-fetch.nu` fetches the project's own `install.nu` at that
    git ref and runs it with `--dir` and `--register`. The installer picks the archive
    built for the Nushell **minor** that runs it (a plugin loads into exactly one), on
    this OS and arch, verifies the published `.sha256`, places and registers the binary.
    🧩 rung, because it fetches. `ref` pins the installer, not the binary: `HEAD` (the
    default when `@ref` is omitted) follows the project, a tag or a sha freezes the
    installer's behaviour. A plugin qualifies by shipping that `install.nu` at its root;
    `nu_plugin_xlsx` is the reference.
  - **Delegation puts `api.github.com` on the TARGET machine's path** — the installer
    lists releases to pick the build. Unauthenticated: 60 calls/hour per address, and a
    fleet behind one corporate NAT shares the address. For a firewall that blocks it,
    the installer's `--archive <path>` installs from a staged local archive, and an atom
    can pass it — no engine change.
- **Presence is a STATUS, not a file.** A plugin binary compiled against a different
  nushell version is registered, its file exists on disk, and it does not work. Talos
  checks `plugin list | where name == <n> and status == loaded` — `added` is not enough.
  The status says whether the plugin protocol agreed.
- ⚠️ **The version coupling is TIGHT and runs BOTH WAYS, and there is no field for it.**
  `catalog/nushell.yaml` pins Nushell at `0.113.1`; a plugin binary is compiled against a
  specific nu-plugin protocol version. They agree **by hand**. Bump either side and
  `plugin add` fails — loudly, with a load error in the row's terminal, which is the honest
  failure. There is no field expressing "requires Nushell 0.113.x": `requires:` says
  PRESENT, not a version constraint.

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
    run: nu '{dir}/wt-default.nu' apply '{dir}'    # apply = open | patch | save
    check: nu '{dir}/wt-default.nu' check '{dir}'  # dry-run: exit 0 = converged, non-zero = drifted
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
- **`doctor:`** — this package may be launched from the **Doctor** tab as a rescue
  session, by absolute path and no shell. A **capability**, never a category: the tab's
  dropdown lists every present package declaring it, in catalogue order, then the OS
  shell (the floor, always there). Optional `clean:` says how it starts WITHOUT its own
  configuration — `{ env: CLAUDE_CONFIG_DIR }` (a fresh per-machine directory through that
  variable) or `{ args: ["-n"] }` — and is what puts a *Launch clean* button beside
  *Launch*. Declare it only on programs you have MEASURED to start that way. The binary is
  the first word of `detect:`; `--check` refuses a `doctor:` without one.
- **`category:`** — one or more tags, e.g. `category: [editors]`. The **first** one groups
  the package under a header in the Catalog view; the rest are shown as tags. Omitted →
  `misc`. ⚠️ It is a *display* axis only: a category never decides an install, an order, or
  a rung.

## Behaviour seeds — `uac:` · `403:` · `slow:`

Three optional booleans describing what installing this package **does to the operator**.
They are read by the Apply panel to warn before it acts, and they are **seeds**: a fresh
machine has collected nothing, so a hand-written value is what calibrates its first Apply
before the fleet's own observations accumulate.

| field | says | example |
|---|---|---|
| `uac: true` | this install **demands elevation** | `visual-studio-code` via winget (observed) |
| `"403": true` | a corporate firewall answers 403 on this download | observed on a real estate; a SITE fact, so the socle declares none |
| `slow: true` | this one takes a long time | — |

⭐ **A boolean, never a duration.** The author says "this one is slow"; the *measured*
seconds come from the machine's own timing file. Declaring a number by hand would invite it
to drift from what the machine actually observes.

⭐ **They are also OVERRIDES, and that is the only way a fact goes back to false.** The
fleet's shared record can only ratchet a fact **on** — an observation proves a wall exists,
never that it is gone. Writing `"403": false` in the catalogue is the one gesture that turns
it off, and it leaves a diff saying who decided.

⚠️ **`403` is quoted for the reader, not out of necessity.** Measured: a bare `403:` parses
identically. The quotes are there because the Rust field behind it is `forbidden` — an
identifier cannot start with a digit — and a reader deserves to see that this is a key, not
a number.

⚠️ Scope these to what you actually saw. `uac: true` on VS Code came from a Windows
observation; the same package via brew on macOS does not elevate. A seed asserted too
broadly makes Apply promise "nothing will interrupt you" and then interrupt.

## `profiles.yaml` — the legacy multi-card file

Before one-file-per-bundle, all cards lived in one `bundles/profiles.yaml` with a
`profiles:` list. The runtime still reads it for back-compat; `--check` refuses it and
asks for one file per bundle, which is what a kit should ship. Convert by moving each
`- profile:` item into its own `bundles/<name>.yaml` with `bundle:` as the key.
