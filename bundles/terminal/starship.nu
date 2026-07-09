#!/usr/bin/env nu
# starship.nu — the family's Starship config, as a STREAM PATCH.
#
# The config-atom model: the patch is a pure record->record transform that upserts
# ONLY our keys, so a user's own edits pass straight through (no-clobber for free).
#   apply  =  open file | patch | save              (writes)
#   check  =  (open file) == (open file | patch)     (dry-run in memory; exit 0/1)
# The check IS the apply replayed without saving, so detection can't drift from
# application. "Outdated" = "replaying would change something" = check exits 1.
#
# Shipped as a FILE (not inlined in bundle.yaml) so all the quoting lives here,
# never on the powershell -> nu command line where nested quotes get eaten. The
# bundle calls: nu "{dir}/starship.nu" apply|check  ({dir} = bundle folder).

# The patch — the only Starship-specific part; the rest is the generic contract.
def patch []: record -> record {
  $in
  | upsert add_newline false
  | upsert character {
      success_symbol: "[>](bold green)"
      error_symbol: "[>](bold red)"
    }
}

def config-path []: nothing -> path {
  $nu.home-dir | path join .config starship.toml
}

# Current config as a record ({} when the file is absent).
def load []: nothing -> record {
  let f = (config-path)
  if ($f | path exists) { open $f } else { {} }
}

# apply: converge the file. Idempotent — re-running changes nothing.
def "main apply" [] {
  let f = (config-path)
  mkdir ($f | path dirname)
  load | patch | save -f $f
}

# check: exit 0 if already converged, 1 if applying would change something.
def "main check" [] {
  let cur = (load)
  exit (if $cur == ($cur | patch) { 0 } else { 1 })
}

def main [] {
  print "usage: nu starship.nu (apply | check)"
  exit 2
}
