---
name: talos-content
description: Write and maintain Talos content — the `catalog/` and `bundles/` YAML that a Talos kit reads from disk. Carries the doctrine (detect don't remember, non-interactive commands, names not ids, capability not category, MEASURED means measured), a snippet per route (winget, brew, cargo, npm, bun, run, claude-plugin, skill, vscode-extension, nu-plugin), the config-atom pattern, and the loop that ends at zero errors. Use when adding, editing or reviewing a package or a bundle, when `Talos --check` reports something, when a row shows the wrong version or never turns green, when deciding how to pin a version, or when a package should appear in the Doctor tab.
---

# Talos content

*Two flat folders decide everything Talos proposes.*

The engine is generic — a released binary that knows no team. What makes a kit *yours* is
the `catalog/` and `bundles/` beside it, read from disk at runtime, no build step. Editing
content is editing YAML, and shipping it is a commit.

- **`catalog/`** — one YAML per **package**. The file stem is its id; `name:` is what
  everything else refers to.
- **`bundles/`** — one YAML per **card**. A bundle owns nothing; it pulls packages in by
  `name:`, and `needs:` other bundles.

The complete field-by-field semantics live in
[`references/fields.md`](references/fields.md). Read that when you need to know exactly
what a key does; this file is what to hold in your head while writing.

---

## The loop

```bash
Talos --check catalog bundles     # exit 1 on a broken file, an unknown name, a bad version
```

Write → check → fix → **until it says `0 errors`**. Never hand back content that has not
been through it, and never "fix" a finding by deleting the file that produced it.

⭐ **`--check` is stricter than the runtime, on purpose.** In front of a user, a broken
file is skipped with a log line so one bad row cannot take down the screen. At authoring
time that leniency is a lie: a silently skipped file is a package that quietly stopped
existing. `--check` re-reads the same content with the same parsers and names every
finding by file.

⚠️ **On Windows, redirect the output** — `Talos.exe --check catalog bundles > check.txt`.
The release exe is a GUI program; it prints to a file, not to the bare console. The exit
code is the contract either way.

If Talos is not on PATH, call it where the kit is: `./Talos --check catalog bundles`, or
`Talos.app/Contents/MacOS/Talos --check catalog bundles` on a Mac.

---

## The doctrine

Seven rules the engine is built around. Content that breaks one of them passes `--check`
and still misbehaves on a real machine.

**1 — Detect, don't remember.** `detect:` is observed *now*, on the machine in front of
you; that is the truth. Never write content that assumes a previous run, a sentinel file
or a state database. A package removed by hand must read as absent on the next scan.

**2 — Every command is non-interactive.** The row terminal is display-only: a command
that PROMPTS deadlocks the step, forever, with no way to answer it. Write `brew --yes`,
`winget --accept-package-agreements --accept-source-agreements`, `npx --yes`. If you
cannot make a command silent, it is not a Talos route.

**3 — Names, not ids.** `packages:`, `needs:` and `requires:` all resolve against `name:`
— never against the file stem, never against the route id. `winget: Microsoft.Git` is how
one manager spells it; `Git` is what the content calls it. `--check` refuses a name the
catalog does not declare, which is the whole reason to use names.

**4 — One route per package.** A package is a *need*, satisfied by exactly one named
route. The exception is the two system managers: declare **both** `winget:` and `brew:`
on the same package, and the engine picks the one native to the machine. Two *different*
routes on one package is two packages.

**5 — Capability, not category.** `category:` is a **display** axis and decides nothing —
not an install, not an order, not a rung. Anything the engine acts on is a capability
with its own key. `doctor:` is the worked example: it says this package can be launched
as a rescue session, and it is read by the engine, never shown as a group. If you are
tempted to make behaviour depend on a category, you want a new key.

**6 — MEASURED means measured.** `uac:`, `"403":`, `slow:`, and every field of
`doctor.clean` are assertions about what happens on a real machine. Declare them from an
observation you actually made, on the OS you made it on. A seed asserted too broadly
makes Apply promise "nothing will interrupt you" and then interrupt.

**7 — Every gesture leaves a trace.** Prefer content whose failure is *loud*. A pin the
tap cannot serve, a plugin compiled against another nushell — these fail visibly in the
row's terminal, and that is the honest outcome. Content that fails silently is the bug.

---

## Snippets — one per route

Copy, rename, edit. Almost every route below is shown by a real file in the socle, and
every socle file is written as a lesson — read the one nearest your case before writing.

```yaml
# system managers — declare BOTH ids; the engine picks the native one
name: Git
description: Version control — the ground everything else stands on
winget: Git.Git
brew: git
detect: git --version
version: latest
category: [vcs]
```

```yaml
# cargo — cross-platform; reinstall = upgrade
name: Some Rust tool
cargo: some-crate
detect: some-crate --version
```

```yaml
# npm — global install; it therefore requires Node BY NAME
name: Claude Code
npm: "@anthropic-ai/claude-code"
detect: claude --version
version: latest
requires: [Node.js]
```

```yaml
# bun — global install via `bun add -g` (npm wins if a package declares both)
name: Some bun tool
bun: some-package
detect: some-package --version
```

```yaml
# run — the raw-command escape hatch. Non-interactive or nothing.
# With `check:` and no install route it is a config-atom (see below);
# `runUninstall:` is how it is undone.
name: Git default branch
run: git config --global init.defaultBranch main
check: git config --global --get init.defaultBranch
runUninstall: git config --global --unset init.defaultBranch
requires: [Git]
```

```yaml
# claude-plugin — `plugin@marketplace`; `marketplace:` is registered first.
# `detect:` is the name `claude plugin list` reports, NOT a binary on PATH.
name: Superpowers
claude-plugin: superpowers@claude-plugins-official
marketplace: anthropics/claude-plugins-official
detect: superpowers
requires: [Claude Code]
```

```yaml
# skill — a SKILL.md every agent on the machine reads, via `npx skills`.
# `skillName:` pins the list-name when the display name differs.
name: Writing guidelines
skill: vercel-labs/agent-skills@writing-guidelines
skillName: writing-guidelines
detect: writing-guidelines
requires: [Claude Code, Node.js]
```

```yaml
# vscode-extension — lowercase id, NO `detect:`, NO `version:`,
# and the host declared in `requires:` (the ~/.vscode folder outlives it).
name: Claude Code for VS Code
vscode-extension: anthropic.claude-code
requires: [Visual Studio Code, Claude Code]
```

```yaml
# nu-plugin — the plugin NAME, never the nu_plugin_ binary filename.
# No `plugin-release:` = bundled beside nu (no network). With it = fetched
# from the project's own release, which puts api.github.com on the target's path.
name: Polars for Nushell
nu-plugin: polars
requires: [Nushell]
# with a source:  plugin-release: ChristianLemer/nu_plugin_xlsx@HEAD
```

```yaml
# config-atom — a `run:`/`check:` pair and no install route: it converges a
# FILE instead of installing software. `{dir}` is this folder, absolute —
# in SINGLE quotes: PowerShell interpolates inside double ones, and a `$`
# in the path would vanish.
name: Windows Terminal default shell
run: nu '{dir}/wt-default.nu' apply '{dir}'
check: nu '{dir}/wt-default.nu' check '{dir}'
runUninstall: nu '{dir}/wt-default.nu' uninstall '{dir}'
requires: [Nushell, Windows]
```

```yaml
# a bundle — a card that pulls packages IN, by name
bundle: Manuals
emoji: 📚
usage: Teach the agent how to work, and keep a second one at hand.
highlights: [Superpowers, astral]
description: Plugins and skills that shape how the agent works, plus Copilot CLI.
needs: [Base]
packages:
  - Superpowers
  - astral
```

---

## The three traps that cost the most

**`version:` has three forms, and the words are not decoration.** `"0.113.1"` converges
exactly, `"latest"` is a *decision* to take the newest, `"pending"` is a HOLD — the row
still shows the update and still offers it per-row, but Apply skips it. Omitted means
nobody wrote anything, which behaves as `latest`. Anything else is **refused at load**:
a word has no leading digits, so the comparison would read `0.0.0`, decide the machine is
*above* the pin, and offer a **downgrade** — the only destructive path there is.
**Quote the value.**

**An extension route carries no pin at all.** `claude-plugin`, `skill` and
`vscode-extension` build no upgrade command, so a `version:` on them — keyword included —
produces an action with nothing to run.

**A sidecar is not a package.** `catalog/` holds `.yaml` files *and* the `.nu` scripts a
config-atom calls. `--check` reads only the YAML. When you count packages, count YAML.

---

## When something is wrong

| Symptom | Look at |
|---|---|
| the row never turns green | `detect:` — is exit 0 really "present"? For `claude-plugin` / `skill` it is a **list name**, not a binary |
| the displayed version is wrong | `version-regex:` — override how the version is pulled from the output |
| the row is greyed / indeterminate | an unmet `requires:` — the reason is shown on the row |
| the step hangs forever | the command prompts (rule 2) |
| a downgrade is offered out of nowhere | an invalid `version:` word read as `0.0.0` |
| a `--check` finding names an unknown field | serde ignores it: the field did nothing. Check the spelling against `references/fields.md` |
| a VS Code extension reads present after uninstalling the editor | the host is missing from `requires:` |

Everything else — the full `version:` table, the config-atom contract, `{dir}` expansion,
behaviour seeds and how they ratchet, the nu-plugin version coupling, the legacy
`profiles.yaml` — is in [`references/fields.md`](references/fields.md).
