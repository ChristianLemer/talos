// watch-window.ts — while a step runs, watch for a foreign window appearing in
// our process tree (an installer wizard / consent dialog popping BEHIND the Edge
// --app panel) and signal it. "Talos honest about what's happening": the panel
// looks frozen while it's really just waiting for a window the user can't see.
//
// Two natures, like detect.ts splits pure build from IO:
//   - SCAN_SCRIPT: a pure PowerShell string. Given a root PID, it enumerates the
//     VISIBLE top-level windows of the process tree, excludes Edge/Talos, and if
//     found: FlashWindowEx + tries SetForegroundWindow. Prints JSON
//     { found, title?, pushed }. Only meaningful on Windows; tested standalone on
//     the VM (real P/Invoke).
//   - makeWatcher: the per-tick DECISION (dedup, silence→UAC, anti-overlap, stop).
//     spawner + emit + clock are INJECTED, so this logic is unit-tested on Mac
//     with a fake spawner — no real timer, no real windows.
//
// The scheduler (setInterval) lives at the call site (server.ts): it's trivial
// plumbing around runTick(), nothing to test.

// --- the scan result -------------------------------------------------------

export interface Scan {
  found: boolean;
  title?: string;
  pushed?: boolean;
}

// Defensive parse of the PS script's stdout. Anything malformed → not found (the
// safe direction: never claim a window is there when we can't be sure).
export function parseScan(stdout: string): Scan {
  try {
    const o = JSON.parse(stdout);
    if (o && o.found === true) {
      return {
        found: true,
        title: typeof o.title === "string" ? o.title : undefined,
        pushed: o.pushed === true,
      };
    }
  } catch { /* not JSON → fall through */ }
  return { found: false };
}

// --- the PowerShell scan + push script (pure string) -----------------------
// Roots at Talos's OWN pid ({ROOT_PID} = Deno.pid): the powershell→winget→wizard
// chain are all its descendants (the pty-ffi lib doesn't expose the child pid, so
// we walk from Talos down, not from the pty up). That subtree ALSO contains the
// Edge --app panel we spawned — so we exclude windows owned by a browser process
// (msedge/chrome/talos) BY NAME, plus the scan powershell itself. On the first
// remaining foreign window: FlashWindowEx + try SetForegroundWindow. Emits one
// line of JSON { found, title?, pushed }.
export const SCAN_SCRIPT = String.raw`
$ErrorActionPreference='SilentlyContinue'
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct FLASHWINFO { public uint cbSize; public IntPtr hwnd; public uint dwFlags; public uint uCount; public uint dwTimeout; }
  [DllImport("user32.dll")] public static extern bool FlashWindowEx(ref FLASHWINFO pwfi);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  // Bypass the foreground-lock: SetForegroundWindow is refused when the caller
  // doesn't own the foreground. Attaching our input thread to the current
  // foreground window's thread lifts the lock for the call, then we detach.
  // SW_RESTORE(9) un-minimizes if the wizard came up iconic.
  public static bool ForceForeground(IntPtr h){
    if(IsIconic(h)) ShowWindow(h, 9);
    IntPtr fg = GetForegroundWindow();
    uint p2 = 0;
    uint tFg = GetWindowThreadProcessId(fg, out p2); // out-var declared first: Add-Type on PS 5.1 is C# 5, no inline out
    uint tMe = GetCurrentThreadId();
    bool attached = (tFg != tMe) && AttachThreadInput(tMe, tFg, true);
    BringWindowToTop(h);
    bool ok = SetForegroundWindow(h);
    if(attached) AttachThreadInput(tMe, tFg, false);
    return ok;
  }
}
"@
# process tree rooted at Talos, plus a pid->name map for the browser exclusion
$root=[uint32]{ROOT_PID}
$all=Get-CimInstance Win32_Process
$name=@{}; foreach($p in $all){ $name[[uint32]$p.ProcessId]=$p.Name }
$tree=New-Object System.Collections.Generic.HashSet[uint32]
$tree.Add($root) | Out-Null
$changed=$true
while($changed){ $changed=$false; foreach($p in $all){ if($tree.Contains([uint32]$p.ParentProcessId) -and -not $tree.Contains([uint32]$p.ProcessId)){ $tree.Add([uint32]$p.ProcessId)|Out-Null; $changed=$true } } }
$self=$PID
$skip='msedge.exe','chrome.exe','Talos.exe' # our own UI + the engine, never "foreign"
$result=@{found=$false}
$cb={ param($h,$p)
  if(-not [W]::IsWindowVisible($h)){ return $true }
  $pid2=0; [W]::GetWindowThreadProcessId($h,[ref]$pid2)|Out-Null
  if($pid2 -eq $self){ return $true }
  if(-not $tree.Contains([uint32]$pid2)){ return $true }
  if($skip -contains $name[[uint32]$pid2]){ return $true }
  $len=[W]::GetWindowTextLength($h); if($len -le 0){ return $true }
  $sb=New-Object System.Text.StringBuilder ($len+1); [W]::GetWindowText($h,$sb,$sb.Capacity)|Out-Null
  $title=$sb.ToString()
  $fi=New-Object W+FLASHWINFO; $fi.cbSize=[uint32][System.Runtime.InteropServices.Marshal]::SizeOf($fi); $fi.hwnd=$h; $fi.dwFlags=3; $fi.uCount=5; $fi.dwTimeout=0
  [W]::FlashWindowEx([ref]$fi)|Out-Null
  $pushed=[W]::ForceForeground($h)
  $script:result=@{found=$true;title=$title;pushed=[bool]$pushed}
  return $false
}
[W]::EnumWindows($cb,[IntPtr]::Zero)|Out-Null
$result | ConvertTo-Json -Compress
`;

