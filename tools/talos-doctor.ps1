<#
.SYNOPSIS
  talos-doctor — collect a diagnostic report about this machine's package
  managers and Claude plugins, so a problem can be reported without a
  back-and-forth.

.DESCRIPTION
  Talos decides what to install by OBSERVING the machine. When it gets that
  wrong, the useful question is always "what did the machine actually answer?".
  This script asks, and writes the answers to a timestamped file you can attach
  to an issue.

  It is READ-ONLY by design: it lists, queries and reads files. It never
  installs, upgrades, uninstalls, or writes anything outside the report file.
  Every command is non-interactive.

  Discovery is DERIVED, never hardcoded: package ids come from the catalog
  folder if one is next to the executable, otherwise from what the machine
  already has. So the report stays meaningful as the catalogue evolves.

.PARAMETER CatalogPath
  Folder holding the catalogue (one YAML per package). Defaults to a `catalog`
  folder beside this script, then beside the Talos executable.

.PARAMETER OutputPath
  Where to write the report. Defaults to a timestamped file in the current
  directory.

.PARAMETER Deep
  Also time the per-package probes. Slower (roughly one second per package on
  Windows) but it is the measurement that shows whether a bulk scan would help.

.EXAMPLE
  pwsh -File tools/talos-doctor.ps1

.EXAMPLE
  pwsh -File tools/talos-doctor.ps1 -Deep -CatalogPath C:\Talos\catalog

.NOTES
  PowerShell 5.1+ or 7+. Windows, macOS and Linux.
  Attach the resulting file to the issue. Read it first if you would rather not
  share something: it contains package names, versions, and your host name.
#>
[CmdletBinding()]
param(
  [string] $CatalogPath,
  [string] $OutputPath,
  [switch] $Deep
)

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

# ---------------------------------------------------------------- report sink
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if (-not $OutputPath) { $OutputPath = Join-Path (Get-Location) "talos-doctor-$stamp.txt" }
$script:Lines = [System.Collections.Generic.List[string]]::new()

function Emit([string] $Text = '') {
  $script:Lines.Add($Text)
  Write-Host $Text
}
function Section([string] $Title) {
  Emit ''
  Emit "=== $Title ==="
}
function Field([string] $Name, $Value) {
  Emit ("  {0,-22} {1}" -f $Name, $Value)
}

# Run a command without letting a missing binary abort the report. Returns the
# combined output as one string, or $null when the binary is absent.
function Try-Run([string] $Exe, [string[]] $CmdArgs) {
  if (-not (Get-Command $Exe -ErrorAction SilentlyContinue)) { return $null }
  try { return (& $Exe @CmdArgs 2>&1 | Out-String) } catch { return "ERROR: $_" }
}
function Time-It([scriptblock] $Block) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $r = & $Block
  $sw.Stop()
  return @{ Output = $r; Seconds = $sw.Elapsed.TotalSeconds }
}

# ------------------------------------------------------------------ platform
$isWindowsHost = $true
if ($null -ne (Get-Variable -Name IsWindows -ErrorAction SilentlyContinue)) { $isWindowsHost = $IsWindows }
$homeDir = if ($env:USERPROFILE) { $env:USERPROFILE } else { $HOME }

Section 'talos-doctor'
Field 'generated'  (Get-Date -Format 'o')
Field 'report'     $OutputPath
Field 'deep mode'  $(if ($Deep) { 'yes (per-package timings)' } else { 'no (pass -Deep to add them)' })

Section '1. Machine'
Field 'host'        $(if ($env:COMPUTERNAME) { $env:COMPUTERNAME } else { (hostname) })
Field 'os'          $([System.Runtime.InteropServices.RuntimeInformation]::OSDescription.Trim())
Field 'arch'        $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)
Field 'powershell'  $PSVersionTable.PSVersion
Field 'shell'       $(if ($isWindowsHost) { 'powershell' } else { $env:SHELL })

