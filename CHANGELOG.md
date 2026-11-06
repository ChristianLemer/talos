# Changelog

One section per stable release, newest first. A release page shows its version's section
(`admin/release-notes.sh`), then the commits since the tag before. The `v0.0.1-beta.N` builds
that led to 0.1.0 have no section of their own: their pages list their commits.

## 0.1.0 — unreleased

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
  `talos-content-0.1.0.zip`. The names of the betas are published too, as the same files.
- `releases/latest/download/get-talos.sh` (and `.ps1`) now resolves: this is the first release
  that is not a pre-release.
- With no tag and no `.talos-version` pin, the kit scripts compose the newest stable release; a
  beta is fetched by its tag.

### Known limits
- The launchers are not signed or notarised.
- macOS: Apple silicon only. Linux builds and runs, but its support is not measured the way
  Windows and macOS are; a firewall page there opens in the browser.