// --- the real spawner (IO) -------------------------------------------------
// Runs SCAN_SCRIPT for a root pid via powershell, returns its stdout (JSON). The
// pure watcher takes this as an injected Spawner; tests pass a fake instead.
export function powershellSpawner(pid: number): Promise<string> {
  const script = SCAN_SCRIPT.replace("{ROOT_PID}", String(pid));
  return new Deno.Command("powershell.exe", {
    args: ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script],
    stdout: "piped",
    stderr: "null",
    stdin: "null",
  })
    .output()
    .then((o) => new TextDecoder().decode(o.stdout))
    .catch(() => "{}"); // spawn failure → not found (safe direction)
}

// --- the watcher (pure decision, IO injected) ------------------------------

type Spawner = (pid: number) => Promise<string>;
type Emit = (msg: unknown) => void;

export interface WatcherOpts {
  i: number; // step index — tags every message so the UI knows the row
  isWin: boolean;
  silenceMs: number; // pty quiet longer than this + no window → likely UAC
  spawner: Spawner; // runs SCAN_SCRIPT for a pid, returns raw stdout
  emit: Emit; // sends a WS message to the client
  now: () => number; // injectable clock (tests), Date.now in prod
}

export interface Watcher {
  runTick(pid: number, lastActivity: number): Promise<void>;
  stop(): void;
}

// Build a watcher. Off Windows it's inert. On Windows, each runTick spawns the
// scan for `pid`, then decides what (if anything) to emit — deduping a window
// already signalled, and falling back to wait-silent when nothing is found yet
// the pty has gone quiet past the threshold (the UAC case: its window lives on
// the secure desktop, so it's never enumerated → silence IS the signal).
export function makeWatcher(o: WatcherOpts): Watcher {
  let stopped = false;
  let inFlight = false;
  let lastTitle: string | null = null; // last foreign window we announced
  let silentAnnounced = false; // wait-silent emitted for the current quiet spell

  return {
    async runTick(pid, lastActivity) {
      if (!o.isWin || stopped || inFlight) return;
      inFlight = true;
      try {
        const scan = parseScan(await o.spawner(pid));
        if (stopped) return; // finished mid-tick → drop this result
        if (scan.found) {
          silentAnnounced = false; // a real window supersedes the silence guess
          const title = scan.title ?? "";
          if (title !== lastTitle) {
            lastTitle = title;
            // pushed = did the raise-to-front actually succeed? The taskbar
            // flash always fires, but SetForegroundWindow can be refused by the
            // foreground-lock — carry the truth so the banner doesn't claim a
            // raise that didn't happen.
            o.emit({
              type: "wait-window",
              i: o.i,
              title,
              pushed: scan.pushed === true,
            });
          }
          return;
        }
        // nothing found: is the pty silent long enough to suspect a hidden wait?
        const quietFor = o.now() - lastActivity;
        if (quietFor > o.silenceMs) {
          if (!silentAnnounced) {
            silentAnnounced = true;
            o.emit({ type: "wait-silent", i: o.i });
          }
        }
      } finally {
        inFlight = false;
      }
    },
    stop() {
      if (stopped) return;
      stopped = true;
      o.emit({ type: "wait-clear", i: o.i });
    },
  };
}
