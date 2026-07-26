// Spots a foreign window (installer/UAC) that
// pops up BEHIND the panel during an install. The Win32 P/Invoke lives ENTIRELY in
// SCAN_SCRIPT (PowerShell); Rust only spawns + parses the output JSON.
use serde::Deserialize;

// Used only by the #[cfg(windows)] watcher in server.rs — seen as "dead"
// from a Mac/Linux build, which is normal (the watcher is Windows-only).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Deserialize, Default, PartialEq)]
pub struct Scan {
    pub found: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub pushed: bool,
}

/// Defensive parse of the PS script's stdout: anything malformed → not found
/// (safe direction: never claim a window is there without certainty).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn parse_scan(stdout: &str) -> Scan {
    match serde_json::from_str::<Scan>(stdout.trim()) {
        Ok(s) if s.found => s,
        _ => Scan::default(),
    }
}

/// The PowerShell script. Enumerates the visible windows of the process tree rooted
/// at {ROOT_PID},
/// excludes msedge/chrome/msedgewebview2/Talos + itself, and on the 1st foreign
/// window: FlashWindowEx + ForceForeground. Emits a JSON line {found,title?,pushed}.
pub const SCAN_SCRIPT: &str = r#"
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
  public static bool ForceForeground(IntPtr h){
    if(IsIconic(h)) ShowWindow(h, 9);
    IntPtr fg = GetForegroundWindow();
    uint p2 = 0;
    uint tFg = GetWindowThreadProcessId(fg, out p2);
    uint tMe = GetCurrentThreadId();
    bool attached = (tFg != tMe) && AttachThreadInput(tMe, tFg, true);
    BringWindowToTop(h);
    bool ok = SetForegroundWindow(h);
    if(attached) AttachThreadInput(tMe, tFg, false);
    return ok;
  }
}
"@
$root=[uint32]{ROOT_PID}
$all=Get-CimInstance Win32_Process
$name=@{}; foreach($p in $all){ $name[[uint32]$p.ProcessId]=$p.Name }
$tree=New-Object System.Collections.Generic.HashSet[uint32]
$tree.Add($root) | Out-Null
$changed=$true
while($changed){ $changed=$false; foreach($p in $all){ if($tree.Contains([uint32]$p.ParentProcessId) -and -not $tree.Contains([uint32]$p.ProcessId)){ $tree.Add([uint32]$p.ProcessId)|Out-Null; $changed=$true } } }
$self=$PID
# msedgewebview2.exe = Tauri's OWN webview, a child of talos.exe so IN the tree
# -> must be excluded, else the watcher detects OUR OWN window.
$skip='msedge.exe','chrome.exe','msedgewebview2.exe','Talos.exe'
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
"#;

/// Runs SCAN_SCRIPT for a root pid via powershell, returns its stdout (JSON).
/// Spawn failure → "{}" (safe direction: not found). Mirror of powershellSpawner.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn powershell_spawner(pid: u32) -> String {
    let script = SCAN_SCRIPT.replace("{ROOT_PID}", &pid.to_string());
    match crate::platform::quiet_command("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => "{}".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_found() {
        let s = parse_scan(r#"{"found":true,"title":"Setup","pushed":true}"#);
        assert_eq!(
            s,
            Scan {
                found: true,
                title: Some("Setup".into()),
                pushed: true
            }
        );
    }

    #[test]
    fn parse_not_found() {
        assert_eq!(parse_scan(r#"{"found":false}"#), Scan::default());
    }

    #[test]
    fn parse_garbage_is_not_found() {
        assert_eq!(parse_scan("not json"), Scan::default());
        assert_eq!(parse_scan(""), Scan::default());
    }
}
