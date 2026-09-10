# Talos

> Τάλως — the bronze automaton that guards the perimeter, in a loop.

The **graphical promoter and installer** that puts software on a machine,
repairs it, and shows its state — *by showing* what it does. It holds out a
ramp to those who do not yet live inside the text. Its inaugural payload is
Tekton (Chiron included, as a removable example).

**Status: the installer runs.** It is a native app (Tauri + Rust) with an
embedded web UI: a single `Talos` / `Talos.exe` that scans a machine and
installs / upgrades / removes packages live — proven on macOS and Windows, and
building and running on Linux. See
**Build the kit** below.

The core is generic — it knows no client. Everything specific (theme, packages,
plugins) enters by **extension** (a bundle folder), never into the core.

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
  `skill`, a `vscode-extension`, a `nu-plugin`), how to detect it, which version to
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

**Keep the content in a repo of your own.** Two folders and one more file:

```
your-talos-content/
├── catalog/
├── bundles/
└── .talos-version      ← one line: the release your kit runs, e.g. v0.0.1-beta.37
```

Changing what Talos proposes is a commit. Moving to a newer Talos is a commit that
changes one line. And before either lands:

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

Every tag `v*` publishes the launchers on the [Releases](../../releases) page: the
zipped `Talos.app` (Apple Silicon), `Talos.exe` (x64), the bare Linux binary — and
`talos-content.zip`, the socle of that same tag, plus the kit script. They are the same
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
what the OS already ships:

```bash
# macOS or Linux
curl -fsSL https://github.com/ChristianLemer/talos/releases/download/v0.0.1-beta.43/get-talos.sh | sh -s -- ~/Talos
```

```powershell
# Windows (Windows PowerShell 5.1 is enough - no pwsh 7 needed)
irm https://github.com/ChristianLemer/talos/releases/download/v0.0.1-beta.43/get-talos.ps1 | iex
```

⚠️ **The tag is spelled out on purpose, and it moves with each release.** GitHub's
`releases/latest/download/…` link skips pre-releases, and every Talos release is a
pre-release until v0.1.0 — so that shorter form answers 404 today. At v0.1.0 these two
lines become `latest/download/` and stop needing an edit. Until then, the newest tag is on
the [releases page](https://github.com/ChristianLemer/talos/releases); the script itself
already picks the newest release once it is running, so only fetching it needs the tag.

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
  catalog/   bundles/
  .talos/    VERSION · KIT.txt · MANIFEST.sha256
```

Without `--content` the kit gets the **socle** of the same release. With it, your
`.talos-version` picks the release and your content replaces the socle. Either way
every asset is verified against its published digest; the composed kit is then read by
`--check` **with no arguments**, on the launcher just placed, so the engine resolves the
content itself and the layout is proven rather than assumed — a kit that does not pass is
not left behind. The `MANIFEST.sha256` it writes is one artefact three tools read back
(`shasum -a 256`, `sha256sum`, `Get-FileHash`), so a kit composed on Windows re-verifies
offline on a colleague's Mac with `get-talos.sh <kit> --verify`.

Built to live on a **shared OneDrive**, launched by many machines from the same
copy — the exe is never copied per machine:

- It does **not** extract anything beside itself (that folder is shared). The
  native bits it needs go to each machine's own `%LOCALAPPDATA%\Talos` on first
  run, idempotently.
- It **self-heals**: each launch kills any stale instance holding its port, so
  everyone runs the latest code on the share without manual cleanup.
- Windows locks a running `Talos.exe`, so a refreshed exe reaches a machine at the
  next sync after Talos is closed there. On a Mac, mark the kit folder *Always keep
  on this device*: the `.app` is a folder of files, and a half-synced app must never
  be launched.

Drop the kit on the share once; every machine runs it in place.

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
  `.talos-version` pinned at the newest release, a workflow that runs `--check` in CI, the
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
