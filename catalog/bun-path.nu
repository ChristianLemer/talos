#!/usr/bin/env nu
# bun-path.nu — puts ~/.bun/bin on the PATH of the shells that matter, as a
# config-atom modelled on starship.nu. `bun add -g` links global CLIs (claude…)
# into ~/.bun/bin, a dir NOTHING adds to PATH when bun came from brew/winget — so
# without this the tool installs but `claude` is "command not found", and Talos
# (probing via $SHELL -ilc / powershell) reports a false "absent".
#
#   apply  = converge every target        (idempotent: no-op if converged, re-writes if drifted)
#   check  = is every target converged?    (exit 0/1; --diff lists what would change)
#
# The check IS the apply replayed in memory: for each target we compare the
# MANAGED REGION (what lives between our sentinels — see `managed-region`) against
# the desired content. So check catches a block edited by hand (drift), not merely
# an absent one — the starship.nu contract, generalised to a partially-owned file.
#
# Targets, per OS (the ones Talos + GUI apps like VSCode actually read via $SHELL -ilc):
#   macOS/Linux : ~/.zshrc, ~/.bashrc (sentinel block) + Nushell autoload (owned file)
#   Windows     : User Path in the registry (GUI-inherited) + Nushell autoload
# The catalog calls: nu "{dir}/bun-path.nu" apply|check   ({dir} = catalog folder)

const START = "# >>> talos bun path >>>"
const END = "# <<< talos bun path <<<"

def bun-bin []: nothing -> path { $nu.home-dir | path join .bun bin }
def nu-autoload []: nothing -> path { $nu.default-config-dir | path join autoload bun.nu }

def nu-autoload-content []: nothing -> string {
  "# talos bun path — managed by Talos (bun-path config-atom)\nuse std\nstd path add ($nu.home-dir | path join .bun bin)\n"
}

def posix-rcs []: nothing -> list<path> {
  [".zshrc" ".bashrc"] | each {|f| $nu.home-dir | path join $f }
}

def read-or-empty [f: path]: nothing -> string {
  if ($f | path exists) { open --raw $f } else { "" }
}

# The line we manage INSIDE a POSIX rc (the content between the sentinels).
def posix-managed []: nothing -> string { 'export PATH="$HOME/.bun/bin:$PATH"' }

# managed-region: project a file's FULL text onto JUST our part — the text between
# the sentinels — or null when our block is absent. The key to check=apply-in-memory
# on a file we only partially own: compare THIS to posix-managed to detect drift.
def managed-region [text: string]: nothing -> any {
  if not ($text | str contains $START) { null
  } else { $text | split row $START | get 1 | split row $END | get 0 | str trim }
}

# Remove our block (markers included), leaving the user's own lines intact.
def strip-managed [text: string]: nothing -> string {
  if not ($text | str contains $START) { $text
  } else {
    let before = ($text | split row $START | get 0)
    let after = ($text | split row $START | get 1 | split row $END | get 1? | default "")
    $"($before)($after)"
  }
}

def posix-block []: nothing -> string { $"($START)\n(posix-managed)\n($END)\n" }

# Converge one rc: no-op if the managed region already matches; else strip any
# drifted block and append the fresh one (never duplicates).
def converge-rc [f: path] {
  let text = (read-or-empty $f)
  if (managed-region $text) == (posix-managed) { print $"($f): already converged"
  } else {
    let base = (strip-managed $text)
    let sep = (if ($base | is-empty) or ($base | str ends-with "\n") { "" } else { "\n" })
    $"($base)($sep)(posix-block)" | save -f $f
    print $"($f): converged"
  }
}

def "main apply" [] {
  let naf = (nu-autoload)
  mkdir ($naf | path dirname)
  if (read-or-empty $naf) != (nu-autoload-content) { nu-autoload-content | save -f $naf; print $"nushell: wrote ($naf)"
  } else { print "nushell: already converged" }

  if $nu.os-info.name == "windows" {
    let bin = (bun-bin)
    let cur = (^powershell -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path','User')" | str trim)
    if ($cur | str contains $bin) { print "windows path: already present"
    } else {
      let next = (if ($cur | is-empty) { $bin } else { $"($cur);($bin)" })
      ^powershell -NoProfile -Command $"[Environment]::SetEnvironmentVariable\('Path','($next)','User'\)"
      print $"windows path: added ($bin)"
    }
  } else {
    for f in (posix-rcs) { converge-rc $f }
  }
}

def "main check" [--diff] {
  mut drifts = []
  let naf = (nu-autoload)
  if (read-or-empty $naf) != (nu-autoload-content) { $drifts = ($drifts | append $"nushell autoload ($naf)") }

  if $nu.os-info.name == "windows" {
    let bin = (bun-bin)
    let cur = (^powershell -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path','User')" | str trim)
    if not ($cur | str contains $bin) { $drifts = ($drifts | append "windows User Path (registry)") }
  } else {
    for f in (posix-rcs) {
      if (managed-region (read-or-empty $f)) != (posix-managed) { $drifts = ($drifts | append ($f | into string)) }
    }
  }

  if $diff {
    if ($drifts | is-empty) { print "converged — every target already carries our managed block"
    } else { print "would change:"; $drifts | each {|d| print $"  ($d)" } }
    exit 0
  }
  exit (if ($drifts | is-empty) { 0 } else { 1 })
}

def main [] {
  print "usage: nu bun-path.nu (apply | check [--diff])"
  exit 2
}
