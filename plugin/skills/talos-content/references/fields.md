# The field reference — every key a `catalog/` or `bundles/` file may carry

The complete semantics of the content YAML: every field, what it means, what the engine
does with it, and the traps that cost somebody an afternoon. `SKILL.md` beside this file
carries the doctrine and the snippets; this is where you look when you need to know
*exactly* what a field does.

⭐ **This file is the single source of the reference.** The socle's `bundles/README.md`
points here, and `/talos:init` copies this file into a content repo as `REFERENCE.md` so
an agent without the plugin still has it. A key added to `RawPkg::KNOWN_KEYS` in the
engine and not mentioned here fails `cargo test` — the doc cannot drift from the engine.

## The shape

```
distribution.yaml       ← optional: what this content is called, and which edition (see below)
catalog/
├── git.yaml            ← one package = one file; the stem is its id
├── jq.yaml
├── wt-default.yaml
├── wt-default.nu       ← a config-atom may ship a file beside it (see Config-atoms)
└── …
bundles/
├── README.md           ← a pointer to this reference, shipped with the socle
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
default: on                            # checked on a FIRST launch; omit for off
packages:
  - Superpowers
  - astral
```

A bundle is a **button**: activating it pulls its packages "in" as a group. Purely
additive — it never forces anything out, and a manual "out" always wins. Several can
be active at once; their pulls union. `needs:` chains bundles: the shipped chain is
Base ← Manuals, Plus, Terminal — each of the three `needs: [Base]`.

`default: on` makes the card **checked when Talos opens on a machine for the first
time** — that is, when no selection has ever been saved there (`selection.json` in the
local data dir does not exist). What it does, exactly:

- **It pulls its `needs:`**, as a click on the card does: `default: on` on Manuals
  opens with Manuals *and* Base checked.
- **It is a proposal, not a lock.** The user can uncheck the card; the first gesture —
  a click on a card, a setting, or Apply — saves the selection, and from then on the
  SAVED selection decides, on every launch —
  an unchecked default card stays unchecked. A machine that already has a selection
  sees no change when you add `default: on` to a bundle.
- **Reset returns to it.** Reset puts the default cards back, as on a first launch.
- **Nothing is installed by it.** A checked card is intention; only Apply acts.
- **Bundles are read at launch.** Refresh re-reads the catalogue, not `bundles/`: to see
  an edited `default:` (or any bundle change), close Talos and open it again.
- **Values: `on` or `off`** (`off` is the same as omitting the key). Write `on`, not
  `true` or `yes`: the runtime reads any other value as off, and `--check` refuses it.

⚠️ **There is no bundle-level `posture:`.** An older version of this reference
described one (`mandatory` / `opt-out` / `opt-in` / `forbidden`); the engine never read
it. `--check` now names any key a bundle file does not support, `posture:` included.

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
| copilot-plugin | `copilot-plugin: plugin@marketplace` + optional `marketplace:`, or a direct `owner/repo[:path]` + `detect:` | Copilot CLI plugin (same format as Claude's) |
| skill | `skill: source` + optional `skillName:` | cross-agent SKILL.md via `npx skills` |
| vscode-extension | `vscode-extension: publisher.name` | a VS Code extension |
| nu-plugin | `nu-plugin: name` + optional `plugin-release: owner/repo@tag` | a nushell plugin |

**`detect:`** — how Talos knows a package is already present.
- A binary route: the command whose exit 0 = present, e.g. `detect: code --version`.
  Its output also yields the installed version (free, source-agnostic — the best signal).
- `claude-plugin` / `skill`: `detect:` is the **name the tool lists the item under**
  (not a PATH binary) — Talos parses `claude plugin list` / `npx skills list`.
- `copilot-plugin`: Talos reads Copilot's own record (`~/.copilot/config.json`, or
  `$COPILOT_HOME/config.json`). A plugin switched off in Copilot reads **absent**, with
  the reason on the row: Apply's install turns it back on. The marketplace form **omits
  `detect:`**: the id is what Copilot lists it under. The **direct** form (`owner/repo`,
  `owner/repo:path`, a git URL, a local folder) **must declare it**, as the name in the plugin's own manifest: Copilot records a direct
  plugin with an empty marketplace, by that name alone. `--check` refuses a direct source
  without it.
