#!/usr/bin/env nu
# The sidecar of config-atom.yaml. A fixture: it writes a marker file and reads it back.
def marker [dir: string]: nothing -> path { $dir | path join ".atom-applied" }
export def main [verb: string, dir: string]: nothing -> nothing {
    match $verb {
        'apply' => { "applied" | save --force (marker $dir) }
        'check' => { if not ((marker $dir) | path exists) { exit 1 } }
        'uninstall' => { rm --force (marker $dir) }
        _ => { error make { msg: $"unknown verb: ($verb)" } }
    }
}
