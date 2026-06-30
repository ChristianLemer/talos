# Talos

> Τάλως — the bronze automaton that guards the perimeter, in a loop.

The **graphical promoter and installer** that puts software on a machine,
repairs it, and shows its state — *by showing* what it does. It holds out a
ramp to those who do not yet live inside the text. Its inaugural payload is
Tekton (Chiron included, as a removable example).

**Status: nascent.** The scope is set; the surfaces and anatomy are taking
shape. We begin with the installer.

The core is generic — it knows no client. Everything specific (theme, packages,
plugins) enters by **extension** (a bundle folder), never into the core.

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

Where Talos ends — at the edge of the deterministic — Chiron begins. Full
detail in [TEKTON.md](TEKTON.md).

---

## Go deeper

| You want to know… | Read |
|---|---|
| *Why* Talos exists — the ramp, the deterministic vs the open sea, the refuge | [INTENTION.md](INTENTION.md) |
| *How* it's built — Node, node-pty, xterm.js, bundles, the panel | [STACK.md](STACK.md) |
| *What corpus* it carries — Chiron, Kentauros, Genesis, Praxis, Aisthesis, and AI/Claude/Chiron disambiguated | [TEKTON.md](TEKTON.md) |

---

*The bronze guardian, in a loop, around the perimeter.*
