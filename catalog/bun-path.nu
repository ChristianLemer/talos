#!/usr/bin/env nu
# bun-path.nu — puts ~/.bun/bin on the PATH of the shells that matter, as a
# config-atom (like starship.nu). `bun add -g` links global CLIs (claude…) into
# ~/.bun/bin, a dir NOTHING adds to PATH by default when bun came from brew/winget
# — so without this the tool installs but `claude` is "command not found", and
# Talos (which probes via $SHELL -ilc / powershell) reports a false "absent".
#
#   apply = converge every relevant target (idempotent — re-running changes nothing)
#   check = exit 0 if already converged, 1 if applying would change something
#
# Targets, per OS (the ones Talos + GUI apps like VSCode actually read):
#   macOS/Linux : ~/.zshrc, ~/.bashrc  (the $SHELL -ilc bridge GUI/AI inherit) + Nushell autoload
#   Windows     : User Path in the registry (the PATH GUI apps inherit)         + Nushell autoload
# Nushell is included because it's Talos's own config language, but it is NOT the
# shell VSCode/agents inherit — hence the POSIX rc / registry are the ones that count.
# The catalog calls: nu "{dir}/bun-path.nu" apply|check   ({dir} = catalog folder)

const START = "# >>> talos bun path >>>"
const END = "# <<< talos bun path <<<"

def bun-bin []: nothing -> path {
  $nu.home-dir | path join .bun bin
}

def nu-autoload []: nothing -> path {
  $nu.default-config-dir | path join autoload bun.nu
}

def nu-autoload-content []: nothing -> string {
  "# talos bun path — managed by Talos (bun-path config-atom)\nuse std\nstd path add ($nu.home-dir | path join .bun bin)\n"
}

def posix-rcs []: nothing -> list<path> {
  [".zshrc" ".bashrc"] | each {|f| $nu.home-dir | path join $f }
}

def read-or-empty [f: path]: nothing -> string {
  if ($f | path exists) { open --raw $f } else { "" }
}

def has-block [text: string]: nothing -> bool {
  $text | str contains $START
}

def posix-block []: nothing -> string {
  $"($START)\nexport PATH=\"$HOME/.bun/bin:$PATH\"\n($END)\n"
}

def "main apply" [] {
  let naf = (nu-autoload)
  mkdir ($naf | path dirname)
  nu-autoload-content | save -f $naf
  print $"nushell: wrote ($naf)"

  if $nu.os-info.name == "windows" {
    let bin = (bun-bin)
    let cur = (^powershell -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path','User')" | str trim)
    if ($cur | str contains $bin) {
      print "windows path: already present"
    } else {
      let next = (if ($cur | is-empty) { $bin } else { $"($cur);($bin)" })
      ^powershell -NoProfile -Command $"[Environment]::SetEnvironmentVariable\('Path','($next)','User'\)"
      print $"windows path: added ($bin)"
    }
  } else {
    for f in (posix-rcs) {
      let text = (read-or-empty $f)
      if (has-block $text) {
        print $"($f): already present"
      } else {
        let sep = (if ($text | is-empty) or ($text | str ends-with "\n") { "" } else { "\n" })
        $"($text)($sep)(posix-block)" | save -f $f
        print $"($f): added block"
      }
    }
  }
}

def "main check" [] {
  mut ok = true
  let naf = (nu-autoload)
  if not (($naf | path exists) and ((read-or-empty $naf) | str contains "talos bun path")) {
    $ok = false
  }
  if $nu.os-info.name == "windows" {
    let bin = (bun-bin)
    let cur = (^powershell -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path','User')" | str trim)
    if not ($cur | str contains $bin) { $ok = false }
  } else {
    for f in (posix-rcs) {
      if not (has-block (read-or-empty $f)) { $ok = false }
    }
  }
  exit (if $ok { 0 } else { 1 })
}

def main [] {
  print "usage: nu bun-path.nu (apply | check)"
  exit 2
}
