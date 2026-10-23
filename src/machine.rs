//! machine.rs — what this machine IS, and how fast it answers, written to the journal.
//!
//! Two records, both appended to the install journal beside the install lines
//! (`consent::append_record`), so they follow the SAME rule: always the local file, the
//! shared `logs/<host>/<user>.jsonl` only with the share consent. No new channel.
//!
//! - `machine` — one per launch: OS, CPU, RAM, free disk, where the kit sits, the cost of
//!   ONE empty shell, and where each package manager the plan uses actually resolves.
//! - `scan` — one per scan: the batched calls and every package probe, in milliseconds.
//!   These were measured long before this file existed, and printed with `println!` — which
//!   a Windows release (`windows_subsystem = "windows"`) sends nowhere. On the fleet, the
//!   answer to "where does the time go" was computed and thrown away on every launch.
//!
//! ⚠️ DETECT, DON'T REMEMBER. Every value is observed at this launch; nothing here is ever
//! read back as a truth about the machine. The journal is the trace, not a cache.
//!
//! ⚠️ PRIVACY. The shared log may be read by a team and the repo is public: no serial
//! number, no MAC address, no user name beyond the one the shared log's path already
//! carries — and the home folder is written `~` in every path, so not even that one leaks
//! through a tool's location.
//!
//! ⚠️ ON winget "never found". Its path does NOT vary between machines: it is a per-user
//! App Execution Alias, `%LOCALAPPDATA%\Microsoft\WindowsApps\winget.exe`, that App
//! Installer registers asynchronously at the user's first logon. What varies is whether that
//! registration happened (a fresh profile, an image that removed the Store apps, LTSC or
//! Server) and whether `WindowsApps` is on the PATH the probes see. `alias` and `on_path`
//! below tell those two apart; the resolved path alone cannot.
//!
//! Same discipline as detect.rs: the script and its parsing are PURE (tested); the IO is a
//! thin shell that never panics — a profile that cannot be collected must not sink a launch.

use crate::platform::Os;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One `machine` journal line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub kind: String,
    pub at: String,
    /// The build stamp, so a slow line can be tied to the binary that wrote it.
    pub talos: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub cpu: String,
    /// Logical cores — what the OS schedules on.
    pub cores: usize,
    pub ram_total_mb: u64,
    pub ram_available_mb: u64,
    /// Free space on the system disk — the one disk fact kept: installers and package
    /// caches land there, wherever the kit sits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_disk: Option<DiskFree>,
    pub kit: Kit,
    /// One EMPTY command through `platform::shell_probe`, the wrapper every probe pays for.
    /// The cleanest number for "is something taxing process start here" — a corporate
    /// antivirus or EDR scanning each spawn shows up here first.
    pub shell_ms: u64,
    pub tools: Vec<Tool>,
    /// What the one tools probe took — CIM queries included on Windows, so it is NOT a
    /// second spawn baseline; `shell_ms` is.
    pub tools_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winget: Option<WingetAlias>,
    /// Antivirus products the OS declares (Windows Security Center, client editions only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub antivirus: Vec<String>,
    /// Defender real-time protection, when Defender answers at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime: Option<bool>,
    /// `ac` | `battery` — Windows throttles a machine on battery saver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<String>,
    /// Every value set under the policy keys that govern winget and the Store, as stored.
    /// None: not read (not Windows, or the probe did not get that far); empty: none set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policies: Option<Vec<Policy>>,
    /// The registry type of the user PATH: `ExpandString` (REG_EXPAND_SZ) or `String`
    /// (REG_SZ — then `%USERPROFILE%` in it is never expanded, and WindowsApps is lost).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_path_kind: Option<String>,
    /// What collecting all of this took, end to end.
    pub collect_ms: u64,
    /// Set when the collection failed or did not finish — then every other field is empty,
    /// and this line says so instead of the Machine tab waiting forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Profile {
    /// The profile of a collection that failed: the stamp, the time and the reason.
    pub fn failed(reason: &str) -> Self {
        Profile {
            kind: "machine".into(),
            at: chrono::Utc::now().to_rfc3339(),
            talos: format!("{} {}", crate::build_info::TAG, crate::build_info::SHA),
            error: Some(reason.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiskFree {
    pub mount: String,
    pub free_mb: u64,
    pub total_mb: u64,
}

/// Where the kit (the folder beside the exe) sits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Kit {
    pub path: String,
    /// `local` | `network` | `onedrive` | `synced` — a synced or network kit is read
    /// through a slower path than a local disk.
    pub location: String,
}

/// A package manager or tool the plan relies on, and where the probes' shell finds it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    /// None: the probes' shell does not find it.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WingetAlias {
    /// The per-user alias file exists (App Installer registered it for this user).
    pub alias: bool,
    /// Its folder is on the PATH the probes see.
    pub on_path: bool,
}

/// One policy value, named `<key leaf>.<value name>` — `AppInstaller.EnableDefaultSource`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub name: String,
    pub value: String,
}

