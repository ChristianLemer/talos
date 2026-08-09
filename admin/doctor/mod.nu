# doctor — ask the machine what it actually answers, and say it out loud.
#
# Talos decides what to install by OBSERVING the machine. When a row is wrong,
# the only useful question is "what did the machine answer?". This asks, and
# returns TABLES — so you can pipe, filter and sort the answer instead of
# reading a wall of text. `--save` writes a timestamped report to attach to an
# issue.
#
# READ-ONLY by construction: it lists, queries and reads files. It never
# installs, upgrades, uninstalls, or writes anything except the report file.
# Every command is non-interactive.
#
# Package ids are DERIVED from catalog/, never hardcoded, so this stays true as
# the catalogue evolves.

# This module's own folder, resolved at PARSE time. `$env.FILE_PWD` only exists
# while the file is being sourced, not when a command later runs — hence a const.
const MODULE_DIR = path self .

# The catalogue: one YAML per package. Resolved relative to this module so the
# gesture works from any cwd.
def catalog-dir []: nothing -> path {
    [($MODULE_DIR | path dirname | path dirname) "catalog"] | path join
}

# The OS Talos targets. `winget` is Windows, `brew` is macOS/Linux — a route is
# only meaningful on the platform that carries its manager, so every gesture
# reads this rather than assuming.
export def os []: nothing -> record {
    # `$nu.os-info` already names the platform ("windows"/"macos"/"linux") and
    # the arch, so nothing here has to pattern-match on a display string like
    # "Darwin" or "Microsoft Windows 11 Pro".
    let i = $nu.os-info
    {
        os: $i.name
        version: (sys host | get os_version)
        arch: $i.arch
        family: $i.family
        native_manager: (if $i.name == "windows" { "winget" } else { "brew" })
        shell: ($env.SHELL? | default "powershell")
    }
}

# Read the routes the catalogue declares, one row per package. A deliberately
# shallow read of top-level `key: value` lines — Talos itself parses with serde.
export def catalog []: nothing -> table {
    let dir = (catalog-dir)
    if not ($dir | path exists) { return [] }
    # `ls` does not glob a variable-built path in nu — list the dir and filter.
    ls $dir | get name | where {|f| $f | str ends-with ".yaml" } | each {|f|
        let lines = (open --raw $f | lines | where {|l| not ($l | str starts-with "#") })
        def field [key: string]: nothing -> any {
            let hit = ($lines | where {|l| $l | str starts-with $"($key):" } | first)
            if ($hit | is-empty) { null } else {
                $hit | str replace $"($key):" "" | str trim | str trim --char '"' | str trim --char "'"
            }
        }
        {
            id: ($f | path basename | str replace ".yaml" "")
            winget: (field "winget")
            brew: (field "brew")
            plugin: (field "claude-plugin")
            marketplace: (field "marketplace")
            check: (field "check" | is-not-empty)
        }
    }
}

# Which managers are on PATH, and their version. Absence is a normal answer.
export def managers []: nothing -> table {
    [winget brew cargo npm node claude nu] | each {|m|
        let found = ((which $m | length) > 0)
        {
            manager: $m
            present: $found
            version: (if $found {
                (try { ^$m --version | lines | where {|l| $l | is-not-empty } | first | str trim } catch { "?" })
            } else { null })
        }
    }
}

# One bulk listing per manager, timed. This is the cheap shape: constant cost
# instead of one process per package.
export def bulk []: nothing -> table {
    # Each listing is run ONCE and both its duration and its text are kept —
    # timing a command and then re-running it would double the cost and could
    # report a figure that never happened.
    mut out = []
    if (which winget | is-not-empty) {
        let started = (date now)
        let txt = (try { ^winget list --accept-source-agreements | complete | get stdout } catch { "" })
        $out = ($out | append {
            route: "winget", command: "winget list"
            elapsed: ((date now) - $started), lines: ($txt | lines | length)
        })
    }
    if (which brew | is-not-empty) {
        let t1 = (date now)
        let a = (try { ^brew list --versions | complete | get stdout } catch { "" })
        let e1 = ((date now) - $t1)
        let t2 = (date now)
        let b = (try { ^brew list --cask --versions | complete | get stdout } catch { "" })
        $out = ($out
            | append { route: "brew", command: "brew list --versions", elapsed: $e1, lines: ($a | lines | length) }
            | append { route: "brew-cask", command: "brew list --cask --versions", elapsed: ((date now) - $t2), lines: ($b | lines | length) })
    }
    $out
}

