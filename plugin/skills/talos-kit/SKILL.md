---
name: talos-kit
description: Compose and distribute a Talos kit — the released launchers plus your `catalog/` and `bundles/`, in one flat folder a team launches from a shared drive. Covers `.talos-version` (the pin), `get-talos.sh` (compose, and what it verifies), `--verify` on a synced replica, the OneDrive realities (a locked exe, a half-synced .app, self-healing), and the Doctor tab — why a rescue candidate must declare `doctor:` and how `clean:` is measured. Use when shipping content to a team, bumping the Talos version, diagnosing a kit that opens inert or runs stale code, or deciding whether a package belongs in the Doctor.
---

# The Talos kit

*A kit is a folder. Three things in it, flat, and every machine launches it in place.*

```
<the kit folder>
├── Talos.app/          ← macOS launcher (a folder of files)
├── Talos.exe           ← Windows launcher
├── catalog/            ← YOUR content
├── bundles/            ← YOUR content
├── VERSION             ← the tag this kit was composed from
├── KIT.txt             ← provenance: tag, date, per-asset size and published digest
└── MANIFEST.sha256     ← every file as it sits here, in `shasum -c` format
```

⚠️ **A release is TWO halves.** Ship a launcher without `catalog/` + `bundles/` and Talos
opens **inert** — it does not crash, it simply has nothing to propose. Any handoff says
"copy the whole folder", never "copy the exe".

**You never build the engine.** It is a published release, identical for everyone. What
makes a kit yours is the content beside it. The two meet on the target machine, never in
a repo.

---

## The content repo

Your content lives in **your own repo**, not in a fork of talos:

```
your-talos-content/
├── catalog/
├── bundles/
└── .talos-version      ← one line: the release this kit runs, e.g. v0.0.1-beta.42
```

- Changing what Talos proposes is a commit in `catalog/` or `bundles/`.
- Moving to a newer Talos is a commit that changes **one line**.
- Before either lands: `Talos --check catalog bundles` must say `0 errors`. Put it in the
  repo's pipeline; `/talos:init` scaffolds a workflow that does.

⭐ **`.talos-version` is the pin, and the composing script reads it.** Give no
`--version` flag and the tag comes from `.talos-version` beside the content — so the
repo, not the operator's memory, decides which engine the team runs.

---

## Composing — one script

```sh
# your team: the launchers of the pinned release, plus YOUR content, into the shared folder
sh get-talos.sh "<kit folder>" --content .

# discovering Talos: a folder that works, ready to double-click, with the SOCLE
sh get-talos.sh ~/Talos

# a specific release
sh get-talos.sh "<kit folder>" --version v0.0.1-beta.42 --content .
```

`get-talos.sh` ships with every release (and lives at `admin/get-talos.sh` in the talos
repo). Without `--content` the kit gets the **socle** of that same release —
`talos-content.zip`, what a stranger receives. With it, your folder replaces the socle.

⚠️ **It composes from a Mac.** It leans on `ditto` and `xattr` to unpack the `.app` with
its symlinks and permissions intact, so today it runs on macOS. It nonetheless places
*both* launchers — the Windows machines on the share get their `Talos.exe` from the same
run. A Linux binary is published per release but is not put in the kit, deliberately: a
bare `Talos` file beside `Talos.exe` would only confuse the folder.

⚠️ **Plain `sh`, not nushell, on purpose.** This is the tool that installs the tool that
installs nushell. A bootstrap may lean only on what the OS ships — `sh`, `curl`,
`shasum`, `ditto`, `xattr` — and on nothing you have to install first.

⚠️ **The newest tag is read from the releases list, not from GitHub's "latest".** That
link skips pre-releases, and every Talos release is a pre-release until v0.1.0.

### What the script guarantees

**Every byte is verified, twice.** At download, each asset is checked against the sha256
GitHub publishes for it; a mismatch refuses the kit rather than composing it. After
placing, `MANIFEST.sha256` records what actually landed on disk, file by file.