/// One `scan` journal line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanRecord {
    pub kind: String,
    pub at: String,
    pub total_ms: u64,
    pub packages: usize,
    pub outdated_ms: u64,
    pub presence_ms: u64,
    pub vscode_ms: u64,
    /// Why a batched call gave no answer, in its own words — the line that says winget was
    /// not found used to be printed to a console the Windows release does not have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<String>,
    pub probes: Vec<ProbeTime>,
    /// The machine profile was being collected during this scan, so its two shells ran
    /// beside the probes and may have slowed them: read these timings with that in mind.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub profile_running: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProbeTime {
    pub name: String,
    pub ms: u64,
}

/// The binary behind each route. `run` has none: its command is free text.
fn route_binary(route: &str) -> Option<&'static str> {
    Some(match route {
        "winget" => "winget",
        "brew" => "brew",
        "cargo" => "cargo",
        "npm" => "npm",
        "bun" => "bun",
        "claude-plugin" => "claude",
        "copilot-plugin" => "copilot",
        "skill" => "npx",
        "vscode-extension" => "code",
        "nu-plugin" => "nu",
        _ => return None,
    })
}

/// The tools worth locating: the OS's native manager and git always (a claude plugin
/// clones with it, and it is the first thing asked when a row fails), then one binary per
/// route the plan actually uses — in that order, without duplicates.
pub fn tools_for(os: Os, routes: &[&str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    let native = crate::managers::native_manager(os).and_then(|m| route_binary(m.route));
    for t in native
        .into_iter()
        .chain(std::iter::once("git"))
        .chain(routes.iter().filter_map(|r| route_binary(r)))
    {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// The ONE command that locates every tool, run through `platform::shell_probe` so it sees
/// exactly the PATH a package probe sees (the registry refresh on Windows, the login shell
/// elsewhere). One line per fact, tab-separated, read by `parse_tools`.
///
/// Windows: no double quote anywhere — the line travels as a `-Command` argument and every
/// other probe keeps to single quotes too. Each optional fact sits in its own try, so a
/// missing cmdlet (Defender absent, a server edition without Security Center) costs that
/// fact and nothing else. ⚠️ Power: `Win32_Battery.BatteryStatus` 2 is "on AC" but 3
/// (fully charged) and 6–9/11 (charging) are on AC too — only 1 (discharging) and 4/5
/// (low/critical) say battery. Reading `2` as the only AC value logged a plugged-in laptop
/// as on battery.
///
/// POSIX: one `command -v` per tool, no loop, no `case`, no `$(…)` — the probe runs in the
/// USER's login shell, and a fish login shell rejects sh syntax whole, which emptied every
/// fact at once. Plain enough for the sh family and fish. NOT for nu: MEASURED, nu has no
/// `command` — but with nu as `$SHELL` every probe of Talos fails the same way, so this
/// line is the least of that machine's problems. A found tool prints its path, then an
/// empty line the parser skips.
/// On a Mac, `pmset`'s first line ("Now drawing from 'AC Power'") is read by `parse_tools`.
pub fn tools_command(os: Os, tools: &[&str]) -> String {
    match os {
        Os::Windows => {
            let list = tools
                .iter()
                .map(|t| format!("'{t}'"))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "$tab=[char]9; \
                 foreach ($t in @({list})) {{ $c = Get-Command $t -ErrorAction SilentlyContinue | Select-Object -First 1; $p = ''; if ($c) {{ $p = $c.Source; if (-not $p) {{ $p = [string]$c.Definition }} }}; Write-Output (('tool',$t,$p) -join $tab) }}; \
                 $wa = Join-Path $env:LOCALAPPDATA 'Microsoft\\WindowsApps'; \
                 Write-Output (('alias',(Test-Path (Join-Path $wa 'winget.exe'))) -join $tab); \
                 Write-Output (('onpath',[bool](($env:Path -split ';') | Where-Object {{ $_.TrimEnd('\\') -eq $wa }})) -join $tab); \
                 try {{ Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntiVirusProduct -ErrorAction Stop | ForEach-Object {{ Write-Output (('av',$_.displayName) -join $tab) }} }} catch {{}}; \
                 try {{ Write-Output (('rt',(Get-MpComputerStatus -ErrorAction Stop).RealTimeProtectionEnabled) -join $tab) }} catch {{}}; \
                 try {{ $b = @(Get-CimInstance -ClassName Win32_Battery -ErrorAction Stop); if ($b.Count -gt 0 -and @(1,4,5) -contains $b[0].BatteryStatus) {{ Write-Output (('power','battery') -join $tab) }} else {{ Write-Output (('power','ac') -join $tab) }} }} catch {{}}; \
                 try {{ foreach ($k in @('HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\AppInstaller','HKLM:\\SOFTWARE\\Policies\\Microsoft\\WindowsStore')) {{ $i = Get-Item $k -ErrorAction SilentlyContinue; if ($i) {{ foreach ($n in $i.GetValueNames()) {{ Write-Output (('policy',((Split-Path $k -Leaf)+'.'+$n),[string]$i.GetValue($n)) -join $tab) }} }} }}; Write-Output (('policyscan','done') -join $tab) }} catch {{}}; \
                 try {{ Write-Output (('pathkind',(Get-Item 'HKCU:\\Environment' -ErrorAction Stop).GetValueKind('Path')) -join $tab) }} catch {{}}"
            )
        }
        _ => {
            let mut parts: Vec<String> = tools
                .iter()
                .map(|t| {
                    format!("printf 'tool\\t%s\\t' {t}; command -v {t} 2>/dev/null; printf '\\n'")
                })
                .collect();
            if os == Os::Darwin {
                parts.push("pmset -g batt 2>/dev/null | head -n 1".into());
            }
            parts.join("; ")
        }
    }
}

/// What the tools probe said. Tolerant: an interactive login shell may print its own
/// banner around the facts, so only lines starting with a known key are read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolsReport {
    pub tools: Vec<Tool>,
    pub winget: Option<WingetAlias>,
    pub antivirus: Vec<String>,
    pub realtime: Option<bool>,
    pub power: Option<String>,
    pub policies: Option<Vec<Policy>>,
    pub user_path_kind: Option<String>,
}

fn ps_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub fn parse_tools(output: &str) -> ToolsReport {
    let mut r = ToolsReport::default();
    let (mut alias, mut on_path) = (None, None);
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        let mut f = line.splitn(3, '\t');
        match (f.next(), f.next(), f.next()) {
            (Some("tool"), Some(name), path) if !name.is_empty() => r.tools.push(Tool {
                name: name.to_string(),
                path: path
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(String::from),
            }),
            (Some("alias"), Some(v), None) => alias = ps_bool(v),
            (Some("onpath"), Some(v), None) => on_path = ps_bool(v),
            (Some("av"), Some(v), None) if !v.trim().is_empty() => {
                r.antivirus.push(v.trim().to_string())
            }
            (Some("rt"), Some(v), None) => r.realtime = ps_bool(v),
            (Some("power"), Some(v @ ("ac" | "battery")), None) => r.power = Some(v.into()),
            (Some("policy"), Some(name), Some(value)) if !name.is_empty() => {
                r.policies.get_or_insert_with(Vec::new).push(Policy {
                    name: name.to_string(),
                    value: value.trim().to_string(),
                })
            }
            (Some("policyscan"), Some("done"), None) => {
                r.policies.get_or_insert_with(Vec::new);
            }
            (Some("pathkind"), Some(v), None) if !v.trim().is_empty() => {
                r.user_path_kind = Some(v.trim().to_string())
            }
            (Some(l), None, None) if l.starts_with("Now drawing from") => {
                if l.contains("'AC Power'") {
                    r.power = Some("ac".into());
                } else if l.contains("'Battery Power'") {
                    r.power = Some("battery".into());
                }
            }
            _ => {}
        }
    }
    if let (Some(alias), Some(on_path)) = (alias, on_path) {
        r.winget = Some(WingetAlias { alias, on_path });
    }
    r
}

