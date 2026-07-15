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
# With --diff, print WHAT would change (the differing keys) and always exit 0 —
# a read-only inspection for a self-managed config; writes nothing either way.
def "main check" [--diff] {
  let cur = (load)
  let next = ($cur | patch)
  if $diff {
    if $cur == $next {
      print "no changes — your config already matches the family patch"
    } else {
      print "keys the family patch would upsert:"
      $next | transpose key value | where {|r| ($cur | get --optional $r.key) != $r.value } | table | print
    }
    exit 0
  }
  exit (if $cur == $next { 0 } else { 1 })
}

def main [] {
  print "usage: nu starship.nu (apply | check)"
  exit 2
}
