# Changelog

One section per stable release, newest first. A release page shows its version's section
(`admin/release-notes.sh`), then the commits since the tag before. The `v0.0.1-beta.N` builds
that led to 0.1.0 have no section of their own: their pages list their commits.

## 0.2.1 — 2026-11-10

A finder in the Catalog tab.

### What it does
- **`/` finds, Esc clears.** On the Catalog tab, `/` focuses a search field (never while
  typing in another field); Esc clears the query and leaves the field.
- **Matching is fzf's, simplified.** The query's characters in order, not necessarily
  adjacent, with case and accents folded on both sides (`nüs` finds Nushell). Space-separated
  terms must all match. A name match ranks above any match in the description, the categories
  or the catalogue id.
- **The list is filtered in place.** Categories holding a match open, ordered by their best
  row; the others fold, header kept. No match says so in one line. Clearing the field brings
  the accordion back as it was, a category folded by hand included.
- **A view, nothing more.** The finder changes neither the scope, nor an intention, nor the
  Apply. During an Apply, its focus mode wins over a standing query.

### Why a patch release
Content is unchanged: a kit pinned at v0.2.0 reads the same content, and nothing in
`catalog/` or `bundles/` needs a newer pin.

## 0.2.0 — 2026-11-10

Two new content forms. A dependent can say how recent its host must be, and a Claude plugin
stays current on its own.

### What it does
- **`requires:` takes a minimum version.** `requires: [Orca >= 1.4.252]` sits beside a bare
  name (`≥` is read too). When the requirement is present but older, the row is held before
  the Apply, and it says why with both versions: `requires Orca >= 1.4.252 (found 1.4.237)`.
  - **Held:** the minimum holds what would run the row against the old host, which is an
    install, an upgrade or an `update-on` update.
  - **Never held:** a removal. A row whose host is too old stays in scope, and turning it off
    still uninstalls it.
  - **Unknown version:** held as `(version unknown)`, never guessed either way.
  - **The requirement itself** is never upgraded by this rule; its own row decides, as always.
  - A row that requires a row whose install is held is held too. `--check` refuses any other sign, a version
    that is not one, the same name twice, and a minimum above the requirement's own exact pin.
- **Claude plugins stay current (#13).** A `claude-plugin:` row with `update-on: apply` runs
  `claude plugin update` once per session. The row says `X → Y` or `already current` (when
  both versions are read), and a failure is a line on the row. The update targets the user scope; the uninstall is bare, as
  in 0.1.0.

### Why a minor release
Content that uses either form needs an engine pinned at **v0.2.0 or later** in `.talos-version`.
- A 0.1.0 engine reads `Name >= x` as the name of an unknown package, so the row is held
  everywhere.
- Its `--check` refuses both forms.
- The 0.2.0 socle's `superpowers.yaml` fails a 0.1.0 `--check`.

### Known limits
- A row held by a minimum is not installed in the same Apply when its requirement's own row
  takes it past the minimum: it waits for the next scan.
- A `claude-plugin:` copy installed at PROJECT scope reads present: detection ignores the
  scope.

## 0.1.0 — 2026-11-06

The first stable release. Talos shows what a machine really has, lets a person choose what
should be there, and converges, showing every step.

### What it does
- **Reads the machine as it is.** Presence and versions come from the system managers (winget
  on Windows, brew on macOS) and each package's own `detect:` command, at every scan — never
  from a memory of what was installed. A manager that does not answer reads `unknown`, not
  absent.
- **Keeps what the person wants apart from what is there.** Bundles (`needs:`), personal
  picks, and four intentions per row (manual, off, automatic, on) persist; the machine is
  re-read before every Apply.
- **Converges live.** Apply installs, upgrades and removes in a visible order, with each row's
  own terminal. A cancelled or skipped row stays on screen with its reason, and every gesture
  goes into the journal.
- **Sees a repaired PATH without a restart.** Each step re-reads the user PATH, so when a
  content row repairs it on Windows, the rows that `requires:` it find their tools in that
  same Apply, with no sign-out. A row that prints `talos: path-repaired` gets one informative
  note at the end of the Apply: terminals opened from then on use the new PATH, the ones
  already open keep the old.
- **Gets past a corporate firewall.** A blocked download opens the firewall's page inside
  Talos (Windows, macOS), and Talos retries by itself after the approval. For applications
  that update themselves (`version: "self"`), `unblock:` checks their own download on the
  road the system gives it, a proxy auto-config script included, and installs nothing.
- **Ships as a kit.** `get-talos.sh` (macOS, Linux) and `get-talos.ps1` (Windows PowerShell
  5.1) compose the same folder from any machine: the launchers of one release, verified
  against its checksums, beside a team's own `catalog/` and `bundles/`. It runs from a
  shared drive; on Windows the launcher copies itself locally, so the drive's exe is never
  locked.
- **Says what is wrong with content.** `Talos --check` reads a content folder the strict way;
  the Doctor tab offers the rescue packages a content declares.
- **Ships an example, not a policy.** The `catalog/` and `bundles/` published with a release
  are a starting point to copy into a team's own content. They name no system runtime that
  other software shares, so a first Apply never proposes removing one.
- **Comes with an authoring kit.** The repo is a Claude Code marketplace: the `talos` plugin
  holds the content doctrine, the field reference, and `/talos:init`, which scaffolds a team's
  content repository.

### Release files
- Each launcher is named after its version and target: `Talos-0.1.0-aarch64-apple-darwin.app.zip`,
  `Talos-0.1.0-x86_64-pc-windows-msvc.exe`, `Talos-0.1.0-x86_64-unknown-linux-gnu`, and
  `talos-content-0.1.0.zip`. The betas' short names (`Talos.exe`, `Talos-macos-aarch64.app.zip`,
  `Talos-linux-x86_64`, `talos-content.zip`) are no longer published: a script that fetches
  them by name moves to these. A content repo whose CI `/talos:init` wrote before beta.78
  fetches `Talos-linux-x86_64`: recopy `plugin/template/.github/workflows/talos-check.yml`
  when its `.talos-version` moves to v0.1.0. `get-talos` still composes a pinned beta.
- `releases/latest/download/get-talos.sh` (and `.ps1`) now resolves: this is the first release
  that is not a pre-release.
- With no tag and no `.talos-version` pin, the kit scripts compose the newest stable release; a
  beta is fetched by its tag.

### Known limits
- The launchers are not signed or notarised.
- macOS: Apple silicon only. Linux builds and runs, but its support is not measured the way
  Windows and macOS are; a firewall page there opens in the browser.
