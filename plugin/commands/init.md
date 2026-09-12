---
description: Scaffold a Talos content repo — catalog/, bundles/, .talos-version pinned at the newest release, a CI check, the field reference, and an AGENTS.md so any agent can maintain it.
argument-hint: "[folder — defaults to the current one]"
allowed-tools: Bash, Read, Write, Edit
---

# Scaffold a Talos content repo

Create, in **`$1`** (or the current folder), the repo an integrator keeps.

⭐ **The files are not written here — they are COPIED from `${CLAUDE_PLUGIN_ROOT}/template/`.**
That folder is the scaffold, versioned and `--check`-ed by the engine's own test suite. Do
not retype its content, do not improve it in passing: a scaffold that differs from one run
to the next is a scaffold nobody can test. What is left to you is the judgement — refusing
a folder that is already a repo, pinning the right tag, and fixing whatever `--check` says.

## 1 — Refuse an occupied folder

```bash
ls -a
```

⚠️ If `catalog/`, `bundles/` or `.talos-version` already exists, **stop**. Say what is
there: this folder is already a content repo, and what the user wants is the
`talos-content` skill, not a scaffold. Other unrelated files are fine — a content repo can
live beside them.

## 2 — Copy the template

```bash
cp -R "${CLAUDE_PLUGIN_ROOT}/template/." .
cp "${CLAUDE_PLUGIN_ROOT}/skills/talos-content/references/fields.md" REFERENCE.md
```

That is `catalog/` (4 packages), `bundles/base.yaml`, `AGENTS.md`, `README.md`,
`.github/workflows/talos-check.yml`, and the field reference.

Then prepend one line to `REFERENCE.md`, above its first:

```
<!-- Copied verbatim by /talos:init from the talos plugin. Re-run the copy to refresh. -->
```

⚠️ **Never rewrite, summarise or reorder `REFERENCE.md`.** One authored source, one
verbatim copy — that is the only reason the copy can still be trusted months later.

If `CLAUDE_PLUGIN_ROOT` is unset, say so and stop rather than reconstructing the files from
memory. The template is at
`https://github.com/ChristianLemer/talos/tree/beta/plugin/template`.

## 3 — Pin the engine

GitHub's `latest` link **skips pre-releases**, and every Talos release is a pre-release
until v0.1.0 — so read the list:

```bash
curl -fsSL https://api.github.com/repos/ChristianLemer/talos/releases \
  | grep -m1 '"tag_name"' | cut -d'"' -f4 > .talos-version
```

If the call fails (offline, rate-limited), say so and leave `.talos-version` absent: a
wrong pin is worse than none, and the user can supply the tag.

## 4 — `git init`, no commit

If this is not already a git repo, `git init`. Do **not** commit — leave the tree for the
user to read first.

## 5 — Prove it

Find a Talos binary: on PATH, `./Talos`, `MacOS/Talos.app/Contents/MacOS/Talos` inside a
kit folder, or one the user names.

```bash
Talos --check catalog bundles
```

It must say `0 errors`. If it does not, **fix the content and run it again** — that is the
whole point of the scaffold, and a template that stopped passing is a bug worth reporting
upstream.

If no binary is reachable, say so plainly and give the one line that fetches one for this
OS from the pinned tag. Do not claim the scaffold checks out when you have not run it.

## Finally

Say, in a few lines: the tag pinned, what was created, whether `--check` ran and what it
said, and the two next moves — edit `catalog/` (the `talos-content` skill is the guide),
and `sh get-talos.sh "<kit folder>" --content .` to publish (the `talos-kit` skill).

Mention once that the **socle** — the full 19-package lesson set, where every route is
shown by a real file — is `talos-content.zip` on any release, for someone who would rather
start from that and prune.
