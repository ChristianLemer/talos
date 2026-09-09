# The `talos` plugin

*Any Claude Code becomes the assistant for writing and maintaining Talos content.*

Nobody receives "a configuration". An integrator receives the socle and an agent that
adapts it — this is that agent, and it is versioned with the engine it describes because
it ships from the same branch and the same tags.

```bash
claude plugin marketplace add github:ChristianLemer/talos
claude plugin install talos@talos
```

## What is in it

| Component | What it carries |
|---|---|
| skill **`talos-content`** | The doctrine to respect while writing, a snippet per route, the traps, and the loop that ends at `0 errors`. Its `references/fields.md` is the **complete field reference** — the single source, which the socle's `bundles/README.md` points at. |
| skill **`talos-kit`** | Composing and distributing a kit: `.talos-version`, `get-talos.sh`, what it verifies, `--verify` on a synced replica, the shared-drive realities, and the Doctor tab — why a rescue candidate declares `doctor:` and how `clean:` is measured. |
| command **`/talos:init`** | Scaffolds a content repo in a folder: `catalog/`, `bundles/`, a `.talos-version` pinned at the newest release, a workflow that runs `--check` in CI, `REFERENCE.md` copied verbatim, and an `AGENTS.md` so an agent without this plugin can still maintain the repo. |

## Why it lives in the engine's repo

The reference describes fields the engine defines. Kept anywhere else it drifts; kept here
it is guarded — `cargo test` fails if a key is added to `RawPkg::KNOWN_KEYS` and not
mentioned in `skills/talos-content/`. The doc cannot fall behind the code without CI
saying so.

It also means the marketplace and the engine share a tag: a kit pinned at
`v0.0.1-beta.42` and a plugin checked out at that tag describe the same content format.

## Scope

The plugin speaks of Talos and nothing else — deliberately separate from any other
marketplace. It teaches content, not the engine's Rust; for the engine, read
[`CLAUDE.md`](../CLAUDE.md) at the repo root.