- `vscode-extension`: **omit it.** The id you declared *is* the lookup name, so writing
  it twice is a second place to get it wrong. Talos reads the profile manifest
  (`~/.vscode/extensions/extensions.json`) — never `code --list-extensions`, whose output
  is polluted on Windows, and never the sub-directories, which **survive an uninstall**.

### `copilot-plugin` — the same plugins, another host

Copilot CLI reads the plugin and marketplace format Claude Code does, so a plugin written
for one installs into the other. It is a route of its own, not a flag on `claude-plugin`:
one key, one host, like `vscode-extension`.

```yaml
# Marketplace form: the id is `plugin@<name marketplace.json announces>`.
name: Chiron
copilot-plugin: chiron@tekton-beta
marketplace: ChristianLemer/tekton#beta
requires: [Copilot CLI, Git]
```

```yaml
# Direct form: a repository or a folder of one. `detect:` is the plugin's own name.
name: Markdown Converter
copilot-plugin: intellectronica/agent-skills:plugins/markdown-converter
detect: markdown-converter
requires: [Copilot CLI, Git, uv]
```

- Install is `marketplace add` (if declared), then `install`, then `update` — measured
  non-interactive on both forms, on Linux and Windows. A re-Apply prints *"already
  registered"* from the `add`: expected, it does not fail the row.
- **Git is a real prerequisite**: Copilot clones with the machine's `git`, over HTTPS.
- Every value is passed in single quotes, so a local folder may hold a space; a `'` in
  `copilot-plugin:`, `marketplace:` or `detect:` is refused by `--check`.
- Copilot warns that direct installs are deprecated in favour of `plugin@marketplace`.
  They still work (1.0.103); prefer a marketplace when one exists.

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

### `nu-plugin` — the network follows the source

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
- **The source axis determines what the install touches.** This is the first route in the
  catalogue whose cost **depends on the package, not the route name** — and the row's glyph
  follows it (⚡ or 🧩):
  - **No `plugin-release:` = bundled with nushell** — the binary ships beside `nu`, so
    `plugin add nu_plugin_polars` resolves in the directory of the running `nu` and
    engages **no network at all**. ⚡, measured. The path nushell RECORDS is the
    resolved one — under brew, `Cellar/nushell/<version>/…` — and `plugin add`
    canonicalises any path it is given, so the stable `/opt/homebrew/bin/` link cannot be
    recorded instead. Each Nushell upgrade therefore kills the registration; the Apply
    that upgraded Nushell sees it and re-registers (below).
  - **`plugin-release: owner/repo@ref` = fetched from the project's release** — the
    sidecar `catalog/nu-plugin-fetch.nu` fetches the project's own `install.nu` at that
    git ref and runs it with `--dir` and `--register`. The installer picks the archive
    built for the Nushell **minor** that runs it (a plugin loads into exactly one), on
    this OS and arch, verifies the published `.sha256`, places and registers the binary.
    🧩, because it fetches. `ref` pins the installer, not the binary: `HEAD` (the
    default when `@ref` is omitted) follows the project, a tag or a sha freezes the
    installer's behaviour. A plugin qualifies by shipping that `install.nu` at its root;
    `nu_plugin_xlsx` is the reference.
  - **Delegation puts `api.github.com` on the TARGET machine's path** — the installer
    lists releases to pick the build. Unauthenticated: 60 calls/hour per address, and a
    fleet behind one corporate NAT shares the address. For a firewall that blocks it,
    the installer's `--archive <path>` installs from a staged local archive, and an atom
    can pass it — no engine change.
