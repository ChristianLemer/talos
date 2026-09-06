# Contributing

Talos is small enough that this fits on one page.

## Before you claim anything works

Four checks, the same four CI runs:

```bash
cargo test --all-targets
cargo fmt --check
cargo clippy --all-targets -- -D warnings
node --test "test/*.test.mjs"
```

Green tests are not proof the app runs: CI compiles and bundles, it never boots
the window. The runtime check is a manual one on a real machine.

## Rules that keep being re-learned

- **English everywhere in the artefact** — UI text, comments, commit messages.
- **The core knows no client.** Anything specific to a team, a site or a flavour
  enters through `catalog/` and `bundles/`, never into `src/`.
- **Detect, don't remember.** What the machine answers *now* is the truth.
- **Every command Talos runs must be non-interactive.** A prompt deadlocks the row.
- **One shell wrapping.** Only `platform::shell_probe` / `pty_shell` build a shell
  line.
- **Every gesture leaves a trace.** A cancelled or skipped row stays visible with
  its reason.

## Commits

Conventional commits with an emoji after the colon, a subject that reads as a
sentence, and a body that says *why* — the bodies are the design notes of this
project. A change made with an AI assistant says so in a trailer.

## Content, not code?

If what you want to change is what Talos proposes — a package, a bundle — that is
content, and it does not live here. See the README, *Package it for your team*:
you keep `catalog/` and `bundles/` in your own place and pair them with a released
binary. The ones in this tree are examples and test fixtures.