/// Writes the home folder as `~`, so a shared log names no one through a path. Windows
/// paths compare without case, as the filesystem does.
pub fn tilde(path: &str, home: &str) -> String {
    if home.is_empty() || path.len() < home.len() {
        return path.to_string();
    }
    let (head, rest) = path.split_at(home.len());
    let same = if home.contains('\\') || home.as_bytes().get(1) == Some(&b':') {
        head.eq_ignore_ascii_case(home)
    } else {
        head == home
    };
    // A prefix that stops mid-name (`/home/al` vs `/home/alice`) is not the home folder.
    if same && (rest.is_empty() || rest.starts_with(['/', '\\'])) {
        format!("~{rest}")
    } else {
        path.to_string()
    }
}

/// `tilde` for free text: every occurrence of the home folder, wherever it appears in a
/// message, written `~`. For a panic message, which may quote a path anywhere in it.
pub fn tilde_all(text: &str, home: &str) -> String {
    if home.is_empty() {
        return text.to_string();
    }
    text.replace(home, "~")
}

/// Where the kit sits. A UNC path, or a drive letter that is no local disk (a mapped
/// network drive), is `network`; a OneDrive folder is `onedrive`; another sync client's
/// folder is `synced`.
pub fn kit_location(path: &str, on_local_disk: bool) -> &'static str {
    if path.starts_with("\\\\") || path.starts_with("//") || !on_local_disk {
        return "network";
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("onedrive") {
        return "onedrive";
    }
    const SYNCED: [&str; 5] = [
        "google drive",
        "dropbox",
        "icloud",
        "cloudstorage",
        "insync",
    ];
    if SYNCED.iter().any(|s| lower.contains(s)) {
        return "synced";
    }
    "local"
}