# The raw bulk output for a route, used to answer "is this id visible in it?".
def bulk-text [route: string]: nothing -> string {
    match $route {
        "winget" => (try { ^winget list --accept-source-agreements | complete | get stdout } catch { "" })
        "brew" => {
            let a = (try { ^brew list --versions | complete | get stdout } catch { "" })
            let b = (try { ^brew list --cask --versions | complete | get stdout } catch { "" })
            $"($a)\n($b)"
        }
        _ => ""
    }
}

# ⭐ The finding that decides whether a bulk scan can replace per-package
# probing: an id the machine HAS but the bulk listing does not show. `absent`
# alone is fine (not installed); `missing_from_bulk` is the blocker.
export def coverage [
    --deep  # also probe each package individually, to tell absent from missing
]: nothing -> table {
    let cat = (catalog)
    let wtxt = (if (which winget | is-not-empty) { bulk-text "winget" } else { "" })
    let btxt = (if (which brew | is-not-empty) { bulk-text "brew" } else { "" })

    $cat | each {|p|
        let route = (if ($p.winget | is-not-empty) and ($wtxt | is-not-empty) { "winget" } else if ($p.brew | is-not-empty) and ($btxt | is-not-empty) { "brew" } else { null })
        if ($route == null) { null } else {
            let sid = (if $route == "winget" { $p.winget } else { $p.brew })
            let txt = (if $route == "winget" { $wtxt } else { $btxt })
            let in_bulk = ($txt | str contains $sid)
            let probed = (if $deep { probe-one $route $sid } else { null })
            {
                id: $p.id
                route: $route
                system_id: $sid
                in_bulk: $in_bulk
                installed: $probed
                missing_from_bulk: (if $deep { $probed and (not $in_bulk) } else { null })
            }
        }
    } | compact
}

# A single per-package probe — the shape Talos uses today.
def probe-one [route: string, sid: string]: nothing -> bool {
    match $route {
        "winget" => {
            let o = (try { ^winget list --id $sid --exact --source winget --accept-source-agreements | complete | get stdout } catch { "" })
            not ($o | str contains "No installed package")
        }
        "brew" => {
            let a = (try { ^brew list --versions $sid | complete | get stdout } catch { "" })
            if ($a | str trim | is-not-empty) { true } else {
                let b = (try { ^brew list --cask --versions $sid | complete | get stdout } catch { "" })
                ($b | str trim | is-not-empty)
            }
        }
        _ => false
    }
}

# Installed Claude plugins, as Talos reads them: name + version, nothing more.
# ⚠️ The SOURCE is not in this file — see `marketplaces`.
export def plugins []: nothing -> table {
    let f = ([$nu.home-dir ".claude" "plugins" "installed_plugins.json"] | path join)
    if not ($f | path exists) { return [] }
    let d = (open $f)
    $d.plugins | transpose key entries | each {|r|
        let e = ($r.entries | first)
        {
            key: $r.key
            installed: ($e.version? | default "?")
            scope: ($e.scope? | default "")
            updated: ($e.lastUpdated? | default "" | str substring 0..10)
        }
    }
}

# Each marketplace's DECLARED version per plugin. The manifest is NOT always at
# the clone root: marketplace.json declares a `source` per plugin (`./` for one,
# `./plugins/<name>` for another), and that source can even be a nested git URL,
# in which case nothing local can be read.
export def marketplaces []: nothing -> table {
    let root = ([$nu.home-dir ".claude" "plugins" "marketplaces"] | path join)
    if not ($root | path exists) { return [] }
    ls $root | where type == dir | get name | each {|dir|
        let mkt = ([$dir ".claude-plugin" "marketplace.json"] | path join)
        if not ($mkt | path exists) { null } else {
            let m = (try { open $mkt } catch { null })
            ($m.plugins? | default [] | each {|pl|
                let src = ($pl.source? | default "./")
                let nested = (($src | describe) != "string")
                {
                    marketplace: ($dir | path basename)
                    plugin: ($pl.name? | default "?")
                    source: (if $nested { $"nested: ($src | to nuon)" } else { $src })
                    declared: (if $nested { null } else {
                        let pj = ([$dir $src ".claude-plugin" "plugin.json"] | path join)
                        if ($pj | path exists) and (($pj | path type) == "file") {
                            try { open $pj | get version? } catch { null }
                        } else { null }
                    })
                }
            })
        }
    } | compact | flatten
}

