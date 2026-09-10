---
name: talos-kit
description: Compose and distribute a Talos kit — the released launchers plus your `catalog/` and `bundles/`, in one folder a team launches from a shared drive. Covers `.talos-version` (the pin), `get-talos.sh` and `get-talos.ps1` (compose from any OS, and what they verify), `--verify` on a synced replica, a kit for one machine, the OneDrive realities (a locked exe, a half-synced .app, self-healing), and the Doctor tab — why a rescue candidate must declare `doctor:` and how `clean:` is measured. Use when shipping content to a team, bumping the Talos version, diagnosing a kit that opens inert or runs stale code, or deciding whether a package belongs in the Doctor.
---

# The Talos kit

*A kit is a folder. Every machine launches it in place, from the name of its own OS.*

```
<the kit folder>
├── MacOS/Talos.app     ← macOS launcher (a folder of files)
├── Windows/Talos.exe   ← Windows launcher
├── Linux/Talos         ← Linux launcher
├── catalog/            ← YOUR content
├── bundles/            ← YOUR content
└── .talos/
    ├── VERSION         ← the tag this kit was composed from
    ├── KIT.txt         ← provenance: tag, date, per-asset size and published digest
    └── MANIFEST.sha256 ← every file as it sits here, in `shasum -c` format
```

⭐ **One folder per OS is what lets every launcher keep its standard name.** A bare
`Talos` next to `Talos.exe` in one folder shows as two identical rows in an Explorer
that hides extensions, and one of them does nothing when double-clicked. Whoever
receives the kit opens the name of their system. The three metadata files are read by
the scripts and by nobody else, so they sit out of the way under `.talos/`.

⭐ **The engine finds the content beside the launcher, then exactly ONE level up**, then
the current directory (a dev fallback). One level, never a search upward. That is why a
kit composed flat — every kit made before the OS folders — still resolves unchanged and
needs no re-composing.

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

⭐ **It composes from any OS, and the kit does not record which one.** `get-talos.sh`
runs on macOS and Linux; `get-talos.ps1` does the same job on Windows:

```powershell
irm https://github.com/ChristianLemer/talos/releases/latest/download/get-talos.ps1 | iex

# iex cannot pass arguments, so build the scriptblock when you need them:
& ([scriptblock]::Create((irm .../get-talos.ps1))) -Kit "C:\Kits\Talos" -Content .
```

All three launchers are placed whichever machine composes. What differs between hosts is
only *how* the `.app` is unpacked — `ditto` + `xattr` on a Mac, plain `unzip` or
`Expand-Archive` elsewhere, which is faithful because the bundle is four files with no
symlink — and *which* launcher runs the check below.

⚠️ **Plain `sh`, and Windows PowerShell 5.1 — not nushell, not pwsh 7.** This is the tool
that installs the tool that installs everything else. A bootstrap may lean only on what
the OS ships: `sh`, `curl`, `unzip`, `shasum`/`sha256sum` on one side; on the other, the
5.1 that Windows already has. Requiring pwsh 7 would mean installing a shell in order to
run the installer.

⚠️ **The newest tag is read from the releases list, not from GitHub's "latest".** That
link skips pre-releases, and every Talos release is a pre-release until v0.1.0.

### What the script guarantees

**Every byte is verified, twice.** At download, each asset is checked against the sha256
GitHub publishes for it; a mismatch refuses the kit rather than composing it. After
placing, `MANIFEST.sha256` records what actually landed on disk, file by file.

**The kit is checked by the engine it ships, from where it will be read.** After
composing, the script runs `--check` **with no arguments**, on the launcher it has just
placed, **from an empty working directory**. No arguments, so the engine resolves
`catalog/` and `bundles/` itself — that proves the LAYOUT and not just the content. From
an empty directory, because the engine's last resort is the current one: run the check
from a content repo and that fallback quietly answers with *that repo's* catalog, so a kit
the engine cannot read reports "0 errors". A kit that does not pass is not left behind —
the script exits 1.

⚠️ **A tag older than the OS folders cannot read this layout.** Its engine only ever
looked beside itself. The scripts detect exactly that — they re-ask with the paths spelled
out — and say so instead of blaming your content. Compose with a newer `--version`.

**The manifest is one artefact, three readers.** `shasum -a 256` on macOS, `sha256sum` on
Linux, `Get-FileHash` on Windows: lowercase hex, two spaces, forward slashes, LF, no BOM.
A kit composed on Windows re-verifies on a colleague's Mac.

---

## A kit for ONE machine

The same script, a folder of your own, and nothing else to install:

```sh
curl -fsSL https://github.com/ChristianLemer/talos/releases/download/<tag>/get-talos.sh | sh -s -- ~/Talos
```

```powershell
irm https://github.com/ChristianLemer/talos/releases/download/<tag>/get-talos.ps1 | iex
```

Then open the launcher for the system you are on — `MacOS/Talos.app`, `Windows\Talos.exe`,
`Linux/Talos`.

⚠️ **Use the URL of a TAG, not `latest/download/`.** GitHub's "latest" link skips
pre-releases, and every Talos release is a pre-release until v0.1.0, so the `latest` form
answers 404 today.

⚠️ **Linux needs the WebKitGTK the build links against** — `webkit2gtk-4.1` and
`libayatana-appindicator`, whatever your distribution calls them. `ldd <kit>/Linux/Talos |
grep "not found"` says whether anything is missing before you wonder why nothing opens.

⭐ **A kit composes cleanly and can still have little to say on your machine, and that is
not a failure — it is what the content is for.** The engine knows exactly two system
managers: `winget` (Windows) and `brew` (macOS *and* Linux). A package routed only through
`winget` is out of scope on a Mac; one routed through `brew` needs Homebrew present, on
Linux too. Everything else — `npm`, `cargo`, `bun`, `run`, `claude-plugin`, `skill`,
`vscode-extension`, `nu-plugin` — depends only on its own tool being there. So a Linux box
without Homebrew reads the socle mostly as "manager absent": correct, and disappointing if
nobody said so first. Read the rows before concluding the kit is broken, and remember that
the answer to a sparse window is **your own content**, not a different kit.

---

## `--verify` — the gesture for a shared drive

```sh
sh get-talos.sh "<kit folder>" --verify      # macOS, Linux
.\get-talos.ps1 "<kit folder>" -Verify        # Windows
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