/// Is `mount` the folder `path` lives under — a prefix that ends at a folder boundary, so
/// `/mnt/data` does not claim `/mnt/data2`? Windows mounts (`C:\`) compare without case.
fn under(path: &str, mount: &str) -> bool {
    let windows = mount.contains('\\') || mount.as_bytes().get(1) == Some(&b':');
    // `get`, never an index: a non-ASCII path may not split at the mount's byte length.
    let (Some(head), Some(rest)) = (path.get(..mount.len()), path.get(mount.len()..)) else {
        return false;
    };
    let prefixed = if windows {
        head.eq_ignore_ascii_case(mount)
    } else {
        head == mount
    };
    prefixed && (mount.ends_with(['/', '\\']) || rest.is_empty() || rest.starts_with(['/', '\\']))
}

/// The mount a path lives on: the LONGEST mount point it lives under.
pub fn disk_for(path: &str, mounts: &[String]) -> Option<usize> {
    mounts
        .iter()
        .enumerate()
        .filter(|(_, m)| under(path, m))
        .max_by_key(|(_, m)| m.len())
        .map(|(i, _)| i)
}

/// Linux: is `path` on a network filesystem, per `/proc/self/mounts`? sysinfo lists only
/// local disks, so on POSIX the kit always matches `/` there — the mount table is what can
/// tell a NFS or SMB kit from a local one. Mount points escape a space as `\040`.
pub fn network_fs_linux(path: &str, proc_mounts: &str) -> bool {
    const NETWORK: [&str; 12] = [
        "nfs",
        "nfs4",
        "cifs",
        "smb3",
        "smbfs",
        "9p",
        "afs",
        "ceph",
        "glusterfs",
        "fuse.sshfs",
        "fuse.rclone",
        "davfs",
    ];
    proc_mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_, mnt, fstype) = (f.next()?, f.next()?, f.next()?);
            Some((mnt.replace("\\040", " "), fstype))
        })
        .filter(|(mnt, _)| under(path, mnt))
        .max_by_key(|(mnt, _)| mnt.len())
        .is_some_and(|(_, fstype)| NETWORK.contains(&fstype))
}

/// macOS: a volume under `/Volumes/` that sysinfo does not list is not a local one (it
/// lists local volumes only), even though the path matches `/`.
pub fn unlisted_mac_volume(path: &str, best_mount: Option<&str>) -> bool {
    path.starts_with("/Volumes/") && !best_mount.is_some_and(|m| m.starts_with("/Volumes/"))
}