# The configured SOURCE of each marketplace. A plugin is the pair name+source:
# two marketplaces can serve the same plugin name from different places, so a
# version match alone does not prove they are the same plugin.
export def sources []: nothing -> table {
    let f = ([$nu.home-dir ".claude" "settings.json"] | path join)
    if not ($f | path exists) { return [] }
    let d = (try { open $f } catch { null })
    ($d.extraKnownMarketplaces? | default {} | transpose name cfg | each {|r|
        let s = ($r.cfg.source? | default {})
        {
            marketplace: $r.name
            kind: ($s.source? | default "?")
            where: ($s.path? | default ($s.repo? | default ($s.url? | default "?")))
        }
    })
}

# ⭐ Installed vs declared, per plugin — the comparison Talos does NOT make yet.
# Verdicts: `outdated` (a real upgrade is due), `up-to-date`, `unverifiable`
# (nothing local to compare against — the honest answer, not a green row).
export def plugin-drift []: nothing -> table {
    let inst = (plugins)
    let decl = (marketplaces)
    let srcs = (sources)
    $inst | each {|p|
        let parts = ($p.key | split row "@")
        let name = ($parts | first)
        let mp = (if ($parts | length) > 1 { $parts | last } else { "" })

        # A `github` marketplace is CLONED under marketplaces/, so its declared
        # version is read there. A `directory` marketplace is not cloned — the
        # source IS on disk, so read it where settings.json says it lives.
        let cfg = ($srcs | where marketplace == $mp)
        let kind = (if ($cfg | is-empty) { null } else { ($cfg | first).kind })
        let hit = ($decl | where marketplace == $mp and plugin == $name)
        let declared = if ($hit | is-not-empty) {
            ($hit | first).declared
        } else if $kind == "directory" {
            declared-in-directory ($cfg | first).where $name
        } else { null }

        {
            plugin: $name
            marketplace: $mp
            kind: $kind
            installed: $p.installed
            declared: $declared
            verdict: (if ($declared == null) { "unverifiable"
                } else if ($declared == $p.installed) { "up-to-date"
                } else { "outdated" })
        }
    }
}

# The version a `directory` marketplace declares for one plugin. Two steps, and
# the first is not optional: marketplace.json says WHERE the plugin manifest
# lives (`./` for one plugin, `./plugins/<name>` for another).
def declared-in-directory [root: path, name: string]: nothing -> any {
    if ($root | is-empty) or (not ($root | path exists)) { return null }
    let mkt = ([$root ".claude-plugin" "marketplace.json"] | path join)
    if not ($mkt | path exists) { return null }
    let m = (try { open $mkt } catch { null })
    let entry = ($m.plugins? | default [] | where {|pl| ($pl.name? | default "") == $name })
    if ($entry | is-empty) { return null }
    let src = (($entry | first).source? | default "./")
    if (($src | describe) != "string") { return null }  # nested git source: nothing local
    let pj = ([$root $src ".claude-plugin" "plugin.json"] | path join)
    if ($pj | path exists) and (($pj | path type) == "file") {
        try { open $pj | get version? } catch { null }
    } else { null }
}