- **Presence is a LOAD, not a status.** Talos hands the registered file to
  `plugin add --plugin-config <throwaway>`, which STARTS the plugin and applies nushell's
  own compatibility check, then discards the throwaway registry. Four verdicts:
  not registered → absent; registered but nushell refuses to load it → **absent**, with
  nushell's error as the evidence; loads but **stale** → absent; loads → present. `nu`
  missing → indeterminate. Talos judges no compatibility itself: which minor a plugin loads
  into is nushell's rule. *Stale* compares two records nushell wrote — the version in the
  user's registry and the one the throwaway registry just read from the binary on disk.
  winget upgrades Nushell by replacing the bundled `nu_plugin_*.exe` in place: the file
  loads, but the registry keeps the old version and the old signatures, so a command the
  new plugin adds stays invisible until `plugin add` runs again.
  `plugin list`'s `loaded` is NOT this — it means the signatures are in the registry, the
  binary is never run, and it stays `loaded` for a deleted file or a plugin built for
  another minor (measured, 2026-10-18).
- ⚠️ **A Nushell upgrade breaks plugin rows, and the SAME Apply repairs them.** A bundled
  plugin's recorded path dies with the old Cellar (brew), or its file is replaced in place
  and its registration goes stale (winget); a released one was compiled for the old minor. Once the step that upgraded Nushell has
  changed its version, Apply re-probes its dependents and installs again the ones that no
  longer load (see `requires:` below): the bundled line re-registers the binary shipped with
  the new nu, the sidecar re-fetches the build for the running minor. Measured in a Windows
  VM: Nushell 0.113.1 → 0.116.1 and Excel for Nushell repaired, in ONE Apply. There is no field expressing
  "requires Nushell 0.116.x" — `requires:` says PRESENT, not a version constraint — and
  none is needed: the load probe is the constraint, checked on the machine.

**`requires:`** — names a package (or a bare command) that must be present first.
Resolved against the whole plan: a plugin that `requires: [Claude Code]` stays
indeterminate until the agent is present or wanted, and Apply installs the dependency
first (topological order). Example: `requires: [Nushell, Claude Code]`.
It also works in the other direction, once: when an Apply step CHANGES a package (its
version or its presence, observed after the step, not merely "a step ran"), the packages
that `requires:` it directly are probed again, and those the user wants present that now
read absent are installed in that same Apply. Rows left on Auto are repainted, never
acted on; a repaired row does not re-check its own dependents.

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
| above the pin | **downgrade** — uninstall + reinstall at the pin, in Apply |
| absent | install at the pin |

An exact pin below what is installed is honoured by the Apply: the package is
uninstalled and reinstalled at the pin. The version belongs to the author — whoever wants
*their own* version takes the row out of Talos's hands. A route that cannot build a
downgrade leaves the row as it is and says so on it (`held`).

**Per-route reality:** winget / cargo / bun pin natively (`--version` / `@version`).
**brew has no version flag**: it installs its *current* version of a formula, or a
versioned formula `id@version` when a tap ships one. So when a brew pin has to move, the
Apply first asks brew (`brew info`, for pinned brew rows only, never in the scan):

| brew offers | What the Apply does |
|---|---|
| the pin is its current version, at the pin's precision | plain `brew install` (it upgrades an outdated formula too) |
| a versioned formula `id@version` (e.g. `node@24` for `version: "24"`) | installs that formula |
| neither | **nothing** — the row is `held` and says *brew offers X, not Y* |
| a name that is a cask as well (`wireshark`), or no readable answer | the command it always ran |

brew's revision suffix is not a version: `0.116.1_1` (a rebuild) **is** the pin `0.116.1`.
Only an exact pin asks brew: a `self` row is never upgraded by brew, whatever brew offers.
A plain brew change is judged like any other: exit 0 with the pin still not
installed reads *unchanged*, and the Apply leaves the row until the version or the pin moves.
A held row is not a failure, and the Apply button stops offering it until a Refresh. Each
Apply that wants the row asks brew again, so once brew reaches the pin, the next Apply installs it. While you sit *at* the pin, nothing runs and brew is not asked, so a newer brew
release is harmless until you actually drift. An exact pin that brew cannot serve also
holds an **absent** package — it is not installed at another version — so a package
pinned for winget's cadence and `brew:` on the side stays off the Mac until brew reaches
the pin — and what `requires:` it stays off with it, saying so. ⚠️ A versioned formula is another name to brew: with `node@24` installed,
`brew list --versions node` answers *not installed*. Declare a `detect:` (`node --version`),
which finds the binary whatever formula put it there.

