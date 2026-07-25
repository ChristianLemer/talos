# Talos

> Τάλως — the bronze automaton that guards the perimeter, in a loop.

The **graphical promoter and installer** that puts software on a machine,
repairs it, and shows its state — *by showing* what it does. It holds out a
ramp to those who do not yet live inside the text. Its inaugural payload is
Tekton (Chiron included, as a removable example).

**Status: the installer runs.** It is a native app (Tauri + Rust) with an
embedded web UI: a single `Talos` / `Talos.exe` that scans a machine and
installs / upgrades / removes packages live — proven on macOS and Windows. See
**Build the kit** below.

The core is generic — it knows no client. Everything specific (theme, packages,
plugins) enters by **extension** (a bundle folder), never into the core.

---

## Package it for your team

**This repo is the workshop, not the product.** You clone it, put *your* content
in, and produce a **kit** to hand to your colleagues or users. The engine is
generic — it knows no client; what makes a kit *yours* is the `bundles/` you
ship beside it.

### 1 — Declare your content

A bundle is a **folder** with a `bundle.yaml`. It lists packages as *needs*
satisfied by a named route (`winget`, `brew`, `cargo`, `bun`, a raw `run`, a
Claude Code `claude-plugin`, or a cross-agent `skill`). Copy an existing one and
edit — the full field reference (routes, postures, version pinning, config-atoms,
profiles) lives in [`bundles/README.md`](bundles/README.md):

```yaml
bundle: Editors
emoji: ✏️
description: Where you see and edit what the AI produces.
priority: 10           # order on screen (and now the order Apply acts in)
selectable: true       # can the whole card be toggled?
posture: opt-out       # default for its packages (see below)

packages:
  - name: Visual Studio Code
    description: Where you see and edit what the AI produces
    winget: Microsoft.VisualStudioCode   # the route (id winget knows)
    detect: code --version               # how to tell it's already present

  # A Claude Code plugin (hooks/agents/commands/MCP):
  - name: Chiron
    claude-plugin: chiron@tekton              # <plugin>@<marketplace>
    marketplace: github:ChristianLemer/tekton # registered before install
    detect: chiron                            # name `claude plugin list` reports
    requires: claude                          # not offered if `claude` is absent

  # A cross-agent skill (SKILL.md, via bunx skills):
  - name: uv/ruff/ty skills
    skill: astral-sh/claude-code-plugins      # source for `bunx skills add`
    detect: astral                            # name `bunx skills list` reports
    requires: Bun
```

For `claude-plugin` and `skill`, `detect:` is the name the tool lists the item
under (not a PATH binary), and `requires:` names a command that must be on PATH —
if it's missing, the row shows *"requires … (absent)"* instead of acting.

**Posture** — the author's policy per bundle: `mandatory` (always on, locked),
`opt-out` (on by default, user may decline), `opt-in` (off by default, user may
add), `forbidden` (always off, locked). Drop folders into `bundles/`, remove the
ones you don't want — `base/` is the neutral substrate, the rest are examples.

### 2 — Build the kit

With [Rust](https://rustup.rs) + the Tauri CLI (`cargo install tauri-cli`).
Build natively on each target OS — Tauri does **not** cross-compile (WebView2 +
MSVC on Windows, WebKit on macOS):

```
cargo tauri build          # macOS → target/release/bundle/macos/Talos.app
                           # Windows → target/release/Talos.exe
```

`public/` (the web UI) is **sealed into the binary** at compile time — the exe is
self-contained for its front end. Your `bundles/` are **not** sealed: they live in
a folder **beside** the exe, read from disk at runtime (the hermetic boundary — the
engine changes rarely, content changes often).

**The deliverable** is the executable **plus** `bundles/` next to it. The retained
distribution model is **rsync** (or any copy): drop `Talos.exe` + `bundles/` side by
side on the target — no installer required. The binaries are **unsigned** for now
(signing/notarisation is a separate step).

### 3 — Distribute it

Hand `dist/` to your team. The two parts travel **together** — the exe is the
generic engine, `bundles/` is your content, read from disk beside it at runtime.
Ship one without the other and Talos opens inert (nothing to propose); it won't
crash, it just has nothing to say.

Built to live on a **shared OneDrive**, launched by many machines from the same
copy — the exe is never copied per machine:

- It does **not** extract anything beside itself (that folder is shared). The
  native bits it needs go to each machine's own `%LOCALAPPDATA%\Talos` on first
  run, idempotently.
- It **self-heals**: each launch kills any stale instance holding its port, so
  everyone runs the latest code on the share without manual cleanup.

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

*The bronze guardian, in a loop, around the perimeter.*