Section '2. Package managers'
# Route -> the binary Talos would drive. Derived from the routes the catalogue
# can declare; absence is a normal answer, not an error.
$managers = [ordered]@{
  winget = @('--version')
  brew   = @('--version')
  cargo  = @('--version')
  npm    = @('--version')
  node   = @('--version')
  claude = @('--version')
  nu     = @('--version')
}
$present = @{}
foreach ($m in $managers.Keys) {
  $v = Try-Run $m $managers[$m]
  if ($null -eq $v) { Field $m 'ABSENT (not on PATH)' }
  else {
    $first = ($v -split "`r?`n" | Where-Object { $_.Trim() } | Select-Object -First 1)
    Field $m $first.Trim()
    $present[$m] = $true
  }
}

# ------------------------------------------------------------------- catalog
Section '3. Catalogue'
if (-not $CatalogPath) {
  $candidates = @(
    (Join-Path $PSScriptRoot '..\catalog'),
    (Join-Path $PSScriptRoot 'catalog'),
    (Join-Path (Get-Location) 'catalog')
  )
  $CatalogPath = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
$ids = @{ winget = @(); brew = @(); plugin = @() }
if ($CatalogPath -and (Test-Path $CatalogPath)) {
  $CatalogPath = (Resolve-Path $CatalogPath).Path
  $files = Get-ChildItem $CatalogPath -Filter '*.yaml' -ErrorAction SilentlyContinue
  Field 'path'      $CatalogPath
  Field 'yaml files' $files.Count
  # A deliberately minimal YAML read: only the top-level `key: value` lines this
  # report needs. Not a parser — Talos itself uses serde.
  foreach ($f in $files) {
    foreach ($line in (Get-Content $f.FullName -ErrorAction SilentlyContinue)) {
      if ($line -match '^\s*#') { continue }
      if ($line -match '^winget:\s*(\S+)')        { $ids.winget += $Matches[1].Trim('"',"'") }
      elseif ($line -match '^brew:\s*(\S+)')      { $ids.brew   += $Matches[1].Trim('"',"'") }
      elseif ($line -match '^claude-plugin:\s*(\S+)') { $ids.plugin += $Matches[1].Trim('"',"'") }
    }
  }
  Field 'winget ids'    $ids.winget.Count
  Field 'brew ids'      $ids.brew.Count
  Field 'claude plugins' $ids.plugin.Count
} else {
  Field 'path' 'NOT FOUND — pass -CatalogPath to include catalogue-derived checks'
  Emit  '  (a Talos release is TWO halves: the executable AND catalog/ + bundles/'
  Emit  '   read from beside it. If this is missing, that is worth reporting.)'
}

# ------------------------------------------------------- bulk vs per-package
Section '4. Presence scan: one bulk call'
# Talos probes presence PER package today. A single listing is the cheaper
# shape; this section records both the cost and whether the bulk output
# actually contains the catalogue's ids.
$bulkText = @{}
if ($present.winget) {
  $r = Time-It { Try-Run winget @('list', '--accept-source-agreements') }
  $bulkText.winget = $r.Output
  Field 'winget list'  ("{0:N1}s, {1} lines" -f $r.Seconds, ($r.Output -split "`r?`n").Count)
}
if ($present.brew) {
  $r1 = Time-It { Try-Run brew @('list', '--versions') }
  $r2 = Time-It { Try-Run brew @('list', '--cask', '--versions') }
  $bulkText.brew = "$($r1.Output)`n$($r2.Output)"
  Field 'brew list'      ("{0:N1}s, {1} formulae" -f $r1.Seconds, ($r1.Output -split "`r?`n" | Where-Object { $_.Trim() }).Count)
  Field 'brew list cask' ("{0:N1}s, {1} casks"    -f $r2.Seconds, ($r2.Output -split "`r?`n" | Where-Object { $_.Trim() }).Count)
}
if (-not $bulkText.Count) { Emit '  no native package manager on PATH — nothing to list' }

Section '5. Are the catalogue ids visible in the bulk output?'
Emit '  A package MISSING from the bulk listing but present on the machine is the'
Emit '  reason a bulk scan cannot simply replace per-package probing. Report any MISS.'
$missing = @()
foreach ($route in @('winget', 'brew')) {
  if (-not $bulkText[$route]) { continue }
  foreach ($id in $ids[$route]) {
    if ($bulkText[$route] -match [regex]::Escape($id)) { Emit "  found   [$route] $id" }
    else { Emit "  absent  [$route] $id"; $missing += "$route/$id" }
  }
}
if (-not $ids.winget.Count -and -not $ids.brew.Count) { Emit '  (no catalogue ids to check)' }
Field 'not in bulk output' $missing.Count
if ($missing.Count) {
  Emit '  NOTE: "absent" can be legitimate — the package may simply not be installed,'
  Emit '  or may be provided outside the manager (e.g. git shipped with Xcode CLT).'
  Emit '  Use -Deep to tell the two apart.'
}

if ($Deep) {
  Section '6. Per-package probe (the current shape), timed'
  Emit '  Compare this total against section 4: that gap is the cost of probing one by one.'
  foreach ($route in @('winget', 'brew')) {
    if (-not $present[$route] -or -not $ids[$route].Count) { continue }
    $found = 0; $disagree = @()
    $t = Time-It {
      foreach ($id in $ids[$route]) {
        if ($route -eq 'winget') {
          $o = Try-Run winget @('list', '--id', $id, '--exact', '--source', 'winget', '--accept-source-agreements')
          $hit = $o -and ($o -notmatch 'No installed package')
        } else {
          $o = Try-Run brew @('list', '--versions', $id)
          if (-not $o -or -not $o.Trim()) { $o = Try-Run brew @('list', '--cask', '--versions', $id) }
          $hit = $o -and $o.Trim()
        }
        if ($hit) {
          $script:found++
          if ($bulkText[$route] -and ($bulkText[$route] -notmatch [regex]::Escape($id))) { $script:disagree += $id }
        }
      }
    }
    Field "$route probes" ("{0} ids in {1:N1}s, {2} present" -f $ids[$route].Count, $t.Seconds, $found)
    if ($disagree.Count) {
      Emit "  DISAGREEMENT — present per-package but NOT in the bulk listing:"
      $disagree | ForEach-Object { Emit "    $_" }
      Emit '  ^ this is the important finding; please include it in the report.'
    } else {
      Emit '  no disagreement: every package found individually also appears in the bulk listing'
    }
  }
}

Section '7. Upgrade scan'
if ($present.winget) {
  $r = Time-It { Try-Run winget @('upgrade', '--accept-source-agreements', '--source', 'winget') }
  Field 'winget upgrade' ("{0:N1}s" -f $r.Seconds)
  ($r.Output -split "`r?`n" | Select-Object -First 4) | ForEach-Object { if ($_.Trim()) { Emit "    |$_" } }
}
if ($present.brew) {
  $r = Time-It { Try-Run brew @('outdated', '--greedy-auto-updates', '--json=v2') }
  Field 'brew outdated' ("{0:N1}s" -f $r.Seconds)
}

Section '8. Claude plugins'
# Talos reads these files rather than shelling out, so the report shows exactly
# what it would see.
$installed = Join-Path $homeDir '.claude/plugins/installed_plugins.json'
$settings  = Join-Path $homeDir '.claude/settings.json'
$marketRoot = Join-Path $homeDir '.claude/plugins/marketplaces'

Field 'installed_plugins' $(if (Test-Path $installed) { 'present' } else { "ABSENT ($installed)" })
if (Test-Path $installed) {
  try {
    $d = Get-Content $installed -Raw | ConvertFrom-Json
    foreach ($p in $d.plugins.PSObject.Properties) {
      $e = $p.Value[0]
      Emit ("    {0,-44} version {1,-16} updated {2}" -f $p.Name, $e.version, ($e.lastUpdated -replace 'T.*',''))
    }
  } catch { Emit "    unreadable: $_" }
}

Field 'marketplaces dir' $(if (Test-Path $marketRoot) { 'present' } else { "ABSENT ($marketRoot)" })
if (Test-Path $marketRoot) {
  Emit '  A marketplace is a local clone. The plugin manifest sits at the path the'
  Emit '  marketplace manifest declares, which is NOT always the clone root.'
  foreach ($mp in (Get-ChildItem $marketRoot -Directory -ErrorAction SilentlyContinue)) {
    $mkt = Join-Path $mp.FullName '.claude-plugin/marketplace.json'
    if (-not (Test-Path $mkt)) { Emit ("    {0,-30} no marketplace.json" -f $mp.Name); continue }
    try {
      $m = Get-Content $mkt -Raw | ConvertFrom-Json
      foreach ($pl in $m.plugins) {
        # `source` is usually a relative path, but it can also be an OBJECT
        # (e.g. @{source=url; url=...} for a plugin hosted elsewhere). Only a
        # relative path can be resolved to a local manifest.
        $src = './'
        $srcLabel = './'
        $remote = $false
        if ($pl.source -is [string]) {
          $src = $pl.source; $srcLabel = $pl.source
        } elseif ($pl.source) {
          $remote = $true
          $srcLabel = "$($pl.source.source):$($pl.source.url)$($pl.source.repo)"
        }
        if ($remote) {
          $ver = 'not local (nested source)'
        } else {
          $pj = Join-Path $mp.FullName (Join-Path $src '.claude-plugin/plugin.json')
          $ver = 'no plugin.json'
          if ((Test-Path $pj) -and -not (Test-Path $pj -PathType Container)) {
            try { $ver = (Get-Content $pj -Raw | ConvertFrom-Json).version } catch { $ver = 'unreadable' }
          }
        }
        Emit ("    {0,-24} {1,-18} source {2,-30} version {3}" -f $mp.Name, $pl.name, $srcLabel, $ver)
      }
    } catch { Emit ("    {0,-30} unreadable: {1}" -f $mp.Name, $_) }
  }
}

if (Test-Path $settings) {
  Emit '  configured marketplace sources (a plugin is the pair name+SOURCE):'
  try {
    $s = Get-Content $settings -Raw | ConvertFrom-Json
    foreach ($k in $s.extraKnownMarketplaces.PSObject.Properties) {
      $src = $k.Value.source
      $where = if ($src.path) { $src.path } elseif ($src.repo) { $src.repo } else { '?' }
      Emit ("    {0,-28} {1,-10} {2}" -f $k.Name, $src.source, $where)
    }
  } catch { Emit "    unreadable: $_" }
}

Section '9. Network reachability (read-only)'
Emit '  Talos does not fetch these itself, but a corporate firewall answering 403'
Emit '  is a known failure mode worth capturing.'
foreach ($u in @('https://github.com', 'https://raw.githubusercontent.com', 'https://formulae.brew.sh', 'https://registry.npmjs.org')) {
  try {
    $t = Time-It { Invoke-WebRequest -Uri $u -Method Head -TimeoutSec 10 -UseBasicParsing -ErrorAction Stop }
    Field $u ("HTTP {0} in {1:N1}s" -f $t.Output.StatusCode, $t.Seconds)
  } catch {
    $code = $null
    if ($_.Exception.Response) { $code = [int]$_.Exception.Response.StatusCode }
    Field $u $(if ($code) { "HTTP $code" } else { "unreachable: $($_.Exception.Message -replace "`r?`n",' ')" })
  }
}

Section 'Summary'
Field 'managers found'      (($present.Keys | Sort-Object) -join ', ')
Field 'catalogue ids'       ("winget {0}, brew {1}, plugins {2}" -f $ids.winget.Count, $ids.brew.Count, $ids.plugin.Count)
Field 'ids not in bulk'     $missing.Count
Emit ''
Emit 'What to do with this file:'
Emit '  1. Read it — it lists package names, versions and your host name.'
Emit '  2. Attach it to the issue, with what you EXPECTED versus what Talos SHOWED.'
Emit '  3. If a row was wrong, say which package and which state it displayed.'

try {
  $script:Lines | Set-Content -Path $OutputPath -Encoding UTF8
  Write-Host ''
  Write-Host "Report written to: $OutputPath"
} catch {
  Write-Host ''
  Write-Host "Could not write the report: $_"
  Write-Host 'Copy the output above instead.'
}
