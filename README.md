# Talos

> Τάλως — the bronze automaton that guards the perimeter, in a loop.

The **graphical promoter and installer** that puts software on a machine,
repairs it, and shows its state — *by showing* what it does. It holds out a
ramp to those who do not yet live inside the text. Its inaugural payload is
Tekton (Chiron included, as a removable example).

**Status: the installer runs.** It compiles to one self-contained executable
(Deno), scans a machine, and installs / upgrades / removes packages live — on
macOS and Windows. See **Run it** below.

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
satisfied by a named route (`winget`, `brew`, `cargo`, `npm`, or a raw `run`).
Copy an existing one and edit:

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
```

**Posture** — the author's policy per bundle: `mandatory` (always on, locked),
`opt-out` (on by default, user may decline), `opt-in` (off by default, user may
add), `forbidden` (always off, locked). Drop folders into `bundles/`, remove the
ones you don't want — `base/` is the neutral substrate, the rest are examples.

### 2 — Build the kit

With [Deno](https://deno.land):

```
deno task build:win     # → dist/talos.exe        + dist/bundles/
deno task build:mac     # → dist/talos-mac-arm64  + dist/bundles/
```

Each task produces the whole kit under `dist/`: the exe **and** a fresh copy of
your `bundles/`. That `dist/` folder is the deliverable.

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

They double-click the exe (or a shortcut you place). It opens a framed panel
(an Edge/Chrome `--app` window), scans the machine, and shows what's present,
missing, or outdated. They act; it acts, *showing* every step. Close the window
and it shuts itself down when idle. No install, no runtime to provision — the
exe carries its own.

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
