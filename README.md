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

Two flat folders, read from disk beside the exe at runtime — no build step:

- **`catalog/`** — one YAML per **package**: what it is called, which route installs
  it (`winget`, `brew`, `cargo`, `npm`, `bun`, `run`, a Claude Code `claude-plugin`, a
  `skill`, a `vscode-extension`, a `nu-plugin`), how to detect it, which version to
  hold. The file stem is the package id; `name:` is what everything else refers to.
- **`bundles/`** — one YAML per **selection**: a named card that pulls packages in by
  their `name`, and `needs:` other bundles (Base ← Documents ← Data ← Development is
  the shipped chain).

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
usage: The essentials — an AI agent, the plumbing it runs on, the method.
packages: [Git, Node.js, jq, Nushell, Claude Code]
```

The full field reference — routes, `version:` pinning, config-atoms, behaviour seeds —
is [`bundles/README.md`](bundles/README.md). The `catalog/` and `bundles/` in this tree
are **examples**, and double as fixtures for the engine's own tests: copy them, then
make them yours.

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
zipped `Talos.app` (Apple Silicon), `Talos.exe` (x64) and the bare Linux binary. They
are the same engine for everyone; you download, you do not compile.

If you do want to build (to change the engine, not the content): [Rust](https://rustup.rs)
and the Tauri CLI, natively on each OS — Tauri does **not** cross-compile:

```
cargo tauri build          # macOS → target/release/bundle/macos/Talos.app
                           # Windows → target/release/Talos.exe
cargo build --release      # Linux → target/release/Talos (bare binary, no bundle)
```

`public/` (the web UI) is **sealed into the binary**; your content is **not** — it is
read from the folder beside the exe at runtime. That is the hermetic boundary: the
engine changes rarely, the content often, and the two meet on the target machine,
never in this repo.

### 3 — Compose and distribute the kit

**The deliverable** is the launchers **plus** `catalog/` + `bundles/` beside them. Ship
one without the other and Talos opens inert (nothing to propose); it won't crash, it
just has nothing to say. The binaries are **unsigned** for now (signing/notarisation is
a separate step).

One script composes the kit, from a Mac, into the folder your team launches from:

```bash
sh admin/get-talos.sh "<kit folder>" --content path/to/your-talos-content
```

It reads `.talos-version`, fetches those launchers, and copies your content beside
them. Without `--content` it refreshes the launchers only and leaves the content in
the kit untouched — for a team that edits its YAML in place on the share. It needs
`gh` (the repo is private today; `gh auth login` once).

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
