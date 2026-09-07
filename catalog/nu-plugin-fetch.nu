#!/usr/bin/env nu
# Fetch a nushell plugin from its project's own release and register it. Called by the
# `nu-plugin` route when a package declares `plugin-release:`.
#
# ⭐ THE PLUGIN'S OWN INSTALLER DOES THE WORK. `plugin-release: owner/repo@ref` names a repo
# and a git ref; this sidecar fetches `install.nu` from that repo at that ref and runs it.
# Which archive to take — the one built for the Nushell MINOR running here, on this OS and
# arch — is decided THERE, where the release scheme is decided, and checked there against
# the `.sha256` the release publishes. One code path, owned by the project that owns the
# scheme, tested in its CI.
#
# The argument is what happened to the previous version of this file: it carried its own
# copy of the scheme (one bare binary per platform, two platforms, rename, chmod) and went
# two generations stale in three weeks while upstream moved to one release per Nushell
# minor, four platforms, archives with checksums. A copy of a scheme is a promise to follow
# it, and nobody here had signed that promise.
#
# The contract a plugin honours to be declared with `plugin-release:`:
#   · an `install.nu` at the ROOT of the repo, so that it is reachable at
#     https://raw.githubusercontent.com/<owner>/<repo>/<ref>/install.nu
#   · accepting `--repo owner/repo`, `--dir <path>` (where the binary goes) and
#     `--register` (run `plugin add` on the placed binary, by absolute path)
#   · choosing the build for the Nushell that RUNS it, and failing loudly when none is
#     published for that minor — a plugin for the wrong minor registers and does not load,
#     and that silent failure is worse than a red row
#   nu_plugin_xlsx is the reference implementation; its `install.nu` was read for this
#   contract on 2026-09-07.
#
# `ref` pins the INSTALLER, not the binary. The binary follows the running Nushell, which
# is the one coupling no version field can express (a plugin loads into exactly one minor).
# `HEAD` follows the project's default branch; a tag freezes the installer's behaviour.

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
        error make { msg: $"plugin-release must be owner/repo@ref, got: ($release)" }
    }
    let repo = $parts.0
    let ref = $parts.1
    let url = $"https://raw.githubusercontent.com/($repo)/($ref)/install.nu"

    let dir = (plugin-dir)
    mkdir $dir
    let installer = ($nu.temp-dir | path join $"nu_plugin_($name)-install.nu")

    print $"[nu-plugin] fetching installer ($url)"
    http get --raw $url | save --force --raw $installer

    print $"[nu-plugin] running it for Nushell ((version).version) into ($dir)"
    # A fresh `nu` process, so the installer's own errors — no build for this minor, a
    # checksum mismatch — arrive as ITS exit code and message, and a non-zero exit fails
    # this row. `--register` makes it `plugin add` the binary by absolute path, which a
    # plugin outside NU_PLUGIN_DIRS needs.
    ^nu $installer --repo $repo --dir $dir --register
    print $"[nu-plugin] ($name) registered — it loads in every new shell"
}

export def main [verb: string, name: string, release: string]: nothing -> nothing {
    match $verb {
        'install' => { install $name $release }
        _ => { error make { msg: $"unknown verb: ($verb)" } }
    }
}