# ⭐ Does a command Talos runs see the same PATH you do?
#
# The field symptom this answers: `git --version` works in your own PowerShell,
# and `claude plugin marketplace add` inside Talos fails saying git is missing.
#
# Talos does not run a command in your shell. On Windows it prefixes EVERY
# command (probe and pty alike, `platform.rs` WIN_PATH_REFRESH) with:
#
#     $env:Path = <Machine registry PATH> + ';' + <User registry PATH>
#
# That is an ASSIGNMENT, not an append — the process PATH is discarded and
# rebuilt from two registry keys. Anything present only in the live process
# environment vanishes: an MSIX/Store shim under WindowsApps, a directory an
# installer added to the session only, or anything your PowerShell profile adds
# (which is never read anyway — the probe passes -NoProfile).
#
# So this compares three PATHs for one executable, and the interesting row is the
# one where `interactive` finds it and `talos_wrap` does not.
#
# ⚠️ The rebuild snippet is READ FROM platform.rs, never retyped. A diagnostic
# that carries its own copy of the thing it diagnoses drifts the day the engine
# changes, and would then describe a Talos that no longer exists — the same reason
# this module derives package ids from catalog/ instead of hardcoding them.
export def path-view [
    exe: string = "git"  # the executable whose visibility is in question
]: nothing -> table {
    let win = ($nu.os-info.name == "windows")
    # 1. What nu sees right now — closest to "your interactive shell".
    let interactive = (which $exe | get path? | default [] | first | default null)

    if not $win {
        # POSIX wraps with the user's own shell in interactive+login, so there is
        # no registry rebuild to lose anything. Reported for symmetry, and because
        # a login shell CAN still differ from this one.
        let login = (try {
            ^($env.SHELL? | default "/bin/zsh") -ilc $"command -v ($exe)" | complete | get stdout | str trim
        } catch { "" })
        return [
            { source: "interactive (nu)", found: ($interactive | is-not-empty), where: $interactive }
            { source: "talos_wrap ($SHELL -ilc)", found: ($login | is-not-empty), where: (if ($login | is-empty) { null } else { $login }) }
        ]
    }

    # 2. The registry-only PATH, rebuilt exactly as Talos does it — read from the
    #    engine, so this cannot describe a wrapping the engine no longer uses.
    let refresh = (win-path-refresh)

    # ⚠️ Built by CONCATENATION, never by `$"..."` interpolation. A PowerShell
    # snippet is full of parentheses, and nu reads `(Get-Command …)` inside an
    # interpolated string as ITS OWN subexpression: it tries to run Get-Command
    # locally, throws, and the catch hands back "" — which then reads as
    # "git not found". That is exactly how this gesture accused the machine of the
    # very defect it was written to detect (field report, 2026-08-09). A probe that
    # cannot run must say so, never answer "absent".
    let resolve = "(Get-Command " + $exe + " -ErrorAction SilentlyContinue).Source"

    let wrapped = (ps-probe ($refresh + " " + $resolve))
    # 3. The SAME powershell WITHOUT the rebuild — the control. If this finds the
    # exe and the wrapped one does not, the rebuild is the cause, full stop.
    let plain = (ps-probe $resolve)

    [
        { source: "interactive (nu)", found: ($interactive | is-not-empty), where: $interactive, note: null }
        { source: "powershell -NoProfile", found: $plain.found, where: $plain.where, note: $plain.note }
        { source: "talos_wrap (PATH rebuilt)", found: $wrapped.found, where: $wrapped.where, note: $wrapped.note }
    ]
}

# The PATH rebuild Talos really prefixes onto every Windows command, READ from
# `src/platform.rs` (WIN_PATH_REFRESH). Reading beats retyping: the whole value of
# this gesture is that it reproduces the ENGINE, and a hardcoded copy would keep
# answering confidently about a wrapping that had changed underneath it.
#
# Falls back to the known literal when the source is unreachable — a kit deployed
# without a checkout still gets an answer, flagged by `source` in `wrapping`.
def win-path-refresh []: nothing -> string {
    let src = ([($MODULE_DIR | path dirname | path dirname) "src" "platform.rs"] | path join)
    let literal = "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');"
    if not ($src | path exists) { return $literal }
    # The const is a single quoted line in Rust; take what is between the quotes.
    let hit = (try {
        open --raw $src | lines
        | where {|l| $l | str contains "GetEnvironmentVariable('Path','Machine')" }
        | where {|l| not ($l | str starts-with "//") }
        | first
    } catch { null })
    if ($hit | is-empty) { return $literal }
    let inner = ($hit | parse --regex '"(?<s>[^"]+)"' | get s? | default [] | first | default "")
    if ($inner | is-empty) { $literal } else { $inner }
}

# ⭐ Where the rebuild snippet came from, and whether it still matches the engine.
# The row to read is `matches_engine: false` — it means this gesture is describing
# a Talos that no longer exists, and every verdict below it is suspect.
export def wrapping []: nothing -> record {
    let src = ([($MODULE_DIR | path dirname | path dirname) "src" "platform.rs"] | path join)
    let from_source = ($src | path exists)
    let used = (win-path-refresh)
    {
        source: (if $from_source { $src } else { "built-in fallback (no checkout beside this module)" })
        assigns_not_appends: ($used | str contains "$env:Path=")
        snippet: $used
        # An APPEND would read `$env:Path=$env:Path+…` or `+=`. Today it does not,
        # which is the whole hypothesis: the live process PATH is discarded.
        keeps_live_path: (($used | str contains '$env:Path=$env:Path') or ($used | str contains '$env:Path+='))
    }
}