fn ms(d: std::time::Duration) -> u64 {
    d.as_millis() as u64
}

/// Observes this machine. Blocking (it spawns two shells): call it off the async runtime,
/// and AFTER the first scan, so its shells never compete with the scan's own probes — the
/// scan is serial on purpose.
pub fn collect(os: Os, kit_dir: &Path, routes: &[&str]) -> Profile {
    use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};
    let started = std::time::Instant::now();
    let home = crate::detect::user_home().to_string_lossy().into_owned();

    let sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing())
            .with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    let disks = Disks::new_with_refreshed_list();
    let mounts: Vec<String> = disks
        .list()
        .iter()
        .map(|d| d.mount_point().to_string_lossy().into_owned())
        .collect();
    let free = |i: usize| {
        let d = &disks.list()[i];
        DiskFree {
            mount: tilde(&mounts[i], &home),
            free_mb: d.available_space() / 1_048_576,
            total_mb: d.total_space() / 1_048_576,
        }
    };
    let system_root = match os {
        Os::Windows => std::env::var("SystemDrive")
            .map(|d| format!("{d}\\"))
            .unwrap_or_else(|_| "C:\\".into()),
        _ => "/".into(),
    };
    let kit_path = kit_dir.to_string_lossy().into_owned();
    let kit_mount = disk_for(&kit_path, &mounts);
    // Windows lists every local drive, so no match means a mapped network drive. POSIX
    // always matches `/`, so each OS asks its own question (see the two functions).
    let on_local_disk = match os {
        Os::Windows => kit_mount.is_some(),
        Os::Linux => !network_fs_linux(
            &kit_path,
            &std::fs::read_to_string("/proc/self/mounts").unwrap_or_default(),
        ),
        Os::Darwin => !unlisted_mac_volume(&kit_path, kit_mount.map(|i| mounts[i].as_str())),
    };

    let shell_started = std::time::Instant::now();
    let empty = match os {
        Os::Windows => "$null",
        _ => ":",
    };
    crate::detect::run_probe_detailed(&crate::platform::shell_probe(os, empty));
    let shell_ms = ms(shell_started.elapsed());

    let tools_started = std::time::Instant::now();
    let names = tools_for(os, routes);
    let out = crate::detect::run_probe_detailed(&crate::platform::shell_probe(
        os,
        &tools_command(os, &names),
    ));
    let tools_ms = ms(tools_started.elapsed());
    let mut report = parse_tools(&out.output);
    for t in &mut report.tools {
        t.path = t.path.as_deref().map(|p| tilde(p, &home));
    }

    Profile {
        kind: "machine".into(),
        at: chrono::Utc::now().to_rfc3339(),
        talos: format!("{} {}", crate::build_info::TAG, crate::build_info::SHA),
        os: System::long_os_version().unwrap_or_default(),
        os_version: System::os_version().unwrap_or_default(),
        arch: System::cpu_arch(),
        cpu: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default(),
        cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        ram_total_mb: sys.total_memory() / 1_048_576,
        ram_available_mb: sys.available_memory() / 1_048_576,
        system_disk: disk_for(&system_root, &mounts).map(free),
        kit: Kit {
            path: tilde(&kit_path, &home),
            location: kit_location(&kit_path, on_local_disk).into(),
        },
        shell_ms,
        tools: report.tools,
        tools_ms,
        winget: report.winget,
        antivirus: report.antivirus,
        realtime: report.realtime,
        power: report.power,
        policies: report.policies,
        user_path_kind: report.user_path_kind,
        collect_ms: ms(started.elapsed()),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_manager_and_git_come_first_then_each_route_once() {
        let t = tools_for(
            Os::Windows,
            &[
                "winget",
                "npm",
                "claude-plugin",
                "npm",
                "run",
                "copilot-plugin",
            ],
        );
        assert_eq!(t, vec!["winget", "git", "npm", "claude", "copilot"]);
        let t = tools_for(
            Os::Darwin,
            &["brew", "vscode-extension", "skill", "nu-plugin"],
        );
        assert_eq!(t, vec!["brew", "git", "code", "npx", "nu"]);
    }

    #[test]
    fn the_windows_command_has_no_double_quote_and_no_ampersands() {
        // It travels as a `-Command` argument: a double quote is re-parsed on the way, and
        // `&&` would be rewritten by `win_and_then` into a guard — neither may appear.
        let c = tools_command(Os::Windows, &["winget", "git"]);
        assert!(!c.contains('"'), "{c}");
        assert!(!c.contains("&&"), "{c}");
        assert!(c.contains("@('winget','git')"), "{c}");
        for key in ["'alias'", "'onpath'", "'av'", "'rt'", "'power'"] {
            assert!(c.contains(key), "{key} missing: {c}");
        }
    }

    #[test]
    fn no_two_powershell_variables_differ_only_by_case() {
        // PowerShell variable names ignore case. MEASURED in the Windows VM: the separator was
        // `$T` and the loop variable `$t` — the same variable — so the loop overwrote the
        // separator and every line came back glued (`toolgitgitgit…`), parsing to no tool at
        // all. A spelling test cannot see that; this one can.
        let c = tools_command(Os::Windows, &["winget"]);
        let re = regex::Regex::new(r"\$([A-Za-z_]\w*)").unwrap();
        let mut seen: std::collections::HashMap<String, String> = Default::default();
        for m in re.captures_iter(&c) {
            let name = m[1].to_string();
            let prev = seen
                .entry(name.to_ascii_lowercase())
                .or_insert(name.clone());
            assert_eq!(*prev, name, "two spellings of one PowerShell variable: {c}");
        }
    }

    #[test]
    fn the_windows_command_reaches_powershell_whole_through_the_one_wrapping() {
        let c = tools_command(Os::Windows, &["winget"]);
        let p = crate::platform::shell_probe(Os::Windows, &c);
        let line = p.args.last().unwrap();
        assert!(line.contains("Microsoft\\WindowsApps"), "{line}");
        assert!(line.contains("Get-Command $t"), "{line}");
    }

    #[test]
    fn the_posix_command_asks_power_only_on_a_mac() {
        let mac = tools_command(Os::Darwin, &["brew", "git"]);
        assert!(mac.contains("command -v brew"), "{mac}");
        assert!(mac.contains("pmset"), "{mac}");
        assert!(!tools_command(Os::Linux, &["git"]).contains("pmset"));
    }

    #[test]
    fn the_posix_command_is_plain_enough_for_any_login_shell() {
        // The probe runs in the USER's shell: a fish or nu login shell rejects sh syntax
        // whole. No loop, no case, no command substitution.
        let c = tools_command(Os::Darwin, &["brew", "git"]);
        for sh_only in ["for ", " do ", "done", "case ", "esac", "$(", "`"] {
            assert!(!c.contains(sh_only), "{sh_only:?} in {c}");
        }
    }

    // Unix only: CI runs this suite on windows-latest, which has no /bin/sh.
    #[cfg(unix)]
    #[test]
    fn the_posix_command_runs_and_parses_under_sh() {
        // A real run, through /bin/sh, of exactly what the probe sends: a found tool, a
        // missing one, and nothing glued between them.
        let c = tools_command(Os::Linux, &["sh", "talos-no-such-tool"]);
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &c])
            .output()
            .unwrap();
        let r = parse_tools(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(r.tools.len(), 2, "{r:?}");
        assert!(
            r.tools[0]
                .path
                .as_deref()
                .is_some_and(|p| p.ends_with("/sh")),
            "{r:?}"
        );
        assert_eq!(r.tools[1].path, None, "{r:?}");
    }

    #[test]
    fn pmset_says_ac_or_battery() {
        let ac = parse_tools("Now drawing from 'AC Power'\n");
        assert_eq!(ac.power.as_deref(), Some("ac"));
        let bat = parse_tools("Now drawing from 'Battery Power'\n");
        assert_eq!(bat.power.as_deref(), Some("battery"));
    }

    #[test]
    fn only_a_discharging_battery_status_reads_battery() {
        // Win32_Battery.BatteryStatus: 1 discharging, 4/5 low/critical — everything else
        // (2 AC, 3 full, 6–9 and 11 charging) is a laptop on its charger.
        let c = tools_command(Os::Windows, &["winget"]);
        assert!(c.contains("@(1,4,5) -contains $b[0].BatteryStatus"), "{c}");
        assert!(!c.contains("BatteryStatus -eq 2"), "{c}");
    }

    #[test]
    fn parsing_reads_known_lines_and_ignores_a_shell_banner() {
        let out = "Welcome to zsh\r\n\
                   tool\twinget\tC:\\Users\\a\\AppData\\Local\\Microsoft\\WindowsApps\\winget.exe\r\n\
                   tool\tgit\t\r\n\
                   alias\tTrue\r\n\
                   onpath\tFalse\r\n\
                   av\tWindows Defender\r\n\
                   av\tSome EDR\r\n\
                   rt\tTrue\r\n\
                   power\tbattery\r\n";
        let r = parse_tools(out);
        assert_eq!(r.tools.len(), 2);
        assert_eq!(
            r.tools[0].path.as_deref(),
            Some("C:\\Users\\a\\AppData\\Local\\Microsoft\\WindowsApps\\winget.exe")
        );
        assert_eq!(
            r.tools[1],
            Tool {
                name: "git".into(),
                path: None
            }
        );
        assert_eq!(
            r.winget,
            Some(WingetAlias {
                alias: true,
                on_path: false
            })
        );
        assert_eq!(r.antivirus, vec!["Windows Defender", "Some EDR"]);
        assert_eq!(r.realtime, Some(true));
        assert_eq!(r.power.as_deref(), Some("battery"));
    }

    #[test]
    fn the_windows_command_reads_the_policies_that_govern_winget_and_the_user_path_kind() {
        // Facts, not verdicts: every value under the two policy keys, as stored, plus the
        // registry TYPE of the user PATH. What any of it blocks is the content's to say.
        let c = tools_command(Os::Windows, &["winget"]);
        assert!(
            c.contains("HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\AppInstaller"),
            "{c}"
        );
        assert!(
            c.contains("HKLM:\\SOFTWARE\\Policies\\Microsoft\\WindowsStore"),
            "{c}"
        );
        assert!(c.contains("GetValueKind('Path')"), "{c}");
        for key in ["'policy'", "'policyscan'", "'pathkind'"] {
            assert!(c.contains(key), "{key} missing: {c}");
        }
    }

    #[test]
    fn policies_are_read_as_stored_and_none_set_differs_from_not_read() {
        let r = parse_tools(
            "policy\tAppInstaller.EnableDefaultSource\t0\r\n\
             policy\tWindowsStore.RequirePrivateStoreOnly\t1\r\n\
             policyscan\tdone\r\n\
             pathkind\tString\r\n",
        );
        assert_eq!(
            r.policies,
            Some(vec![
                Policy {
                    name: "AppInstaller.EnableDefaultSource".into(),
                    value: "0".into()
                },
                Policy {
                    name: "WindowsStore.RequirePrivateStoreOnly".into(),
                    value: "1".into()
                },
            ])
        );
        assert_eq!(r.user_path_kind.as_deref(), Some("String"));
        // The scan ran and found nothing: an empty list, which the card says as "none set".
        assert_eq!(parse_tools("policyscan\tdone\n").policies, Some(vec![]));
        // The scan never answered (a POSIX machine, a probe cut short): no claim at all.
        assert_eq!(parse_tools("").policies, None);
        assert_eq!(parse_tools("").user_path_kind, None);
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_command_runs_and_reports_the_path_kind() {
        // A real run on the CI's windows-latest: the user PATH exists there, so its kind
        // must come back, and the policy scan must finish (none set is still an answer).
        let c = tools_command(Os::Windows, &["git"]);
        let p = crate::platform::shell_probe(Os::Windows, &c);
        let out = std::process::Command::new(&p.cmd)
            .args(&p.args)
            .output()
            .unwrap();
        let r = parse_tools(&String::from_utf8_lossy(&out.stdout));
        assert!(r.policies.is_some(), "{r:?}");
        assert!(r.user_path_kind.is_some(), "{r:?}");
    }

    #[test]
    fn a_half_answer_about_the_winget_alias_is_no_answer() {
        // Only one of the two facts came back: reporting `on_path: false` by default would
        // invent the very diagnosis this is here to make.
        assert_eq!(parse_tools("alias\tTrue\n").winget, None);
        assert_eq!(parse_tools("").winget, None);
        assert_eq!(parse_tools("power\tmaybe\n").power, None);
    }

    #[test]
    fn the_home_folder_is_written_as_a_tilde() {
        assert_eq!(
            tilde("/home/alice/.cargo/bin/cargo", "/home/alice"),
            "~/.cargo/bin/cargo"
        );
        assert_eq!(tilde("/home/alicex/bin", "/home/alice"), "/home/alicex/bin");
        assert_eq!(tilde("/usr/bin/git", "/home/alice"), "/usr/bin/git");
        assert_eq!(
            tilde("c:\\users\\Alice\\AppData\\Local", "C:\\Users\\alice"),
            "~\\AppData\\Local"
        );
        assert_eq!(tilde("/opt/x", ""), "/opt/x");
    }

    #[test]
    fn a_message_quoting_the_home_folder_names_no_one() {
        assert_eq!(
            tilde_all("panicked at C:\\Users\\alice\\x: oops", "C:\\Users\\alice"),
            "panicked at ~\\x: oops"
        );
        assert_eq!(tilde_all("no path", ""), "no path");
    }

    #[test]
    fn the_kit_location_names_the_slow_paths() {
        assert_eq!(kit_location("\\\\srv\\share\\Talos", true), "network");
        assert_eq!(kit_location("Z:\\Talos", false), "network");
        assert_eq!(
            kit_location("C:\\Users\\a\\OneDrive - Corp\\Talos\\Windows", true),
            "onedrive"
        );
        assert_eq!(
            kit_location("/Users/a/Library/CloudStorage/GoogleDrive-x/Talos", true),
            "synced"
        );
        assert_eq!(kit_location("C:\\Tools\\Talos", true), "local");
    }

    #[test]
    fn a_path_lives_on_its_longest_mount() {
        let m = vec!["/".to_string(), "/home".to_string(), "C:\\".to_string()];
        assert_eq!(disk_for("/home/a/kit", &m), Some(1));
        assert_eq!(disk_for("/opt/kit", &m), Some(0));
        assert_eq!(disk_for("c:\\Tools", &m), Some(2));
        assert_eq!(disk_for("\\\\srv\\share", &m[2..]), None);
        let n = vec!["/".to_string(), "/mnt/data".to_string()];
        assert_eq!(
            disk_for("/mnt/data2/kit", &n),
            Some(0),
            "a prefix is not a folder"
        );
        assert_eq!(disk_for("/mnt/data/kit", &n), Some(1));
        assert_eq!(disk_for("/mnt/data", &n), Some(1));
        assert_eq!(
            disk_for("/é", &["/x".to_string()]),
            None,
            "no panic off a char boundary"
        );
    }

    #[test]
    fn a_kit_on_a_network_filesystem_is_named_on_linux_and_mac() {
        let mounts = "/dev/nvme0n1p2 / btrfs rw 0 0\n\
                      /dev/nvme0n1p2 /home btrfs rw 0 0\n\
                      //srv/share /mnt/team\\040share cifs rw 0 0\n";
        assert!(network_fs_linux("/mnt/team share/Talos/Linux", mounts));
        assert!(!network_fs_linux("/home/a/Talos/Linux", mounts));
        assert!(!network_fs_linux("/mnt/teamwork/Talos", mounts));
        assert!(unlisted_mac_volume("/Volumes/Team/Talos/MacOS", Some("/")));
        assert!(!unlisted_mac_volume(
            "/Volumes/USB/Talos",
            Some("/Volumes/USB")
        ));
        assert!(!unlisted_mac_volume("/Users/a/Talos", Some("/")));
    }

    /// A REAL collection on the machine running the tests: two shells and a disk listing,
    /// so it is ignored by default. `cargo test machine -- --ignored --nocapture` prints the
    /// line the journal would receive here.
    #[test]
    #[ignore]
    fn collects_this_machine() {
        let os = crate::platform::current_os();
        let here = std::env::current_dir().unwrap();
        let p = collect(os, &here, &["npm", "cargo"]);
        println!("{}", serde_json::to_string_pretty(&p).unwrap());
        assert!(p.cores > 0 && p.ram_total_mb > 0, "{p:?}");
        assert!(p.tools.iter().any(|t| t.name == "git"), "{p:?}");
    }

    #[test]
    fn a_profile_serialises_without_its_empty_optionals() {
        let p = Profile {
            kind: "machine".into(),
            ..Default::default()
        };
        let line = serde_json::to_string(&p).unwrap();
        for absent in ["antivirus", "winget", "power", "realtime", "system_disk"] {
            assert!(!line.contains(absent), "{absent} in {line}");
        }
        assert!(line.contains("\"shell_ms\":0"), "{line}");
    }
}
