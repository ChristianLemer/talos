# get-talos.ps1 -- compose a Talos kit on Windows: the launchers of ONE release, verified,
# plus your content; or verify a kit that is already there. The `sh` twin of this script,
# `get-talos.sh`, composes the very same kit on macOS and Linux.
#
#   irm https://github.com/ChristianLemer/talos/releases/latest/download/get-talos.ps1 | iex
#
#   # with arguments -- `iex` cannot pass any, so build the scriptblock yourself:
#   & ([scriptblock]::Create((irm .../get-talos.ps1))) -Kit "C:\Kits\Talos" -Content .
#
#   .\get-talos.ps1 <kit folder> [-Version TAG] [-Content DIR]
#   .\get-talos.ps1 <kit folder> -Verify
#
#   -Kit <folder>   where the kit lives -- the shared folder your team launches from, or a
#                   folder of your own you will double-click in. Piped through `iex` there
#                   is no way to pass it, so it defaults to `$env:USERPROFILE\Talos` and
#                   the script says so on screen before it writes anything
#   -Version TAG    the release to fetch (v0.0.1-beta.42). Default: the `.talos-version`
#                   file beside the content (or in the current directory) if there is one,
#                   else the newest release
#   -Content DIR    a folder holding `catalog\` and `bundles\`: YOUR content, copied into
#                   the kit in place of the socle. Without it, the kit gets the SOCLE of
#                   the same release -- `talos-content.zip`, what a stranger receives
#   -Verify         recompute every file in the kit against MANIFEST.sha256 and exit 1 on
#                   the first difference. Offline; run it on any synced replica
#
# *** THE KIT IT COMPOSES -- identical to what `get-talos.sh` composes, because the folder
# does not record which machine made it:
#
#   Talos\
#     MacOS\     Talos.app
#     Windows\   Talos.exe
#     Linux\     Talos
#     catalog\   bundles\
#     .talos\    VERSION, KIT.txt, MANIFEST.sha256
#
# One folder per OS, so every launcher keeps its own standard name -- `Talos` next to
# `Talos.exe` in one folder would show as two identical rows in an Explorer that hides
# extensions, and one of them does nothing when double-clicked. Whoever receives the kit
# opens the name of their OS and double-clicks. The metadata is machine-facing, never
# opened by hand, so it sits out of the way under `.talos\`.
#
# *** WINDOWS POWERSHELL 5.1 IS THE FLOOR, deliberately, and nothing here needs pwsh 7.
# This is the tool that installs the tool that installs everything else: a bootstrap may
# lean only on what the OS ships. Windows ships 5.1. Requiring pwsh 7 would mean asking
# someone to install a shell in order to run the installer -- the same reasoning that keeps
# `get-talos.sh` in plain `sh` rather than nushell. Hence: TLS 1.2 forced (5.1 does not
# negotiate it by default and GitHub refuses anything older), `$ProgressPreference` muted
# (`Invoke-WebRequest` is an order of magnitude slower with the progress bar), no operator
# and no cmdlet introduced after 5.1. The pin against drift is PSScriptAnalyzer's
# `PSUseCompatibleSyntax` targeting 5.1.
#
# ... and PURE ASCII, which is part of the same floor. Windows PowerShell 5.1 reads a
# BOM-less file as the system ANSI code page, so a UTF-8 em dash or a star in a comment
# arrives as mojibake -- and PSScriptAnalyzer will tell you to add a BOM to fix that. Do
# NOT: the entry point here is `irm | iex`, where the bytes never touch a file and a
# leading BOM becomes a stray character in front of the first token. ASCII is the one
# encoding that is unambiguous down both paths. The rest of the repo keeps its glyphs;
# this file pays for being executable straight off the wire.
#
# *** THE KIT IS CHECKED BY THE ENGINE IT SHIPS, in the position it will be read from:
# `--check` with NO arguments on the launcher just placed, so the engine resolves
# `catalog\` and `bundles\` itself -- the content AND the layout, proven not assumed.
#
# *** EVERY BYTE IS VERIFIED, twice: against the sha256 GitHub publishes for each asset at
# download, then into `.talos\MANIFEST.sha256` over the files as they sit in the kit. That
# manifest is ONE artefact read by three tools -- `Get-FileHash` here, `shasum -a 256` on
# macOS, `sha256sum` on Linux -- so a kit composed on Windows verifies on a colleague's
# Mac. Its shape is `sha256sum`'s and is followed to the byte: lowercase hex, two spaces,
# forward slashes, LF endings, no BOM, ordinal sort.
#
# !! The newest tag is read from the releases list, not from GitHub's "latest": that link
# skips pre-releases, and every Talos release is a pre-release until v0.1.0.
#
# !! A running exe on a shared drive: Windows locks `Talos.exe` while it runs, so the sync
# cannot replace it on a machine where Talos is open -- it retries, and the new exe lands
# at the next sync after Talos is closed there.
[CmdletBinding()]
param(
    [Parameter(Position = 0)][string]$Kit,
    [string]$Version,
    [string]$Content,
    [switch]$Verify
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# 5.1 negotiates SSL3/TLS1.0 by default; github.com and its API refuse both.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# 5.1 only ever runs on Windows, so it has no $IsWindows and needs none; pwsh 7 does, and
# this script stays honest under it on a Mac or a Linux box (which is also how it is tested).
$IsWindowsHost = $true
if ($PSVersionTable.PSVersion.Major -ge 6) { $IsWindowsHost = $IsWindows }

$Repo = 'ChristianLemer/talos'
$Api = "https://api.github.com/repos/$Repo/releases"
$MacAsset = 'Talos-macos-aarch64.app.zip'
$WinAsset = 'Talos.exe'
$LnxAsset = 'Talos-linux-x86_64'
$ContentAsset = 'talos-content.zip'

function Fail([string]$Message) {
    # NOT Write-Error: with $ErrorActionPreference = 'Stop' that throws, and a throw inside
    # the try/finally below buries a clear message under PowerShell noise. Stderr and an
    # exit code are what a caller reads.
    [Console]::Error.WriteLine($Message)
    exit 1
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Slashes forward, because the manifest is read on three OSes; separators back, because
# this is where a path is used. One helper each way.
function ConvertTo-KitPath([string]$Root, [string]$Rel) {
    return (Join-Path $Root ($Rel.Replace('/', [string][IO.Path]::DirectorySeparatorChar)))
}

# .NET Framework 4.x has no Path.GetRelativePath (it arrived with .NET Core), so 5.1 needs
# this by hand.
function Get-KitRelativePath([string]$Root, [string]$Full) {
    $prefix = [IO.Path]::GetFullPath($Root).TrimEnd([IO.Path]::DirectorySeparatorChar) +
        [IO.Path]::DirectorySeparatorChar
    return [IO.Path]::GetFullPath($Full).Substring($prefix.Length).Replace('\', '/')
}

# The kit's own artefacts, in the manifest's order: every launcher, every content file.
# Never `.talos\` -- that folder IS the bookkeeping, and a manifest that covered itself
# could not be written.
function Get-KitFileList([string]$Root) {
    $rels = New-Object System.Collections.ArrayList
    foreach ($n in @('MacOS', 'Windows', 'Linux', 'catalog', 'bundles')) {
        $dir = Join-Path $Root $n
        if (-not (Test-Path -LiteralPath $dir)) { continue }
        foreach ($f in Get-ChildItem -LiteralPath $dir -Recurse -File -Force) {
            [void]$rels.Add((Get-KitRelativePath $Root $f.FullName))
        }
    }
    $arr = $rels.ToArray([string])
    # Ordinal, to agree with `LC_ALL=C sort` on the other side. PowerShell's default sort is
    # culture-aware and would not put the same lines in the same order.
    [Array]::Sort($arr, [StringComparer]::Ordinal)
    return , $arr
}

# A launcher that cannot be executed is not a launcher. Windows has no such bit and there
# is nothing to do there; every other host has one, and neither Invoke-WebRequest nor
# Expand-Archive sets it -- the download lands 644 and the .app's binaries come out of the
# zip flat. So restore it wherever the notion exists.
function Set-LauncherExecBit([string]$Path) {
    if ($IsWindowsHost) { return }
    if (-not (Test-Path -LiteralPath $Path)) { return }
    & chmod '+x' $Path
}

# LF, no BOM. 5.1's `-Encoding UTF8` writes a BOM, and a BOM on the first line makes
# `sha256sum -c` read a 67-character hash; CRLF makes it read a filename ending in \r.
function Write-TextLf([string]$Path, [string[]]$Lines) {
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, (($Lines -join "`n") + "`n"), $utf8NoBom)
}

if (-not $Kit) {
    $base = $env:USERPROFILE
    if (-not $base) { $base = $HOME }
    $Kit = Join-Path $base 'Talos'
    Write-Host "no -Kit given (as when piped through iex) - composing in $Kit"
}

# -- -Verify: the replica against its manifest, nothing else ---------------------------
if ($Verify) {
    if (-not (Test-Path -LiteralPath $Kit)) { Fail "no such folder: $Kit" }
    $KitFull = (Resolve-Path -LiteralPath $Kit).ProviderPath
    # `.talos\` is where a kit composed by this version keeps it; the root is where kits
    # composed before the OS folders kept it. Both are verifiable, neither is re-composed.
    $manifest = $null
    foreach ($candidate in @((Join-Path (Join-Path $KitFull '.talos') 'MANIFEST.sha256'),
                             (Join-Path $KitFull 'MANIFEST.sha256'))) {
        if (Test-Path -LiteralPath $candidate) { $manifest = $candidate; break }
    }
    if (-not $manifest) { Fail "no MANIFEST.sha256 in $Kit - not a kit these scripts composed" }

    $bad = 0
    $seen = 0
    foreach ($line in [IO.File]::ReadAllLines($manifest)) {
        if (-not $line.Trim()) { continue }
        # `<64 hex><two spaces><path>` -- sha256sum's shape, and shasum's.
        $m = [regex]::Match($line, '^([0-9a-fA-F]{64})\s\s?(.+)$')
        if (-not $m.Success) { Write-Host "manifest line not understood: $line"; $bad++; continue }
        $seen++
        $rel = $m.Groups[2].Value.Trim()
        $full = ConvertTo-KitPath $KitFull $rel
        if (-not (Test-Path -LiteralPath $full)) { Write-Host "$rel : MISSING"; $bad++; continue }
        if ((Get-Sha256 $full) -ne $m.Groups[1].Value.ToLowerInvariant()) {
            Write-Host "$rel : FAILED"; $bad++
        }
    }
    $versionFile = Join-Path (Join-Path $KitFull '.talos') 'VERSION'
    if (-not (Test-Path -LiteralPath $versionFile)) { $versionFile = Join-Path $KitFull 'VERSION' }
    $tagSeen = '?'
    if (Test-Path -LiteralPath $versionFile) {
        $tagSeen = (Get-Content -LiteralPath $versionFile -TotalCount 1).Trim()
    }
    if ($bad -gt 0) {
        Fail "kit in $Kit : $bad of $seen files differ from the manifest - a half-synced replica, or a hand edit"
    }
    Write-Host "kit $tagSeen in $Kit : all $seen files match their manifest"
    exit 0
}

if ($Content) {
    if (-not (Test-Path -LiteralPath (Join-Path $Content 'catalog')) -or
        -not (Test-Path -LiteralPath (Join-Path $Content 'bundles'))) {
        Fail "$Content has no catalog\ + bundles\"
    }
}

# The version: the flag, else the pin file beside the content (or in the cwd), else newest.
if (-not $Version) {
    $pinDir = $Content
    if (-not $pinDir) { $pinDir = '.' }
    $pin = Join-Path $pinDir '.talos-version'
    if (Test-Path -LiteralPath $pin) { $Version = (Get-Content -LiteralPath $pin -TotalCount 1).Trim() }
}
if (-not $Version) {
    # The first entry in the API's list is the newest release, pre-release or not.
    $newest = Invoke-RestMethod -Uri "${Api}?per_page=1" -UseBasicParsing
    if (-not $newest) { Fail "no release found on $Repo" }
    $Version = @($newest)[0].tag_name
}

New-Item -ItemType Directory -Force -Path $Kit | Out-Null
$KitFull = (Resolve-Path -LiteralPath $Kit).ProviderPath
$Tmp = Join-Path ([IO.Path]::GetTempPath()) ('talos-kit-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
try {
    $dotTalos = Join-Path $KitFull '.talos'
    $versionFile = Join-Path $dotTalos 'VERSION'
    $manifestFile = Join-Path $dotTalos 'MANIFEST.sha256'
    if ((Test-Path -LiteralPath $versionFile) -and (Test-Path -LiteralPath $manifestFile) -and
        (-not $Content) -and (Test-Path -LiteralPath (Join-Path $KitFull 'catalog')) -and
        ((Get-Content -LiteralPath $versionFile -TotalCount 1).Trim() -eq $Version)) {
        Write-Host "kit already at $Version (run with -Verify to check the replica)"
        exit 0
    }

    # What GitHub says each asset of this release weighs and hashes, from the same record a
    # client would use to find the tag. Real JSON here, where the sh twin must grep.
    $release = Invoke-RestMethod -Uri "$Api/tags/$Version" -UseBasicParsing
    $published = @{}
    foreach ($a in $release.assets) {
        if ($a.digest) {
            $published[$a.name] = @{ size = $a.size; digest = ($a.digest -replace '^sha256:', '') }
        }
    }
    if ($published.Count -eq 0) {
        Fail "release $Version publishes no asset digests - refusing to compose an unverifiable kit"
    }

    # The three launchers always -- the kit does not depend on who composed it; the socle
    # only when no content is given.
    $assets = @($MacAsset, $WinAsset, $LnxAsset)
    if (-not $Content) { $assets += $ContentAsset }
    foreach ($asset in $assets) {
        if (-not $published.ContainsKey($asset)) {
            Fail "release $Version has no digest for $asset - refusing"
        }
        $want = $published[$asset].digest
        $out = Join-Path $Tmp $asset
        Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$Version/$asset" `
            -OutFile $out -UseBasicParsing
        $got = Get-Sha256 $out
        if ($got -ne $want) { Fail "$asset : sha256 $got does not match the published $want - refusing" }
        Write-Host "verified $asset"
    }

    # The .app: four files and no symlink, measured on the real asset, so Expand-Archive is
    # as faithful here as ditto is on a Mac.
    $macTmp = Join-Path $Tmp 'mac'
    Expand-Archive -LiteralPath (Join-Path $Tmp $MacAsset) -DestinationPath $macTmp -Force

    foreach ($n in @('MacOS', 'Windows', 'Linux')) {
        $d = Join-Path $KitFull $n
        if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $d | Out-Null
    }
    New-Item -ItemType Directory -Force -Path $dotTalos | Out-Null
    Move-Item -LiteralPath (Join-Path $macTmp 'Talos.app') `
        -Destination (Join-Path (Join-Path $KitFull 'MacOS') 'Talos.app')
    Move-Item -LiteralPath (Join-Path $Tmp $WinAsset) `
        -Destination (Join-Path (Join-Path $KitFull 'Windows') 'Talos.exe')
    Move-Item -LiteralPath (Join-Path $Tmp $LnxAsset) `
        -Destination (Join-Path (Join-Path $KitFull 'Linux') 'Talos')
    Set-LauncherExecBit (Join-Path (Join-Path $KitFull 'Linux') 'Talos')
    $appBin = Join-Path (Join-Path (Join-Path (Join-Path $KitFull 'MacOS') 'Talos.app') 'Contents') 'MacOS'
    if (Test-Path -LiteralPath $appBin) {
        foreach ($f in Get-ChildItem -LiteralPath $appBin -File) { Set-LauncherExecBit $f.FullName }
    }

    # The content: yours, or the socle of this release. REPLACED, not merged -- a package
    # deleted from the source must leave the kit too, or the kit keeps proposing what the
    # author removed.
    foreach ($n in @('catalog', 'bundles')) {
        $d = Join-Path $KitFull $n
        if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force }
    }
    if ($Content) {
        Copy-Item -LiteralPath (Join-Path $Content 'catalog') -Destination (Join-Path $KitFull 'catalog') -Recurse
        Copy-Item -LiteralPath (Join-Path $Content 'bundles') -Destination (Join-Path $KitFull 'bundles') -Recurse
        Write-Host "content from $Content"
    }
    else {
        Expand-Archive -LiteralPath (Join-Path $Tmp $ContentAsset) -DestinationPath $KitFull -Force
        Write-Host "content: the socle of $Version"
    }

    # -- Checked by the engine it ships, from where it will be read --------------------
    # Two Windows facts decide the shape of this call, and both are load-bearing:
    #
    #  . `Talos.exe` is a GUI-subsystem program. `& $exe` does NOT block on one -- the
    #    script would run on while the check was still reading -- and its stdout never
    #    reaches a bare console. Start-Process -Wait gives the exit code (the contract),
    #    -RedirectStandardOutput gives the text (the bonus, per main.rs).
    #  . -WorkingDirectory is an EMPTY folder, and that is the whole point. The engine's
    #    last resort is the current directory -- the dev fallback for `cargo run` at a repo
    #    root. Run this from a content repo and that fallback quietly answers with the
    #    REPO's catalog: the kit is never read, and a broken layout reports "0 errors".
    #    Measured, on the first run of the sh twin. An empty cwd leaves only the kit.
    $checkCwd = Join-Path $Tmp 'empty'
    New-Item -ItemType Directory -Force -Path $checkCwd | Out-Null
    $checkOut = Join-Path $Tmp 'check.txt'
    $checker = Join-Path (Join-Path $KitFull 'Windows') 'Talos.exe'
    if (-not $IsWindowsHost) { $checker = Join-Path (Join-Path $KitFull 'Linux') 'Talos' }
    $p = Start-Process -FilePath $checker -ArgumentList '--check' -WorkingDirectory $checkCwd `
        -NoNewWindow -Wait -PassThru -RedirectStandardOutput $checkOut
    if (Test-Path -LiteralPath $checkOut) { Get-Content -LiteralPath $checkOut | Write-Host }
    if ($p.ExitCode -ne 0) {
        # Two very different causes, and the fix is not the same. Ask the same engine the
        # same question with the paths SPELLED OUT: if it passes that way, the content is
        # sound and what it cannot do is FIND it -- an engine older than the OS folders,
        # which only ever looked beside itself. Say which, rather than blame the content.
        $p2 = Start-Process -FilePath $checker `
            -ArgumentList '--check', (Join-Path $KitFull 'catalog'), (Join-Path $KitFull 'bundles') `
            -WorkingDirectory $checkCwd -NoNewWindow -Wait -PassThru `
            -RedirectStandardOutput (Join-Path $Tmp 'check2.txt')
        Remove-Item -LiteralPath $dotTalos -Recurse -Force -ErrorAction SilentlyContinue
        if ($p2.ExitCode -eq 0) {
            Fail ("$Version ships an engine that predates the kit's OS folders: it reads the " +
                  "content only from beside itself, so it cannot find catalog\ one level up. " +
                  "Compose with a newer -Version, or pin one in .talos-version. Kit NOT published.")
        }
        Fail "the composed kit does not pass Talos --check - kit NOT published"
    }

    # What is on disk now, file by file, in the format `sha256sum -c` reads back --
    # relative to the kit, so the manifest travels with the replica.
    $lines = New-Object System.Collections.ArrayList
    foreach ($rel in (Get-KitFileList $KitFull)) {
        [void]$lines.Add((Get-Sha256 (ConvertTo-KitPath $KitFull $rel)) + '  ' + $rel)
    }
    Write-TextLf $manifestFile $lines.ToArray([string])
    Write-TextLf $versionFile @($Version)

    $kitTxt = New-Object System.Collections.ArrayList
    [void]$kitTxt.Add("talos $Version")
    [void]$kitTxt.Add("composed $((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')) by get-talos.ps1")
    [void]$kitTxt.Add("release https://github.com/$Repo/releases/tag/$Version")
    [void]$kitTxt.Add('')
    [void]$kitTxt.Add('assets as published (name size sha256), verified at download:')
    foreach ($asset in $assets) {
        [void]$kitTxt.Add("  $asset $($published[$asset].size) sha256:$($published[$asset].digest)")
    }
    [void]$kitTxt.Add('')
    if ($Content) { [void]$kitTxt.Add("content: $Content (the integrator's)") }
    else { [void]$kitTxt.Add("content: the socle of $Version") }
    [void]$kitTxt.Add('')
    [void]$kitTxt.Add('files as placed: MANIFEST.sha256 - check any replica with: get-talos.ps1 <kit> -Verify')
    Write-TextLf (Join-Path $dotTalos 'KIT.txt') $kitTxt.ToArray([string])

    Write-Host "kit at $Version : MacOS\ + Windows\ + Linux\ + content, checked in place, manifest written"
    Write-Host "kit ready in $KitFull"
}
finally {
    Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