# ⭐ Run the gesture that ACTUALLY fails, wrapped exactly as Talos wraps it.
#
# `path-view` answers "can that PATH see git?". This answers the operator's real
# question — "does `claude plugin marketplace add` work when Talos runs it?" —
# because a resolvable git is necessary but not sufficient: claude may look for git
# its own way, and only running the thing settles that.
#
# READ-ONLY: `git --version` and `claude --version` identify themselves; the
# marketplace add is run in --help form, never for real, so nothing is cloned or
# registered.
export def marketplace-add-dry []: nothing -> table {
    if ($nu.os-info.name != "windows") {
        return []  # the rebuild is Windows-only; POSIX wraps with the login shell
    }
    let refresh = (win-path-refresh)
    [
        # Both wrapped, so the comparison is apples to apples: the ONLY difference
        # between the two rows of each pair is the PATH rebuild.
        { what: "git --version (talos wrap)",     ps: ($refresh + " git --version") }
        { what: "git --version (no wrap)",        ps: "git --version" }
        { what: "claude --version (talos wrap)",  ps: ($refresh + " claude --version") }
        # What `marketplace add` needs from git, without touching anything: --help
        # exits 0 only if the git binary is really there and runnable.
        { what: "git clone --help (talos wrap)",  ps: ($refresh + " git clone --help") }
    ] | each {|c|
        let r = (ps-probe $c.ps)
        {
            what: $c.what
            # Judged on the EXIT CODE, not on "was there output": these are real
            # commands, and `git clone --help` prints a wall of text whether or not
            # git was found. Exit 0 is the only thing that means "this worked".
            ok: ($r.code == 0)
            # First line only: a version string, or the error that explains the row.
            answer: (if ($r.where | is-empty) { null } else { $r.where | lines | first | str trim })
            note: $r.note
        }
    }
}

# Run one PowerShell snippet and report WHAT HAPPENED, not just whether output
# came back. `found: false` means the exe was not on that PATH; `note` carries the
# reason when the probe itself could not run — the two must never look alike.
def ps-probe [snippet: string]: nothing -> record {
    if (which powershell.exe | is-empty) {
        return { found: false, where: null, note: "powershell.exe not on PATH — probe did not run" }
    }
    let r = (do { ^powershell.exe -NoProfile -Command $snippet } | complete)
    let out = ($r.stdout? | default "" | str trim)
    let err = ($r.stderr? | default "" | str trim)
    let code = ($r.exit_code? | default 0)
    if $code != 0 and ($out | is-empty) {
        # A non-zero exit with nothing on stdout is a BROKEN PROBE, not an absence.
        # ⚠️ 127 is what Talos's own wrapper returns for CommandNotFoundException
        # (platform.rs), so it is worth naming rather than lumping in.
        let why = (if $code == 127 { "command not found (exit 127)" } else { $"exit ($code)" })
        return { found: false, where: null, note: ($"probe failed: ($why) ($err)" | str trim), code: $code }
    }
    # `code` travels so a caller running a REAL command (not just resolving a name)
    # can judge on the exit status instead of on "was there output" — `git clone
    # --help` prints plenty either way.
    { found: ($out | is-not-empty), where: (if ($out | is-empty) { null } else { $out }), note: null, code: $code }
}

