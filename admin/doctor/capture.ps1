<#
.SYNOPSIS
  Capture what THIS machine prints, so a wrong row can be reproduced.

.DESCRIPTION
  The companion of `admin doctor`, for the case that gesture cannot cover: a
  Windows user reporting a problem. `doctor` is written in nushell, and nushell
  is NOT part of the Base bundle — an end user may simply not have it. Windows
  PowerShell is always there.

  So this script stays deliberately small: it does not diagnose, it CAPTURES.
  It records the raw, verbatim output of the commands Talos runs, and the JSON
  files Talos reads. Those captures are what a parser test can then be written
  against — the reason `tests/fixtures/managers/winget-list-real.txt` exists at
  all.

  READ-ONLY: it lists, queries and reads. It never installs, upgrades,
  uninstalls, or writes anything except the capture folder. Every command is
  non-interactive.

.PARAMETER OutputDir
  Where to write the capture. Defaults to a timestamped folder on the Desktop,
  so a non-technical reporter can find it.

.PARAMETER Package
  Optional. The winget or brew id of the package whose row was wrong. Adds the
  per-package probes for it, which is usually the decisive evidence.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File admin\doctor\capture.ps1

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File admin\doctor\capture.ps1 -Package Git.Git

.NOTES
  Then zip the folder and attach it, saying what you EXPECTED and what Talos
  SHOWED. Read the files first if you would rather not share something: they
  contain installed package names, versions and your host name.
#>
[CmdletBinding()]
param(
  [string] $OutputDir,
  [string] $Package
)

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

# Windows-only env vars are NULL elsewhere, and Join-Path throws on a null Path.
# The script must still run on macOS/Linux — that is how it gets tested before a
# user is asked to run it.
$homeDir = if ($env:USERPROFILE) { $env:USERPROFILE } elseif ($HOME) { $HOME } else { (Get-Location).Path }
$localAppData = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { Join-Path $homeDir '.local/share' }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if (-not $OutputDir) {
  $desktop = [Environment]::GetFolderPath('Desktop')
  if (-not $desktop) { $desktop = (Get-Location).Path }
  $OutputDir = Join-Path $desktop "talos-capture-$stamp"
}
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null

$index = [System.Collections.Generic.List[string]]::new()
function Note([string] $Text) { $index.Add($Text); Write-Host $Text }

# Run a command and keep its output VERBATIM — no trimming, no parsing. The
# bytes are the evidence; interpreting them here would throw away the finding.
function Capture([string] $Name, [string] $Exe, [string[]] $CmdArgs) {
  if (-not (Get-Command $Exe -ErrorAction SilentlyContinue)) {
    Note ("  {0,-34} SKIPPED ({1} not on PATH)" -f $Name, $Exe)
    return
  }
  $file = Join-Path $OutputDir "$Name.txt"
  $sw = [Diagnostics.Stopwatch]::StartNew()
  try { $out = (& $Exe @CmdArgs 2>&1 | Out-String) } catch { $out = "ERROR: $_" }
  $sw.Stop()
  # -Encoding UTF8 so box-drawing and accented package names survive the trip.
  "# $Exe $($CmdArgs -join ' ')`n# captured $(Get-Date -Format o) in $([math]::Round($sw.Elapsed.TotalSeconds,1))s`n`n$out" |
    Set-Content -Path $file -Encoding UTF8
  Note ("  {0,-34} {1,7:N1}s  {2} lines" -f $Name, $sw.Elapsed.TotalSeconds, ($out -split "`r?`n").Count)
}

function CopyIfPresent([string] $Name, [string] $Path) {
  if (Test-Path $Path -PathType Leaf) {
    Copy-Item $Path (Join-Path $OutputDir $Name) -Force
    Note ("  {0,-34} copied" -f $Name)
  } else {
    Note ("  {0,-34} ABSENT ({1})" -f $Name, $Path)
  }
}

Note "talos capture — $stamp"
Note "  folder: $OutputDir"

