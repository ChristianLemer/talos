# Talos — working instructions

Talos is a generic graphical promoter/installer: it shows a machine's real state, lets you
choose what should be there, and converges. Rust + Tauri backend, plain-JS front in `public/`.

## Language

**English everywhere in the artefact** — UI text, comments, commit messages, docs. Zero French
in committed files. The conversation with the operator is usually French; the code never is.

## Version control: jj, not git

This repo is driven with **jj** (colocated over git). Do not say or run `git reset`, `git
rebase`, `git commit`. Equivalents: rewrite a message = `jj describe`; realign after a rewritten
history = `jj git fetch` then work from the new head. The only legitimate `jj git …` commands are
`fetch` / `push` / `clone`.

Reading history through `git log` / `git show` / `git ls-tree` is fine — jj exposes no
content-grep across commits, so those stay useful. **Never** use a git command that *writes*.

⚠️ **`jj new` immediately after a push.** jj's working copy is a real commit; if `@` is the
commit the bookmark points at and you edit a file, you amend the **pushed** commit and a `jj
describe` overwrites its message. This repo has been bitten 3×. Check with
`jj log -r 'beta@origin | @'` before touching anything.

⚠️ **`git log --all` lies here.** jj adds thousands of internal `refs/jj/keep` refs, so `--all`
reports ~15 000 commits instead of the real count. Always scope to the branch:
`git log refs/heads/beta`.

⚠️ **`_*` is gitignored** (local scratch). `_PLAN*.md`, `_STACK.md` live on disk, never
versioned. Editing only a `_*` file and committing produces an EMPTY commit.

## Repo shape (as of 2026-08-09)

| ref | what |
|---|---|
| `beta` | **the living branch**, 327 commits, linear (0 merges) |
| `main` | the primordial empty commit — deliberately, work is on `beta` |
| tags | 20 (`v0.0.1-beta.1` … `beta.20`), all on the rewritten history |
| visibility | **private** |

⚠️ **The living branch was called `tauri` until 2026-08-09.** It was renamed because the name
described the *framework* — and the framework had already been replaced once (Deno →
Rust/Tauri), so it was one migration away from lying again. An old note, handoff or shell
history citing `tauri` means `beta`. The branch `tauri` no longer exists.

⚠️ **`beta` the branch and `beta.N` the tags are two different things.** The branch is *where
work happens*; the tags are *released builds*. A future 1.0 will still live on a branch called
`beta` — the name will be wrong then, and that was a knowing trade-off.

`docs/` is **gitignored** — the field notes (handoffs, plans, specs, session states) describe
specific corporate estates, so they are not in the repo. Do not try to `git add` them back,
and do not cite a `docs/…` path in a committed comment: a public reader could not open it.

Since 2026-08-30 they live in a synced vault OUTSIDE the checkout, and **where is deliberately
not written here.** This file is committed: recording the location would publish it the day the
repo does, which is the very thing gitignoring the notes avoids. The same goes for the former
`_*` drafts, which moved there too and dropped the underscore.

⚠️ **`docs` in a working checkout is a local SYMLINK to that vault, never versioned** — it holds
an absolute path that differs per machine and names a personal account. It is what keeps every
`docs/…` path resolving, superpowers included. **The setup recipe lives with the notes**, in a
`SETUP.md` beside them — which is also how a second machine learns what to do, without the repo
having to say it.

⚠️ **`.gitignore` says `docs`, with NO trailing slash, deliberately.** `docs/` matches a
directory only; a symlink is a file to git, so the old pattern stopped ignoring it and the link
was staged for commit on the first try. Do not put the slash back.

Still local and deliberately not documents: `_bundles-test/` (a test fixture), `_chiron-logo/`
(graphics) and `_spike-font.ps1`.

History was purged of every employer identification on 2026-08-09. When writing a comment about
a firewall or a corporate constraint, say **"a corporate network" / "a corporate firewall"** —
never name a company. The technical fact is the valuable part; the identification is not ours to
publish.

## The four verifications

Run all four before claiming anything works. These are exactly what CI enforces:

```bash
cargo test --all-targets                      # 212 + 2 = 214 tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings     # any lint fails the build
node --test "test/*.test.mjs"                 # 122 tests
```

⚠️ **Not `cargo test --bins`** — it runs only 2 tests and looks green while proving nothing.
⚠️ **Quote the node glob.** `node --test test/` treats the path as a directory and misses files;
pass `"test/*.test.mjs"`.

Green tests are not proof the app runs. CI compiles and bundles; it never boots the GUI. The
real runtime check is a manual smoke-test on a real Windows machine.

## Build

```bash
cargo tauri build     # the deliverable; needs `cargo binstall tauri-cli` once
```

The Mac deliverable is `target/release/bundle/macos/Talos.app`, never the bare binary. A release
is triggered by pushing a `v*` tag: `.github/workflows/release.yml` builds natively per OS
(Tauri does not cross-compile) and attaches `Talos-macos-aarch64.app.zip` + `Talos.exe`.

⚠️ **A release is TWO halves**: the binary *and* `catalog/` + `bundles/`, which the exe reads
from disk beside itself. Shipping the exe alone tests a mixture and fails misleadingly. Any
handoff says "copy three things".

Every binary bakes a build stamp (jj change · sha · timestamp) so you can tell which build is
running. ⚠️ The 18 betas before `beta.19` have stamps pointing at SHAs that no longer exist
(history rewrite) — their binaries work, but an old stamp will not resolve.

## Architecture

- **`src/`** — 24 Rust modules. `server.rs` (axum + WS), `catalog.rs` / `bundles.rs` (read YAML
  from disk), `detect.rs` / `outdated.rs` (real machine state), `pty.rs` (portable-pty),
  `decision.rs`, `ladder.rs`, `behaviour.rs`, `platform.rs`.
- **`public/`** — the front, plain ES modules, no framework, no bundler, no CDN. `decision.js`
  is **shared** with the server's logic: one rule, two callers. `model.js` is DOM-free and
  tested.
- **`catalog/`** — one YAML per package (**33 packages / 35 files**: two `.nu` sidecars,
  `bun-path.nu` and `starship.nu`). Never write 35 packages.
- **`bundles/`** — selections, a chain: Base ← Documents ← Data ← Development, plus standalone
  Terminal tools and Legacy.

**The core knows no client.** Anything client- or flavour-specific enters by extension, never
into the core. `catalog/chiron.yaml` still carries a site-specific marketplace path — a known
residue, and the open design question is that `load_catalog(dir)` reads exactly ONE directory
while the old engine scanned two (core + integrator).

## Doctrine that keeps being re-learned

- **Detect, don't remember.** `detectRoutes(pkg)` observed *now* is the truth; a journal is
  partial. But the user's *intention* is a different axis and MUST be persisted.
- **No TTL cache on detection** — it masks a manual removal. Apply re-scans live and repaints
  before acting.
- **The row terminal is display-only.** A command that PROMPTS deadlocks the step. Every command
  must be non-interactive (`brew --yes`, `winget --accept-*`, `npx --yes`).
- **One shell wrapping.** Only `platform::shell_probe` / `pty_shell` build a shell line. Any
  other hardcoded shell string bypasses the fixes.
- **Parse JSON natively** with serde. No nushell in the engine.
- **Scan is serial on purpose** — ~44 concurrent powershells clashed. Never re-parallelise it.
- **Every gesture leaves a trace.** A cancelled or skipped row must still be visible with its
  reason; it must never silently disappear.

## Working with the operator

Peer posture: he has been architect, coach and CTO. Don't explain basics; bring rigour and
pushback. Decide reversible design from reasoning instead of asking for confirmation, and state
the assumption.

⚠️ **When he insists on a gesture from his own toolchain, execute it before arguing.** He knows
jj better than the default reflexes do; a narrow test is not proof of a broad impossibility. If
you claim something is impossible, say it with the scope of what you actually measured.

Measure from the UI, not from a probe: a WS probe is not what `app.js` sends, and an optimisation
triggered by the UI must be timed on a real click.
