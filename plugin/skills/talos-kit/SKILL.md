---
name: talos-kit
description: Compose and distribute a Talos kit — the released launchers plus your `catalog/` and `bundles/`, in one folder a team launches from a shared drive. Covers `.talos-version` (the pin), `get-talos.sh` and `get-talos.ps1` (compose from any OS, and what they verify), `--verify` on a synced replica, a kit for one machine, the OneDrive realities (a locked exe, a half-synced .app, a second launch that does not replace the first), the machine log (what a slow machine or a missing winget tells you, and what sharing sends), and the Doctor tab — why a rescue candidate must declare `doctor:` and how `clean:` is measured. Use when shipping content to a team, bumping the Talos version, diagnosing a kit that opens inert or runs stale code, reading why a machine is slow or never finds winget, or deciding whether a package belongs in the Doctor.
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
├── distribution.yaml   ← YOUR content's name (and edition), when it has one — leads the window title
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
├── distribution.yaml   ← optional: `name:` (and `version:`) — what the team calls this content; leads the window title
└── .talos-version      ← one line: the release this kit runs, e.g. v0.0.1-beta.42
```

- Changing what Talos proposes is a commit in `catalog/` or `bundles/`.
- Naming an edition of it is a commit in `distribution.yaml` (the `talos-content`
  reference has the two keys).
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
sh get-talos.sh ./Talos

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
curl -fsSL https://github.com/ChristianLemer/talos/releases/download/<tag>/get-talos.sh | sh -s -- ./Talos
```

```powershell
irm https://github.com/ChristianLemer/talos/releases/download/<tag>/get-talos.ps1 | iex
```

Then open the launcher for the system you are on — `MacOS/Talos.app`, `Windows\Talos.exe`,
`Linux/Talos`.

⭐ **The kit appears where you ARE, not in your home.** `get-talos.sh` takes the folder as a
required argument and invents nothing; `get-talos.ps1`, which cannot be given one through
`irm | iex`, defaults to `Talos` in the current directory. That is the convention every
command-line tool producing a folder already follows — you chose the place by standing in
it. Both print the target before writing anything.

⚠️ **On a Mac, the `.app` is inside the kit and stays there** — the engine reads `catalog/`
one level above the launcher, so moving it to `/Applications` breaks the kit. Opening it from
the kit folder is the supported gesture; what a Mac user does beyond that is theirs.

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
Linux too. Everything else — `npm`, `cargo`, `bun`, `run`, `claude-plugin`, `copilot-plugin`, `skill`,
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
same copy. The content is **never copied per machine**: `catalog/`, `bundles/`,
`behaviour/` and `logs/` are read and written on the share.

- **It extracts nothing beside itself** — that folder is shared. The native bits it needs
  go to each machine's own `%LOCALAPPDATA%\Talos` on first run, idempotently.
- ⚠️ **A second launch does not replace a Talos already open.** The first one keeps its
  port and serves the new window, so "I relaunched and still see the old version" means
  Talos was never closed on that machine. Close it, then open it again.
- **On Windows the share's `Talos.exe` only launches a local copy of itself.** A running
  exe is locked, and OneDrive answers a replaced locked exe with `Talos (1).exe` and
  deletions. So a release build copies itself, once per version, to
  `%LOCALAPPDATA%\Talos\engine\Talos-<tag>-<hash>.exe` (named after its own sha256, so a
  half-copied file never runs), starts that copy pointed back at the kit (`--kit`), and
  exits. The share's exe is open for a second, not a session: a publication lands even
  where Talos is open. The copy just launched and the newest other one are kept.
- **Pin the share's `Talos.exe`, or a shortcut to it — not the running window.** The window
  belongs to the local copy: pinned, it opens the kit this machine last launched from —
  another one, if the machine has two. If that kit's `Talos.exe` has changed since, the copy
  starts it and steps aside, so the new version opens; but once two newer versions have run,
  that copy is gone and the pin leads nowhere.
- A copy that fails while a publication is replacing the share's exe is tried once more,
  then the newest verified local copy starts instead — an older version for one session
  beats locking the share at the moment the publication lands.
- If the local copy cannot start (a policy refusing executables under `AppData\Local`),
  Talos runs from the share as it always did. `engine\last-launch.txt` says which way the
  last launch went; `TALOS_IN_PLACE=1` forces running from the share.
