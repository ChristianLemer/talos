#!/usr/bin/env nu
# Fetch a nushell plugin's release binary, place it where nushell can load it, and register
# it. Called by the `nu-plugin` route when a package declares `plugin-release:`.
#
# ⚠️ EVERY STEP HERE WAS MEASURED on a real asset (2026-08-17), because three plausible
# implementations are wrong:
#
#   · the file MUST be named `nu_plugin_<name>`(.exe) — nushell validates the prefix in hard
#     code (PluginIdentity::new), so the asset's own name (`nu_plugin_xlsx-aarch64-apple-
#     darwin`) is REFUSED;
#   · the executable bit MUST be set — an HTTP fetch writes mode 644, and `plugin add` EXECUTES
#     the binary to read its signature, so without +x it fails with "permission denied";
#   · ⭐ the quarantine attribute does NOT need stripping — measured, a plain fetch sets only
#     `com.apple.provenance`. `com.apple.quarantine` comes from browsers. An `xattr -d` here
#     would be noise, and noise in an installer is indistinguishable from a fix.

def target-triple []: nothing -> string {
    # The two the fleet runs. ⚠️ Any other platform is an ERROR rather than a guess: fetching
    # the wrong architecture produces a binary that fails at `plugin add` with a message about
    # the plugin rather than about the platform, which sends the reader hunting in the wrong
    # place.
    let os = $nu.os-info.name
    let arch = $nu.os-info.arch
    if $os == 'macos' and $arch == 'aarch64' {
        'aarch64-apple-darwin'
    } else if $os == 'windows' {
        'x86_64-pc-windows-msvc.exe'
    } else {
        error make { msg: $"no published plugin binary for ($os)/($arch)" }
    }
}

def plugin-dir []: nothing -> path {
    # ⭐ nushell's OWN data dir, and NOT a manager's bin directory. A previous attempt wrote
    # into `/opt/homebrew/bin/` — a directory brew OWNS — and the distribution still carries a
    # one-shot script whose only job is deleting that leftover. What failed there was the
    # destination, not the fetching.
    #
    # ⚠️ There is no canonical location: `plugin add` records whatever absolute path it is
    # given. This one is chosen because it survives a brew upgrade of nushell.
    $nu.data-dir | path join plugins
}

export def install [name: string, release: string]: nothing -> nothing {
    let parts = ($release | split row '@')
    if ($parts | length) != 2 {
        error make { msg: $"plugin-release must be owner/repo@tag, got: ($release)" }
    }
    let repo = $parts.0
    let tag = $parts.1
    let suffix = (target-triple)
    let asset = $"nu_plugin_($name)-($suffix)"
    let url = $"https://github.com/($repo)/releases/download/($tag)/($asset)"

    let dir = (plugin-dir)
    mkdir $dir
    let exe = if $nu.os-info.name == 'windows' { '.exe' } else { '' }
    let dest = ($dir | path join $"nu_plugin_($name)($exe)")

    print $"[nu-plugin] fetching ($url)"
    http get --raw $url | save --force --raw $dest

    # POSIX only: Windows has no executable bit, the `.exe` extension is what matters.
    #
    # ⚠️ `^chmod`, with the caret. MEASURED: `chmod` is NOT a nushell builtin — a bare `chmod`
    # fails at PARSE time with `nu::parser::not_found`, so the script would not even run. The
    # caret is what calls the external command.
    if $nu.os-info.name != 'windows' {
        ^chmod +x $dest
    }

    print $"[nu-plugin] registering ($dest)"
    plugin add $dest
    print $"[nu-plugin] ($name) registered — it loads in every new shell"
}

export def main [verb: string, name: string, release: string]: nothing -> nothing {
    match $verb {
        'install' => { install $name $release }
        _ => { error make { msg: $"unknown verb: ($verb)" } }
    }
}
