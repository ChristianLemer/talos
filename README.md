# Talos

**Talos shows what a machine really has, lets you choose what should be there, and puts it
there — showing every step.** A graphical installer for a team's tools, on Windows (winget)
and macOS (brew), that a whole team launches from one shared folder.

## Try it

```bash
# macOS or Linux
curl -fsSL https://github.com/ChristianLemer/talos/releases/latest/download/get-talos.sh | sh -s -- ./Talos
```

```powershell
# Windows — the PowerShell that ships with it is enough
irm https://github.com/ChristianLemer/talos/releases/latest/download/get-talos.ps1 | iex
```

A `Talos` folder appears where you ran it. Open the launcher of your system —
`MacOS/Talos.app`, `Windows/Talos.exe` or `Linux/Talos` — and Talos reads the machine and
shows what it would install, update or leave alone. **Nothing changes until you press Apply.**

<!-- DEMO: the 30-second recording (issue #4) goes here. -->

What you just composed is a **kit**: the launchers of one release, verified against its
checksums, beside an example catalogue. Your team's kit is the same folder with *your*
packages in it — the rest of this page is how.

---

> Τάλως — the bronze automaton that guards the perimeter, in a loop.

Talos holds out a ramp to those who do not yet live inside the text. It is a native app
(Tauri + Rust) with an embedded web UI: one `Talos` / `Talos.exe` that scans a machine and
installs, upgrades and removes packages live — proven on macOS and Windows, building and
running on Linux. The core is generic — it knows no client. Everything specific (packages,
bundles, a name) enters as **content** beside it, never into the core. The content each
release ships is an example to copy, not a policy to adopt.

---

## Package it for your team

**This repo is the workshop, not the product.** What your team gets is a **kit**: a
released Talos binary next to *your* content. The engine is generic — it knows no
client; what makes a kit yours is the `catalog/` and `bundles/` beside it. You never
build the engine, and your content never lives in this repo.

### 1 — Your content: `catalog/` + `bundles/`

Two flat folders, read from disk at runtime — beside the launcher, or one level up
when the kit puts the launchers in an OS folder. No build step:

- **`catalog/`** — one YAML per **package**: what it is called, which route installs
  it (`winget`, `brew`, `cargo`, `npm`, `bun`, `run`, a Claude Code `claude-plugin`, a
  Copilot CLI `copilot-plugin`, a `skill`, a `vscode-extension`, a `nu-plugin`), how to detect it, which version to
  hold. The file stem is the package id; `name:` is what everything else refers to.
- **`bundles/`** — one YAML per **selection**: a named card that pulls packages in by
  their `name`, and `needs:` other bundles (Base ← Manuals, Plus, Terminal is the
  shipped set).

```yaml
# catalog/jq.yaml
name: jq
description: JSON CLI — required by some agent plugin hooks
winget: jqlang.jq
brew: jq
detect: jq --version
version: latest

# bundles/base.yaml
bundle: Base
emoji: 🧱
usage: An agent, an editor, and what they stand on.
packages: [Git, Node.js, jq, Claude Code, Visual Studio Code, uv]
```

The full field reference — routes, `version:` pinning, config-atoms, behaviour seeds — is
[`plugin/skills/talos-content/references/fields.md`](plugin/skills/talos-content/references/fields.md),
and the agent that writes content with you is one command away (see **The plugin** below).
The `catalog/` and `bundles/` in this tree
are the **socle**: what a machine needs to work with an agent, every file written as a
lesson, and every route but two shown by a real package. Copy them, then make them
yours. (The engine's own tests read a separate, synthetic fixture under `tests/`.)

**Keep the content in a repo of your own.** Two folders and two more files:

```
your-talos-content/
├── catalog/
├── bundles/
├── distribution.yaml   ← optional: what your team calls this content, and which edition
└── .talos-version      ← one line: the release your kit runs, e.g. v0.2.1
```

Changing what Talos proposes is a commit. Moving to a newer Talos is a commit that
changes one line. Naming the content — `name: Data management`, `version: 2026.10` — puts
it first in the window title, before the engine's own version. And before any of it lands:

```bash
Talos --check catalog bundles     # exit 1 on a broken file, an unknown name, a bad version
```

`--check` reads your content the way the running app would and **refuses what the
runtime silently skips** — a YAML that does not parse, a bundle naming a package that
does not exist, a `requires:` nobody satisfies, a `version:` word that is neither a
version nor `latest` nor `pending`. Put it in your pipeline, or run it by hand. On
Windows, redirect the output (`Talos.exe --check catalog bundles > check.txt`): the
release exe is a GUI program and prints to a file, not to the bare console. The exit
code is the contract.

### 2 — The engine: a release, not a build

Every tag `v*` publishes the launchers on the [Releases](../../releases) page, each named
after its version and target: the zipped `Talos.app` (Apple Silicon), the Windows exe (x64),
the bare Linux binary — and `talos-content-<version>.zip`, the socle of that same tag, plus
the kit scripts. They are the same
engine for everyone; you download, you do not compile.

If you do want to build (to change the engine, not the content): [Rust](https://rustup.rs)
and the Tauri CLI, natively on each OS — Tauri does **not** cross-compile:

```
cargo tauri build          # macOS → target/release/bundle/macos/Talos.app
                           # Windows → target/release/Talos.exe
cargo build --release      # Linux → target/release/Talos (bare binary, no bundle)
```

`public/` (the web UI) is **sealed into the binary**; your content is **not** — it is
read from disk at runtime. That is the hermetic boundary: the engine changes rarely,
the content often, and the two meet on the target machine, never in this repo.

### 3 — Compose and distribute the kit

**The deliverable** is the launchers **plus** `catalog/` + `bundles/` beside them. Ship
one without the other and Talos opens inert (nothing to propose); it won't crash, it
just has nothing to say. The binaries are **unsigned** for now (signing/notarisation is
a separate step).

One command composes the kit, from **whatever machine you have**, and needs nothing but
what the OS already ships — the two commands of **Try it**, above. They take the newest
stable release; to pin one, give its tag (and pin it for your team in `.talos-version`):

```bash
curl -fsSL https://github.com/ChristianLemer/talos/releases/download/v0.2.1/get-talos.sh | sh -s -- ./Talos --version v0.2.1
```

```powershell
& ([scriptblock]::Create((irm https://github.com/ChristianLemer/talos/releases/download/v0.2.1/get-talos.ps1))) -Kit .\Talos -Version v0.2.1
```

```bash
# your team: the same, into the folder it launches from, with YOUR content
sh get-talos.sh "<kit folder>" --content path/to/your-talos-content
.\get-talos.ps1 "<kit folder>" -Content path\to\your-talos-content
```

Both compose the **same kit** — one folder per OS, so every launcher keeps its own
standard name and whoever receives the folder opens the name of their system:

```
Talos/
  MacOS/     Talos.app
  Windows/   Talos.exe
  Linux/     Talos
  catalog/   bundles/   distribution.yaml (when the content names itself)
  .talos/    content.zip · VERSION · KIT.txt · MANIFEST.sha256
```

`.talos/content.zip` carries the same content as one file. A synced share is often
partial; a single file arrives whole or not at all, so the engine reads the archive (once
per version, from a local extraction) and says so when it is unusable or missing, rather
than opening half a catalogue. The loose folders serve engines older than the archive.

Without `--content` the kit gets the **socle** of the same release. With it, your
`.talos-version` picks the release and your content replaces the socle. Either way
every asset is verified against its published digest; the composed kit is then read by
`--check` **with no arguments**, on the launcher just placed, so the engine resolves the
content itself and the layout is proven rather than assumed — a kit that does not pass is
not left behind. The `MANIFEST.sha256` it writes is one artefact three tools read back
(`shasum -a 256`, `sha256sum`, `Get-FileHash`), so a kit composed on Windows re-verifies
offline on a colleague's Mac with `get-talos.sh <kit> --verify`.

Built to live on a **shared OneDrive**, launched by many machines from the same
copy — the content is never copied per machine:

- It does **not** extract anything beside itself (that folder is shared). The
  native bits it needs go to each machine's own `%LOCALAPPDATA%\Talos` on first
  run, idempotently.
- A second launch does **not** replace a Talos already open on the machine: the first
  one keeps its port and serves the new window. Close Talos to run a newer build.
- Windows locks a running exe, so the share's `Talos.exe` only launches a local copy
  of itself (`%LOCALAPPDATA%\Talos\engine`, one per version, named after its sha256)
  and exits: a refreshed exe lands even where Talos is open. Where a policy refuses to
  run that copy, Talos runs from the share as before.
- On a Mac, mark the kit folder *Always keep on this device*: the `.app` is a folder of
  files, and a half-synced app must never be launched.

Drop the kit on the share once; every machine launches it from there.

### What your users will see

They double-click the exe (or a shortcut you place). It opens a **native window**
(Tauri, 16:9, size/position remembered per machine), scans the machine, and shows
what's present, missing, or outdated. They act; it acts, *showing* every step. No
install, no runtime to provision — the web UI is baked into the exe.

---

## The plugin — an agent that writes the content with you

Nobody receives "a configuration". An integrator receives the socle **and an agent that
adapts it**. This repo is also a Claude Code marketplace serving one plugin, `talos`:

```bash
claude plugin marketplace add github:ChristianLemer/talos
claude plugin install talos@talos
```

- skill **`talos-content`** — the doctrine, a snippet per route, the traps, the loop that
  ends at `0 errors`, and the complete field reference
  ([`references/fields.md`](plugin/skills/talos-content/references/fields.md)).
- skill **`talos-kit`** — composing and distributing a kit, `--verify` on a shared drive,
  and the Doctor tab.
- command **`/talos:init`** — scaffolds a content repo: `catalog/`, `bundles/`, a
  `.talos-version` pinned at the newest stable release, a workflow that runs `--check` in CI, the
  reference copied in verbatim, and an `AGENTS.md` so *any* agent can maintain the repo.

It ships from this branch and these tags, so the plugin and the engine always describe the
same content format — and `cargo test` fails if the engine grows a field the reference
does not mention. See [`plugin/README.md`](plugin/README.md).

---

## The layers — at a glance

Talos is the ramp; it installs the rest. These five are distinct, not
interchangeable:

```
Talos        graphical, deterministic ramp — installs / repairs / shows state
   installs ↓
Claude       the AI model (in a corporate setting: via AWS Bedrock)
   runs as ↓
Claude Code  the terminal application (the CLI)
   hosts ↓
Chiron       the agent / persona that talks with the human
   follows ↓
Kentauros    the C/Si collaboration protocol
```

Where Talos ends — at the edge of the deterministic — Chiron begins.

---

## Go deeper

For *why* Talos exists — the ramp, the deterministic vs the open sea, the refuge
— see [INTENTION.md](INTENTION.md).

---

## License

MIT — see [LICENSE](LICENSE). Use it, fork it, ship it.

---

*The bronze guardian, in a loop, around the perimeter.*