# The PATH entries Talos LOSES by rebuilding from the registry.
#
# Windows-only and deliberately blunt: it diffs the live process PATH against
# `Machine + User`, and every returned row is a directory a Talos-run command
# cannot see. If `git` lives in one of them, the diagnosis is closed.
export def path-lost []: nothing -> table {
    if ($nu.os-info.name != "windows") {
        return []  # POSIX wraps with the user's own login shell; nothing is rebuilt
    }
    let live = (try {
        ^powershell.exe -NoProfile -Command "$env:Path" | complete | get stdout | str trim
    } catch { "" })
    let registry = (try {
        ^powershell.exe -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User')"
        | complete | get stdout | str trim
    } catch { "" })

    def split-path [s: string]: nothing -> list<string> {
        $s | split row ";" | each {|d| $d | str trim | str trim --right --char '\' } | where {|d| $d | is-not-empty }
    }
    let live_dirs = (split-path $live)
    let reg_dirs = (split-path $registry)

    $live_dirs | where {|d| not ($d in $reg_dirs) } | each {|d|
        {
            lost_dir: $d
            exists: ($d | path exists)
            # What a lost directory actually costs: the executables in it. Named
            # rather than counted, because "git.exe was in there" ends the hunt.
            executables: (if ($d | path exists) {
                (try { ls $d | get name | path basename | where {|f| $f | str ends-with ".exe" } | first 6 } catch { [] })
            } else { [] })
        }
    }
}

# Can this machine reach what a package manager needs? Talos does not fetch
# these itself, but a corporate firewall answering 403 is a known failure mode.
export def reachability []: nothing -> table {
    [https://github.com https://raw.githubusercontent.com https://formulae.brew.sh https://registry.npmjs.org] | each {|u|
        let started = (date now)
        # `--full` is what carries the status; a bare response cannot answer
        # "did this 403?". MEASURED: `http head` raises an I/O error on hosts
        # that refuse HEAD (github.com among them), so this uses `get`.
        let r = (try { http get --full --allow-errors --max-time 10sec $u } catch { null })
        {
            url: $u
            status: (if ($r | is-empty) { "unreachable" } else { $r | get status? | default "?" })
            elapsed: ((date now) - $started)
        }
    }
}

# The whole picture. Prints each section, and with --save writes a timestamped
# report to attach to an issue.
# Capture, VERBATIM, what this machine prints — no parsing, no verdicts. The
# bytes are the evidence: a fixture like tests/fixtures/managers/ is made of
# exactly this, and interpreting the output here would throw the finding away.
#
# Runs from a checkout on either platform: the commands are the ones Talos
# itself runs, and absence of a manager is recorded rather than fatal.
export def capture [
    --out: path        # folder to write into (default: talos-capture-<stamp> here)
    --package: string  # the id whose row was wrong — adds its per-package probes
    --stamp: string    # timestamp for the default folder name
]: nothing -> path {
    let ts = (if ($stamp | is-not-empty) { $stamp } else { (date now | format date "%Y%m%d-%H%M%S") })
    let dir = (if ($out | is-not-empty) { $out } else { $"talos-capture-($ts)" })
    mkdir $dir

    # One row per command: what was asked, how long it took, where it landed.
    # `complete` keeps stdout, stderr AND the exit code — a non-zero exit is
    # itself evidence, so nothing is discarded.
    let cmds = ([
        { name: "winget-list",     exe: "winget", args: [list --accept-source-agreements] }
        { name: "winget-upgrade",  exe: "winget", args: [upgrade --accept-source-agreements --source winget] }
        { name: "brew-list",       exe: "brew",   args: [list --versions] }
        { name: "brew-list-cask",  exe: "brew",   args: [list --cask --versions] }
        { name: "brew-outdated",   exe: "brew",   args: [outdated --greedy-auto-updates --json=v2] }
    ] | append (if ($package | is-not-empty) { [
        { name: "probe-winget-list",   exe: "winget", args: [list --id $package --exact --source winget --accept-source-agreements] }
        { name: "probe-brew-versions", exe: "brew",   args: [list --versions $package] }
        { name: "probe-brew-cask",     exe: "brew",   args: [list --cask --versions $package] }
    ] } else { [] }))

    let rows = ($cmds | each {|c|
        if (which $c.exe | is-empty) {
            { command: $"($c.exe) ($c.args | str join ' ')", status: $"skipped: ($c.exe) not on PATH", elapsed: null, lines: 0 }
        } else {
            let started = (date now)
            let r = (try { ^$c.exe ...$c.args | complete } catch { { stdout: "", stderr: $"failed to run", exit_code: -1 } })
            let elapsed = ((date now) - $started)
            let body = $"# ($c.exe) ($c.args | str join ' ')\n# captured (date now | format date '%+'), exit ($r.exit_code?), in ($elapsed)\n\n($r.stdout?)($r.stderr?)"
            $body | save --force ([$dir $"($c.name).txt"] | path join)
            { command: $"($c.exe) ($c.args | str join ' ')", status: $"exit ($r.exit_code?)", elapsed: $elapsed, lines: ($r.stdout? | default "" | lines | length) }
        }
    })

    # The files Talos READS, copied as-is: presence for a plugin comes from these,
    # not from a command.
    let claude = ([$nu.home-dir ".claude"] | path join)
    for pair in [
        [([$claude "plugins" "installed_plugins.json"] | path join), "installed_plugins.json"]
        [([$claude "settings.json"] | path join), "claude-settings.json"]
    ] {
        let src = ($pair | first)
        if ($src | path exists) { cp $src ([$dir ($pair | last)] | path join) }
    }

    # Each marketplace's own manifest — the version a plugin SHOULD be at. Copied
    # whole, because the manifest is not always at the clone root.
    let mkt = ([$claude "plugins" "marketplaces"] | path join)
    if ($mkt | path exists) {
        let mdest = ([$dir "marketplaces"] | path join)
        mkdir $mdest
        for d in (ls $mkt | where type == dir | get name) {
            let m = ([$d ".claude-plugin" "marketplace.json"] | path join)
            if ($m | path exists) { cp $m ([$mdest $"($d | path basename).marketplace.json"] | path join) }
        }
    }

    # ⭐ The PATH as Talos rebuilds it, captured VERBATIM. A `marketplace add` that
    # cannot find git leaves no trace in any listing above — this is where the
    # evidence for that lives, and it costs two cheap commands.
    if ($nu.os-info.name == "windows") {
        let refresh = "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');"
        for probe in [
            [ "path-live",        "$env:Path" ]
            [ "path-registry",    "[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User')" ]
            [ "git-plain",        "(Get-Command git -ErrorAction SilentlyContinue).Source" ]
            # ⚠️ Concatenated, NOT interpolated: nu reads `(Get-Command …)` inside a
            # `$"..."` as its own subexpression and tries to run it locally. See
            # `ps-probe` — that mistake made this very gesture report a false
            # "git not found" in the field.
            [ "git-talos-wrap",   ($refresh + " (Get-Command git -ErrorAction SilentlyContinue).Source") ]
            # The failing gesture itself, wrapped exactly as Talos wraps it. Read-only:
            # --version asks git to identify itself, it clones nothing.
            [ "git-version-talos-wrap", ($refresh + " git --version") ]
        ] {
            let label = ($probe | first)
            let ps = ($probe | last)
            let r = (try { ^powershell.exe -NoProfile -Command $ps | complete } catch { { stdout: "", stderr: "failed to run", exit_code: -1 } })
            $"# powershell -NoProfile -Command ($ps)\n# exit ($r.exit_code?)\n\n($r.stdout?)($r.stderr?)"
            | save --force ([$dir $"($label).txt"] | path join)
        }
    }

    {
        captured: (date now | format date "%+")
        machine: (os)
        managers: (managers)
        commands: $rows
        wrapping: (wrapping)
        path_view: (path-view "git")
        path_lost: (path-lost)
        marketplace_add_dry: (marketplace-add-dry)
    } | to yaml | save --force ([$dir "INDEX.yaml"] | path join)

    print $"capture written to: ($dir)"
    print "Zip it and attach it, with what you EXPECTED versus what Talos SHOWED."
    $rows | print
    $dir
}

export def main [
    --deep          # also probe per package (slower; distinguishes absent from missing)
    --save: string  # write the report to this path; omit the value for a timestamped default
    --stamp: string # timestamp to use in the default filename (default: now)
]: nothing -> nothing {
    print $"talos doctor — (date now | format date '%Y-%m-%d %H:%M:%S')"
    print $"  catalogue: (catalog-dir)"
    print $"  deep:      ($deep)"

    print "\n=== managers ==="
    managers | print

    print "\n=== catalogue ==="
    let cat = (catalog)
    print $"  ($cat | length) packages — winget ($cat | where winget != null | length), brew ($cat | where brew != null | length), plugins ($cat | where plugin != null | length)"

    print "\n=== bulk listing (one call per manager) ==="
    bulk | print

    print "\n=== coverage: is each catalogue id visible in the bulk listing? ==="
    let cov = (coverage --deep=$deep)
    $cov | print
    if $deep {
        let blockers = ($cov | where missing_from_bulk? | default false)
        if ($blockers | is-empty) {
            print "  ✔ nothing installed is missing from the bulk listing — a bulk scan is safe here"
        } else {
            print "  ⚠️ INSTALLED BUT ABSENT FROM THE BULK LISTING — report these:"
            $blockers | print
        }
    } else {
        print "  (pass --deep to tell 'not installed' apart from 'missing from the listing')"
    }

    # ⭐ Placed BEFORE the plugin section on purpose: a plugin install that cannot
    # find git fails here, not in the drift table, and reading the tables in that
    # order is what tells the two apart.
    print "\n=== PATH: does a Talos-run command see what you see? ==="
    # The wrapping FIRST: it says where the snippet came from, and a snippet read
    # from a stale source would make every row below it meaningless.
    let wrap = (wrapping)
    $wrap | print
    let pv = (path-view "git")
    $pv | print

    # ⚠️ A probe that could not RUN must never be read as "git is absent" — that is
    # the defect this section shipped with once (nu ate the PowerShell parentheses
    # and both powershell rows came back false on a healthy machine).
    let broken = ($pv | where note != null)
    if ($broken | is-not-empty) {
        print "  ⚠️ SOME PROBES DID NOT RUN — the rows below are not evidence of anything:"
        $broken | select source note | print
    }

    let lost = (path-lost)
    let wrapped_blind = ($pv | where source =~ "talos_wrap" | where note == null | where not found | is-not-empty)
    let elsewhere_ok = ($pv | where source !~ "talos_wrap" | where note == null | where found | is-not-empty)
    if $wrapped_blind and $elsewhere_ok {
        print "  ⚠️ git is on YOUR PATH but NOT on the one Talos builds."
        print "     Talos rebuilds \$env:Path from the Machine+User registry keys, so anything"
        print "     living only in the live process environment disappears — and a"
        print "     `claude plugin marketplace add` shells out to git, so it fails."
        if ($lost | is-not-empty) {
            print "     Directories lost to that rebuild:"
            $lost | print
        }
    } else if ($lost | is-not-empty) {
        print "  These directories are on the live PATH but NOT in the registry — a"
        print "  Talos-run command cannot see them (harmless unless a tool lives there):"
        $lost | print
    }

    # ⭐ Resolving a name is necessary, not sufficient. This RUNS the gesture that
    # actually fails, wrapped the way Talos wraps it — the only thing that settles
    # whether `marketplace add` can work.
    let dry = (marketplace-add-dry)
    if ($dry | is-not-empty) {
        print "\n=== the failing gesture, run as Talos runs it (read-only) ==="
        $dry | print
        let wrapped_fails = ($dry | where what =~ "talos wrap" | where not ok | is-not-empty)
        let unwrapped_ok = ($dry | where what =~ "no wrap" | where ok | is-not-empty)
        if $wrapped_fails and $unwrapped_ok {
            print "  ⚠️ CONFIRMED: the command works unwrapped and FAILS with Talos's wrapping."
            print "     The PATH rebuild is the cause. Fix = make it APPEND to \$env:Path"
            print "     instead of assigning over it (src/platform.rs WIN_PATH_REFRESH)."
        } else if (not $wrapped_fails) {
            print "  ✔ git and claude answer under Talos's wrapping — the PATH is NOT the cause."
            print "    If a marketplace add still fails, capture its output: the cause is elsewhere."
        }
    }

    print "\n=== claude plugins: installed vs declared ==="
    plugin-drift | print

    print "\n=== marketplace sources (a plugin is name+SOURCE) ==="
    sources | print

    print "\n=== reachability ==="
    reachability | print

    if ($save | is-not-empty) or ($save != null) {
        let ts = (if ($stamp | is-not-empty) { $stamp } else { (date now | format date "%Y%m%d-%H%M%S") })
        let path = (if ($save | is-not-empty) { $save } else { $"talos-doctor-($ts).txt" })
        {
            generated: (date now | format date "%+")
            host: (sys host | get hostname)
            os: $"(sys host | get name) (sys host | get os_version)"
            managers: (managers)
            catalogue: $cat
            bulk: (bulk)
            coverage: $cov
            wrapping: $wrap
            path_view: $pv
            path_lost: $lost
            marketplace_add_dry: $dry
            plugins: (plugin-drift)
            sources: (sources)
            reachability: (reachability)
        } | to yaml | save --force $path
        print $"\nreport written to: ($path)"
        print "Attach it to the issue, with what you EXPECTED versus what Talos SHOWED."
    }
}