⚠️ **A downgrade can still end with the package removed**: its uninstall succeeded and the
reinstall then failed — a 403 on a corporate network, a winget source that no longer
serves that version. The row says *failed*; **Retry** (or the next Apply) installs it at
the pin, since the machine now reads absent.

There's no "floor" mode — an exact pin is exact, **at the precision you write**: the
installed version is compared on as many segments as the pin has, so `2.50.1.windows.1`
satisfies `version: "2.50.1"` and `1.8.2` satisfies `version: "1.8"`. Write more segments to
pin tighter.

### Where a winget package lives — `scope:`

`scope: user` or `scope: machine` (winget only) decides where the package is installed:
every install, upgrade, pinned install and downgrade reinstall passes `--scope`, and a copy
found elsewhere is **moved**, never duplicated. Just before an upgrade or a downgrade, Talos
reads where the installed copy is, and the row's terminal says what it found:

| the copy is… | upgrade | downgrade |
|---|---|---|
| already in the declared scope | `winget install --scope` — in place | uninstall, then reinstall at the pin there |
| elsewhere, or where cannot be read | uninstall, then install there | the same |
| not installed any more | — | install at the pin there (nothing to uninstall) |

⚠️ "Cannot be read" has a price: as long as the copy's location stays unreadable (an empty
InstallLocation is common for MSI packages), **every** upgrade of a package with `scope:`
uninstalls before it installs. A listing winget could not produce changes nothing: the
stored command runs, as without `scope:` probing.

Why not `winget upgrade`, and why not always install in place — all MEASURED on Nushell's
MSI: `winget upgrade` on a per-user copy put the new version machine-wide beside it;
`winget upgrade --scope user` did not even find it (winget files that copy as machine); and
`winget install --scope user` over a **machine-wide** copy added a per-user one beside it.

Without `scope:`, a downgrade reinstalls where the installed copy was, read from the
InstallLocation of its uninstall entry: under the profile, LOCALAPPDATA or APPDATA →
`user`; under Program Files, ProgramData or Windows → `machine`; anywhere else → unknown,
and winget's own default is used (the row says so). Never a guess: a wrong scope would fail
the reinstall after the uninstall had run. A retry that finds the package gone reinstalls it
where it was found. An undeclared upgrade stays a plain `winget upgrade`.

⚠️ Declare only a scope the package's installer offers: many winget packages are
machine-only, and `--scope user` fails on them. A wrong word, or `scope:` on a package with
no `winget:` id, is a `--check` error.

### `version:` takes four forms, and the three words are not decoration

| written | meaning | what Apply does |
|---|---|---|
| `version: "0.113.1"` | **exactly that one** | converges to it (upgrade / downgrade / satisfied) |
| `version: "latest"` | **I decided: the newest** | upgrades when the manager reports it outdated |
| `version: "pending"` | **nobody has decided yet** | **nothing** — the row is held out of the batch |
| `version: "self"` | **it updates itself** | installs it when absent; never upgrades it through the manager (see below) |
| *(omitted)* | nobody wrote anything | same as `latest` |

Whatever the word, a package that is **absent** and wanted is installed: there is no held
version of a thing that is not there.

⭐ **`latest` is not redundant with omitting the field.** It is the difference between an
intention and an absence of decision — and in a catalogue meant to be copied from, a
maintainer must be able to say "yes, newest here" rather than merely leave a blank.

**`pending` is a HOLD, not a blindfold.** The row still *shows* the newer version, greyed,
with the word `arbitration` beside it, and the Arbitration tab lists it. Apply does not
update it — the decision is an edit to the pin, not a click. Use it when a package costs enough to
install that somebody should decide, and nobody has yet. Replace it with `latest` or an
exact version once the decision is made — then
`grep -c 'version: "pending"' catalog/*.yaml` is the work still left.

### `version: "self"` and `update:` — an application that updates itself

Some applications rewrite themselves (VS Code, Electron apps such as Orca, CLIs with a
`<tool> update`). Upgrading them through the manager breaks in two measured ways: winget
"succeeds" without changing anything (VS Code, every Apply), or refuses to touch a copy the
tool rewrote (a portable package). `self` says the decision is made — leave it to the
application — so the row is **not** arbitration and shows one number with `updates itself`.