Note ''
Note 'machine'
@(
  "host        : $env:COMPUTERNAME"
  "os          : $([System.Runtime.InteropServices.RuntimeInformation]::OSDescription.Trim())"
  "arch        : $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
  "powershell  : $($PSVersionTable.PSVersion)"
  "winget      : $(try { (winget --version) } catch { 'absent' })"
  "claude      : $(try { (claude --version) } catch { 'absent' })"
  "captured    : $(Get-Date -Format o)"
) | Set-Content (Join-Path $OutputDir 'machine.txt') -Encoding UTF8
Get-Content (Join-Path $OutputDir 'machine.txt') | ForEach-Object { Note "  $_" }

Note ''
Note 'package manager output (verbatim — this is what the parser must survive)'
Capture 'winget-list'    winget @('list', '--accept-source-agreements')
Capture 'winget-upgrade' winget @('upgrade', '--accept-source-agreements', '--source', 'winget')
Capture 'brew-list'      brew   @('list', '--versions')
Capture 'brew-list-cask' brew   @('list', '--cask', '--versions')
Capture 'brew-outdated'  brew   @('outdated', '--greedy-auto-updates', '--json=v2')

if ($Package) {
  Note ''
  Note "per-package probes for '$Package' (the row that was wrong)"
  Capture 'probe-winget-list'    winget @('list', '--id', $Package, '--exact', '--source', 'winget', '--accept-source-agreements')
  Capture 'probe-winget-show'    winget @('show', '--id', $Package, '--exact', '--source', 'winget', '--accept-source-agreements')
  Capture 'probe-brew-versions'  brew   @('list', '--versions', $Package)
  Capture 'probe-brew-cask'      brew   @('list', '--cask', '--versions', $Package)
}

Note ''
Note 'files Talos reads (copied as-is)'
$claude = Join-Path $homeDir '.claude'
CopyIfPresent 'installed_plugins.json' (Join-Path $claude 'plugins' 'installed_plugins.json')
CopyIfPresent 'claude-settings.json'   (Join-Path $claude 'settings.json')

# Each marketplace's own manifest: the version a plugin SHOULD be at. The
# manifest is not always at the clone root, so the whole file is copied and the
# resolution is left to whoever reads the capture.
$mkt = Join-Path $claude 'plugins' 'marketplaces'
if (Test-Path $mkt) {
  $dest = Join-Path $OutputDir 'marketplaces'
  New-Item -ItemType Directory -Path $dest -Force | Out-Null
  $n = 0
  foreach ($d in (Get-ChildItem $mkt -Directory -ErrorAction SilentlyContinue)) {
    $m = Join-Path $d.FullName '.claude-plugin' 'marketplace.json'
    if (Test-Path $m) {
      Copy-Item $m (Join-Path $dest "$($d.Name).marketplace.json") -Force
      $n++
    }
  }
  Note ("  {0,-34} {1} manifests" -f 'marketplaces/', $n)
} else {
  Note ("  {0,-34} ABSENT ({1})" -f 'marketplaces/', $mkt)
}

# Talos writes its own logs next to the executable and per user; if the reporter
# ran a real Apply, this is where the failure is.
Note ''
Note 'talos logs, if any'
$talosLog = Join-Path $localAppData 'Talos'
if (Test-Path $talosLog) {
  $dest = Join-Path $OutputDir 'talos-local'
  New-Item -ItemType Directory -Path $dest -Force | Out-Null
  Copy-Item (Join-Path $talosLog '*') $dest -Recurse -Force -ErrorAction SilentlyContinue
  Note ("  {0,-34} copied from {1}" -f 'talos-local/', $talosLog)
} else {
  Note ("  {0,-34} none at {1}" -f 'talos-local/', $talosLog)
}

$index | Set-Content (Join-Path $OutputDir 'INDEX.txt') -Encoding UTF8

@"
What this is
------------
A verbatim capture of what this machine answers, taken $(Get-Date -Format o).
Nothing was installed, changed or removed.

What to do
----------
1. Look through the files if you would rather not share something. They contain
   installed package names, versions and this machine's name.
2. Zip this folder.
3. Attach it to the report, and say:
     - which package's row was wrong
     - what you EXPECTED to see
     - what Talos SHOWED instead
"@ | Set-Content (Join-Path $OutputDir 'READ-ME-FIRST.txt') -Encoding UTF8

Write-Host ''
Write-Host "Capture written to: $OutputDir"
Write-Host 'Zip that folder and attach it, with what you expected versus what Talos showed.'
