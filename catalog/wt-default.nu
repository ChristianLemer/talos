#!/usr/bin/env nu
# wt-default.nu — make Nushell the default shell in Windows Terminal, as a config-atom.
#
#   apply  =  open settings.json | upsert defaultProfile <nu guid> | save
#   check  =  the same, compared in memory (exit 0 converged, 1 drifted)
#
# The check IS the apply replayed without saving, so detection cannot drift from
# application (INTENTION rule 2). Only `defaultProfile` is touched: theme, font and
# schemes stay out (C, 2026-08-15).
#
# DO NOT AUTHOR A PROFILE. Nushell's winget package deposits a WT *fragment* at
# %LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\Nushell\nushell.json, which WT
# auto-merges into profiles.list[] at launch; the resulting profile carries
# `source: "nu"`. Writing our own profile on top produces DUPLICATES. So we only look
# the fragment-sourced GUID up and assign it — which is also why this atom requires
# Nushell: no package, no fragment, no GUID.
#
# Derived from local/wt.nu, which holds the same discovery logic but is a MODULE
# (`use ui.nu`, `export def apply`). An atom must be a standalone script with a
# `main apply` / `main check` pair and no neighbour imports, since catalog/ ships one
# file per package. The logic is that file's; the contract is starship.nu's.

const WT_REL = "Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json"
const WT_PREVIEW_REL = "Packages/Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe/LocalState/settings.json"

# Stable WT first, then Preview. Returns null when neither is installed — we never
# seed settings.json from scratch: a file we invented is not a file WT wrote.
def settings-path []: nothing -> any {
  let base = ($env | get --optional LOCALAPPDATA | default "")
  if ($base | is-empty) { return null }
  let stable = ($base | path join $WT_REL)
  if ($stable | path exists) { return $stable }
  let preview = ($base | path join $WT_PREVIEW_REL)
  if ($preview | path exists) { return $preview }
  null
}

# The fragment-sourced Nushell profile's GUID, or null if WT has not merged it yet.
def nu-guid [s: record]: nothing -> any {
  $s
  | get --optional profiles.list
  | default []
  | where { |p| ($p | get --optional source | default "") == "nu" }
  | get --optional 0.guid
}

# apply: converge, or say precisely what is missing and fail loudly. A silent no-op
# here would read as success while the default shell stayed PowerShell.
# `_dir` is the catalog directory, handed over by the engine as an ARGUMENT rather than
# spliced into the command line. This atom does not need it — it reads no sibling file — but
# it accepts it so every atom is invoked identically. See the YAML for why paths are built
# with `path join` in nu instead of with `/` in YAML.
def "main apply" [_dir?: string] {
  if not (applicable) {
    print $"Windows Terminal does not exist on ($nu.os-info.name) — nothing to do."
    exit 0
  }
  let p = (settings-path)
  if $p == null {
    print "Windows Terminal settings.json not found — is WT installed?"
    exit 1
  }
  let s = (open $p)
  let g = (nu-guid $s)
  if $g == null {
    print "Nushell's WT fragment is not registered yet — install Nushell via winget first."
    exit 1
  }
  $s | upsert defaultProfile $g | save -f $p
}

# Is this atom even applicable here? Windows Terminal is a Windows product; on macOS or
# Linux there is no artefact to converge, now or ever.
#
# ⚠️ THE ENGINE HAS NO `platforms:` FIELD. Verified 2026-08-21 against the binary's own
# serde field list — winget/brew scope a PACKAGE by route, but `run:`/`check:` are plain
# commands, so nothing tells Talos an atom is OS-specific. The atom has to say so itself.
def applicable []: nothing -> bool {
  $nu.os-info.name == "windows"
}

# check: exit 0 when already converged, 1 when applying would change something.
#
# ⭐ NOT APPLICABLE IS EXIT 0, NOT 1 — and this reverses an earlier reading. The previous
# comment argued that "cannot be converged here" is honestly answered with no, so a missing
# WT reported drift. The intent was "not yet done"; the EFFECT was a red `failed` row on
# every mac (observed 2026-08-21), because drift makes Apply run, `apply` exits 1, and a
# failed install reads as a broken machine — tooltip and all.
#
# Exit 0 is the correct answer to the question `check` actually asks: "would applying change
# something?" On macOS nothing would change, because nothing can. The row goes quiet instead
# of crying wolf.
#
# On WINDOWS the old behaviour is deliberately kept: WT absent, or the fragment unmerged, is
# genuinely not-yet-done and still reports drift so Apply tries and fails loudly.
def "main check" [_dir?: string] {
  if not (applicable) { exit 0 }
  let p = (settings-path)
  if $p == null { exit 1 }
  let s = (open $p)
  let g = (nu-guid $s)
  if $g == null { exit 1 }
  exit (if ($s | get --optional defaultProfile) == $g { 0 } else { 1 })
}

# ⭐ WHY AN UNINSTALL EXISTS, since a leftover config is inert: it is what makes the row MANAGED.
# The engine derives scope from `canUninstall` (`server.rs`: `s.uninstall.is_some()`), and
# `derivedOut` pushes any row without one OUT of scope — where a config-atom is labelled `yours`.
# C saw that label and decided these must be genuinely managed (2026-08-18).
# uninstall: restore Windows PowerShell as WT's default profile — the factory value, and what
# `local/wt.nu remove` has always done ({61c54bbd-…}).
#
# ⚠️ This DOES overwrite a choice of the user's own if they picked a third shell. Accepted: the
# alternative is a row Talos cannot manage at all, and C chose managed (2026-08-18).
def "main uninstall" [_dir?: string] {
  let p = (settings-path)
  if $p == null { return }
  let s = (open $p)
  $s | upsert defaultProfile "{61c54bbd-c2c6-5271-96e7-009a87ff44bf}" | save -f $p
}

def main [] {
  print "usage: nu wt-default.nu (apply | check | uninstall)"
  exit 2
}