```yaml
  - name: Copilot CLI
    winget: GitHub.Copilot
    brew: copilot-cli
    detect: copilot --version
    version: "self"
    update: copilot update      # optional: the tool's own update command
```

- **Absent** → installed by the manager, as usual.
- **Present, no `update:`** → nothing. The application keeps itself current, when it decides to.
- **Present, with `update:`** → the Apply runs that command, and nothing else, when the version
  `detect:` reads is **behind** what the manager announces. The manager's own receipt does not
  count: the tool rewrote itself behind it. So the trigger is the manager's catalogue, read on
  every scan, never a network call of Talos's own. A version `detect:` cannot read is no
  evidence: the command does not run.
- `update:` therefore needs `detect:` (without it the version read is the manager's stale
  receipt) and a `winget:` or `brew:` id (nothing else announces a newer version). `--check`
  names either one missing.
- **`update-on: apply`** → the Apply runs `update:` on **every** Apply while the package is
  wanted and present, behind or not: the command itself knows when there is nothing to do.
  The route does not matter — winget, brew, npm alike install it; what makes the row is that
  the tool updates itself and has a command to trigger it. No announcement is needed, so
  neither `detect:` nor a `winget:`/`brew:` id is. Such a row is never judged `unchanged`:
  "nothing to do" is its usual answer. The price is that the Apply always has something to
  run, so it never says the machine already matches while such a row is wanted. A package
  the manager upgrades normally needs none of this: no `self`, no `update-on`.
- Where an extension route wins (`claude-plugin`, `copilot-plugin`, `skill`,
  `vscode-extension`, a released `nu-plugin`), the package carries no version, so `self` and
  `update:` do not apply there and the route keeps its own update. `--check` names a package
  where that is so on every OS.
- The command must fit how the tool is installed on the machine: `claude update` updates an
  npm or native install and does nothing under mise (measured), where `mise upgrade claude`
  is the one.
- `update:` goes through the network on the tool's own terms: a failure is a line on the row,
  never an Apply paused on a 403. It must be non-interactive, like every command.
- `update:` without `version: "self"` would never run; `--check` says so.

Brew casks declaring `auto_updates true` are **not** read as `self`: Talos scans brew with
`--greedy-auto-updates` on purpose, so such a cask is upgraded unless its file says `self`.

### A version change is a success when the version moved

After an upgrade or a downgrade that exited 0, Talos reads the version again. Toward an exact
pin, the step succeeded when the pin is read (at its own precision); otherwise when the version
changed, when it is at or past the version announced, or when the manager's own copy moved
(`detect:` may read another copy that comes first on `PATH`). A leading `v` is a spelling, not
a segment: `v1.0.96` is `1.0.96` in every comparison. If not, the row says so (`unchanged`, and a line such as *winget reported success, but
1.141.2 is still installed, not 1.142.0*), the journal records `ok: false` with `found`,
`target`, `exit: 0` and the tool's last lines (`tail`), and the Apply does not run that change again while the same version faces the same
target — a newer announcement, an edited pin, another command (the row moved to its own
`update:`), or the version moving (by hand, or by the tool itself) lifts it. The memory is the running Talos's: a restart
tries once more. A version that cannot be read gives no verdict.

A row whose tool already reads the announced version while the manager still lists it as
outdated (its record lags: the tool updated itself) shows one number and `record <version>`:
the Apply still runs the manager's upgrade, which realigns that record. If that upgrade moves
nothing — not what runs, not the manager's record — running it again would change nothing
either: the row is a success, says `current`, and the Apply leaves it like a held row.

winget refuses to upgrade a **portable** package whose files changed since it installed them —
what a tool that updates itself in winget's folder leaves (*Unable to remove Portable package
as it has been modified*). Talos then reads the version: at or below the announcement, it runs
the upgrade again with `--force`, which takes the tool there and realigns winget's record; past
it, forcing would go back, so the row is left as it is, says `ahead`, and the Apply leaves it
until the tool moves or a newer version is announced. Only a version read from the tool itself
(`detect:`) may force: without one, the version read is winget's own record, behind by
definition. And only a plain `winget upgrade` takes the flag, never an author's compound
`upgrade:`. Both are successes in the journal.
`version: "self"` keeps winget away from such a tool altogether.