**The kit is checked by the engine it ships.** After composing, the script runs
`Talos.app/Contents/MacOS/Talos --check catalog bundles` on the folder itself: the
content is validated by the exact binary that will read it, before the manifest is
written. A kit that does not pass is not left behind — the script exits 1.

---

## `--verify` — the gesture for a shared drive

```sh
sh get-talos.sh "<kit folder>" --verify
```

Recomputes every file against `MANIFEST.sha256`, offline, and exits 1 on the first
difference.

⭐ **The failure mode of a shared folder is not a tampered download — it is a HALF-SYNCED
replica.** Right file names, wrong bytes, on one colleague's machine. The digest GitHub
publishes covers the `.app`'s ZIP, which is gone once extracted; the manifest is computed
over the extracted tree, so it is the only thing that can answer "is *this* copy whole?".
Run `--verify` on the machine that is behaving strangely, not on the one that composed.

---

## Living on a shared drive

The kit is built to sit on a shared OneDrive and be launched by many machines from the
same copy. The exe is **never copied per machine**.

- **It extracts nothing beside itself** — that folder is shared. The native bits it needs
  go to each machine's own `%LOCALAPPDATA%\Talos` on first run, idempotently.
- **It self-heals**: each launch kills any stale instance holding its port, so everyone
  runs the latest code on the share without manual cleanup.
- ⚠️ **Windows locks a running `Talos.exe`.** OneDrive cannot replace it on a machine
  where Talos is open; it retries, and the new exe lands at the next sync after Talos is
  closed *there*. "My colleague has the new version and I don't" is almost always this.
- ⚠️ **On a Mac, mark the kit folder *Always keep on this device*.** The `.app` is a
  folder of files, and a half-synced app must never be launched. `--verify` is how you
  find out before the user does.
- The binaries are **unsigned** for now. Signing and notarisation are a separate step.

---

## The Doctor tab — a rescue session, declared

The Doctor tab launches a **rescue** terminal: a program started by absolute path, with no
shell wrapping. Its dropdown lists every **present** package that declares `doctor:`, in
catalogue order, then the **OS shell without profile** as the floor.

```yaml
name: Claude Code
detect: claude --version
doctor:
  clean: { env: CLAUDE_CONFIG_DIR }
```

- **`doctor:` is a capability, never a category.** `category: [doctor]` does nothing —
  `category:` is a display axis and decides no behaviour. `--check` refuses that spelling,
  and refuses a `doctor:` without a `detect:`: the binary launched is the **first word of
  `detect:`**.
- **`clean:` is optional and it is what puts a *Launch clean* button beside *Launch*.**
  Two forms, because that is how many exist: `{ env: VAR }` — a fresh per-machine
  directory handed through an environment variable — and `{ args: [...] }` — arguments
  that skip the config (`nu -n`, `zsh -f`, `powershell -NoProfile`). Both may be given.
- ⭐ **Declare `clean:` only from a measurement.** Claude Code's `CLAUDE_CONFIG_DIR` was
  measured on macOS: no plugins, no hooks, no memory, no inherited settings, and auth
  survives because credentials come from the keychain. `--bare` was measured too and does
  the opposite — it skips plugin *sync*, keeps the plugins loaded, and drops the keychain.
  A guess here ships a "rescue" that reproduces the very breakage it was opened for.
- **The floor is engine-owned and always there.** A rescue that depends on an install
  having succeeded is not a rescue. That is why the OS shell is not a catalogue entry.
- **The Doctor points; the Catalog installs.** A declared candidate that is absent is
  named in the dropdown with a pointer to the Catalog — never installed from here.

---

## Bumping the engine

1. Read the release notes for the tag you are moving to.
2. Edit `.talos-version` — one line, one commit.
3. `Talos --check catalog bundles` with the **new** binary if a field changed.
4. `sh get-talos.sh "<kit folder>" --content .`
5. `sh get-talos.sh "<kit folder>" --verify` from a second machine once it has synced.

⚠️ Every binary bakes a build stamp (change · sha · timestamp), so you can always tell
which build a machine is actually running. When a report and the share disagree, read the
stamp before believing either.