- ⚠️ **On a Mac, mark the kit folder *Always keep on this device*.** The `.app` is a
  folder of files, and a half-synced app must never be launched. `--verify` is how you
  find out before the user does.
- The binaries are **unsigned** for now. Signing and notarisation are a separate step.

---

## The machine log — what a slow or broken machine tells you

Each machine keeps a journal in its own data dir (`%LOCALAPPDATA%\Talos\history.jsonl`).
With the user's **share consent**, every line written from then on is also appended to
`logs/<host>/<user>.jsonl` beside the exe — that is the file to read when a machine is
slow or never finds a tool. Three kinds of line, one JSON object each:

| line | when | what it carries |
|---|---|---|
| install (has `package`) | each step | `package`, `version`, `action`, `ok`, `secs`; for a version change `found` and `target`; and when `ok` is `false`, **`exit`** (the command's exit code) and **`tail`** (its last 20 lines, about 2 KB of text, as a terminal of the row's size would have shown them — colours, cursor moves and progress repaints resolved) |
| `"kind": "machine"` | once per launch, after the first scan | OS, CPU and logical cores, RAM total/free, free space on the system disk, where the kit sits (`local`, `network`, `onedrive`, `synced`), **`shell_ms`** — one empty shell through the probe wrapper — and where each manager the plan uses resolves; on Windows also the winget alias state, the antivirus products the OS declares, Defender real-time, AC or battery, **`policies`** — every value under the AppInstaller and WindowsStore policy keys (machine and user) and their subkeys, as stored — and **`user_path_kind`** / **`user_path_vars`**, the registry type of the user PATH and whether it names a `%variable%`. A collection that failed or took over 60 s carries `error` instead, its other fields empty |
| `"kind": "scan"` | each scan | total, the three batched calls, every package probe in **ms**, and why a batched call gave no answer; `profile_running: true` when the machine profile was being collected meanwhile — its timings are then not the machine's alone |

What to read first:

- **`shell_ms`** in the seconds is the usual corporate story: an antivirus or EDR scanning
  every process start. Talos spawns hundreds of short processes; every probe pays it.
- **winget "not found"**: its path does not vary between machines — it is a per-user alias,
  `%LOCALAPPDATA%\Microsoft\WindowsApps\winget.exe`, registered by App Installer at the
  user's first logon. `winget.alias: false` means it was never registered for this user (a
  fresh profile, an image without the Store apps, LTSC or Server); `on_path: false` means it
  exists but the probes cannot see it. **`user_path_kind: "String"` with `user_path_vars:
  true`** is the usual reason for the second: the user PATH is stored as REG_SZ, so its
  `%USERPROFILE%\AppData\Local\Microsoft\WindowsApps` is never expanded.
- **`policies`**: measured, `AppInstaller.EnableDefaultSource = 0` removes the `winget`
  source every Talos route pins (all winget rows fail), `AppInstaller.EnableAppInstaller =
  0` disables winget whole; `WindowsStore.RequirePrivateStoreOnly = 1` blocks nothing
  Talos does. `null` means not read, `[]` read in full with none set.
- **A `scan` line's `unavailable`** quotes the reason the release's missing console used to
  swallow.
- **A failed install line says why** in its `tail`: the tool's own last words, as the row's
  terminal showed them. `exit: 0` with `ok: false` is an exit 0 that changed nothing. The user
  sees the same text by hovering the line in the History tab, and its button copies it — the
  way to get it from someone who does not share.
- **Past 2 MB a journal is compacted**, local and shared alike: every install line stays,
  but only the last 100 machine lines and the last 200 scan lines, and only the last 50
  failures keep their `tail`.

What is NOT collected: no serial number, no MAC address, no user name beyond the one the
file's own path carries; the home folder is written `~` in every path. A `tail` is the tool's own text, so it gets more: every user-profile path in it (`C:\Users\<name>`, its 8.3 short form, `/home/<name>`, `/Users/<name>`) is written `~`, a URL's credentials (`https://user:token@host`, or a token alone) `***@`, and what Talos itself typed into the step (a sudo password) `***`. Nothing else in a `tail` is masked: a tool that prints a secret, or a user name outside a path, in its error output would be journalled with it. The **Machine tab shows
the latest machine profile**, as the values themselves — what the user sees there
is what the share receives. Only bookkeeping stays in the file alone: timestamps, the build
stamp, the collection's own durations, the per-probe list. Turning sharing on mid-session
sends this launch's profile to the share at once; earlier lines, the last scan's included,
stay local.

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