⚠️ **Anything else is REFUSED at load**, named in the log, and the package behaves as if
nothing was declared. That guard is not politeness: a word has no leading digits, so the
version comparison reads it as `0.0.0`, concludes the machine is *above* the pin, and
**downgrades** it on the next Apply — an uninstall + reinstall nobody asked for. A typo used
to be enough.

⚠️ **An extension route (`claude-plugin`, `copilot-plugin`, `skill`, `vscode-extension`) carries no pin at
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
  ⚠️ **Quote it SINGLY.** PowerShell interpolates inside double quotes, so a `$` anywhere
  in the expanded path would vanish before the script is called.
- `check:` is the apply *replayed in memory without saving*, so detection can't drift
  from application. "Would applying change something?" → non-zero → the row reads absent
  → Apply reconverges. No sentinel, no state DB.
- Write the patch so it **upserts only your keys** (a record→record transform): the
  user's own edits pass through untouched. That's why config-atoms usually have no
  uninstall — a leftover config is inert, and removing it could destroy the user's edits.

## `distribution.yaml` — the name of the content

The engine is one binary for everyone; what makes a kit *yours* is the content. This
file, beside `catalog/` and `bundles/`, gives that content a name — and the window title
leads with it, before the engine's own version:

```yaml
name: Data management        # required: what the people in front of it call it
version: 2026.10 — Saul      # optional free text: a number, a date, a codename
```

Title: `Data management 2026.10 — Saul · Talos v0.0.1-beta.44`. Without the file, the
title is `Talos · v0.0.1-beta.44`, exactly as before — the file is optional in both
directions, and an engine older than it never looks for it.

- **`name:`** — required, not blank. A display name, spaces welcome.
- **`version:`** — the edition of **your content**, nothing to do with the engine's tag,
  which the title shows beside it anyway. Free text: `2026.10`, `3`, `Autumn`. Omit it
  rather than repeat the engine's version.

`--check` reads it strictly — a blank name or an unknown key is a finding named
`distribution.yaml: …` — and prints `distribution: <name> <version>` before its summary.
The composing scripts (`get-talos.sh`, `get-talos.ps1`) copy the file into the kit beside
the two folders and list it in the manifest; that line in their `--check` output is the
proof it landed where the engine reads it. At runtime a broken file is simply no name.

## Rare fields

- **`id:`** — the stable id, which otherwise **is the file stem**. Accepted by the engine
  and by `--check`, declared by no shipped file, and best left alone: the id names files
  beside the catalog entry (`behaviour/<id>.yaml`), so it is not merely a map key. A blank
  `id:` falls back to the stem rather than writing `behaviour/.yaml`.
- **`version-regex:`** — override how the version is pulled from `detect`/route output,
  when the per-route default gets it wrong. Capture group 1 (or the whole match) is the
  version. Pure JS regex, no shell. Add it only when the *displayed* version is wrong —
  that's the diagnostic that tells you which package needs one.
- **`skillName:`** — the list-name for a `skill` when it differs from the display `name`
  (used to target uninstall/upgrade).
- **`marketplace:`** — for `claude-plugin`, the source registered (`claude plugin
  marketplace add`) before install. Omit when the plugin comes from an already-known
  marketplace. A local path with spaces is fine — it's quoted at the call site.
  For `copilot-plugin` too (`copilot plugin marketplace add`), with two measured
  differences: a **branch** is written `owner/repo#ref`, and Copilot registers the
  marketplace under the name the `marketplace.json` **on that branch** announces —
  `ChristianLemer/tekton` is `tekton`, `ChristianLemer/tekton#beta` is `tekton-beta`. Read
  the name there, never guess it from the repository. Never on a direct source: it
  registers nothing, and `--check` says so.
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
  `misc`. ⚠️ It is a *display* axis only: a category never decides an install or an
  order.

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
