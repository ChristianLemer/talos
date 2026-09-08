// Os + the shell wrapping (shell_probe / pty_shell).
// The single source of "which OS and how to act on it".

use std::path::PathBuf;
use std::process::Command;

/// Builds a `Command` that does NOT pop up a console window on Windows.
/// Talos.exe is in the GUI subsystem, but each child launched via `Command`
/// (powershell/winget at scan time) creates ITS OWN console — hence the black window
/// that appears "later", during the scan. CREATE_NO_WINDOW (0x0800_0000) suppresses it.
/// Elsewhere (macOS/Linux): a plain `Command::new`, the flag does not exist.
///
/// ⭐ It also gets the same safe working directory the pty gets (`pty::safe_working_dir`).
/// PROBES and ACTIONS must not disagree about where they run: `claude` refuses a `git.exe`
/// living under the cwd, so a probe left on the launch folder could report a package
/// absent while the action installs it perfectly — a row that acts correctly and detects
/// wrong is worse than one that fails outright.
///
/// Not reachable today (plugin presence is a native disk read of `installed_plugins.json`,
/// never a shell-out to `claude`), and set anyway: the cost is one call, and the day
/// someone writes `detect: claude plugin list` the trap would be silent. Detect,
/// don't remember — but detect from the same place you act.
pub fn quiet_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if let Some(dir) = crate::pty::safe_working_dir() {
        cmd.current_dir(dir);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Darwin,
    Linux,
}

/// Opens a URL in the default browser, outside the panel. Per-OS:
/// Windows → `cmd /C start "" <url>` (the first "" is the TITLE that `start` requires,
/// otherwise it takes the URL as a title); macOS → `open`; Linux → `xdg-open`. Via
/// quiet_command → no console window flickering on Windows. Best-effort:
/// a spawn failure is logged, never fatal (the front-end keeps the clickable link as fallback).
pub fn open_url(url: &str) -> std::io::Result<()> {
    let mut cmd = match current_os() {
        Os::Windows => {
            let mut c = quiet_command("cmd");
            c.args(["/C", "start", "", url]);
            c
        }
        Os::Darwin => {
            let mut c = quiet_command("open");
            c.arg(url);
            c
        }
        Os::Linux => {
            let mut c = quiet_command("xdg-open");
            c.arg(url);
            c
        }
    };
    cmd.spawn().map(|_| ())
}

/// Status of the macOS "App Management" permission (TCC
/// kTCCServiceSystemPolicyAppBundles). No public API exists — on macOS we probe
/// reality by trying to write inside a real /Applications/*.app we don't own and
/// reading EPERM. Reality, never TCC.db nor the toggle's appearance (survives the
/// ad-hoc re-grant trap where the toggle looks ON but the new cdhash is denied).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMgmtStatus {
    /// Only ever CONSTRUCTED by the macOS write-probe, so on any other target the
    /// variant is dead code in the bin — hence the cfg'd allow (CI runs clippy
    /// `-D warnings` on Linux). It stays in the enum for all targets because
    /// `as_str` and the wire protocol are cross-platform.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Granted,
    Missing,
    /// Non-macOS, or no suitable bundle to probe → never blocks.
    NotApplicable,
}

impl AppMgmtStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppMgmtStatus::Granted => "granted",
            AppMgmtStatus::Missing => "missing",
            AppMgmtStatus::NotApplicable => "na",
        }
    }
}

/// Probes the App Management permission. macOS only; other OSes → NotApplicable.
pub fn app_management_status(os: Os) -> AppMgmtStatus {
    if os != Os::Darwin {
        return AppMgmtStatus::NotApplicable;
    }
    #[cfg(target_os = "macos")]
    {
        probe_app_management()
    }
    #[cfg(not(target_os = "macos"))]
    {
        AppMgmtStatus::NotApplicable
    }
}

/// What one write attempt inside an app bundle told us. The distinction that
/// matters is WHICH refusal we got, and Rust's `ErrorKind::PermissionDenied` hides
/// it: EACCES and EPERM both land there.
/// macOS-only in practice: only `probe_app_management` (cfg'd to macOS) constructs
/// these, so on Linux/Windows the enum and its verdict function have no caller. CI
/// runs clippy `-D warnings` on Linux — hence the cfg'd allow rather than a cfg on
/// the items, which would take their unit test out of the build on the dev machine
/// too. Same pattern as `AppMgmtStatus::Granted` above.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppMgmtProbe {
    /// The witness file was created (and removed) → we may modify this bundle.
    Wrote,
    /// EACCES — an ordinary POSIX refusal: the directory's mode/owner exclude us.
    /// Says NOTHING about App Management; a root:wheel 755 bundle refuses this way
    /// whether the permission is granted or not.
    PosixRefused,
    /// EPERM on a path POSIX would have allowed → the refusal came from TCC, i.e.
    /// App Management is missing. This is the only real signal.
    TccRefused,
    /// Anything else (read-only volume, transient IO, unreadable metadata).
    Inconclusive,
}

/// The verdict ONE probe supports, or `None` when it is not evidence and the caller
/// must keep looking. Pure, so the rule is testable without touching /Applications.
///
/// ⚠️ `PosixRefused` must NOT answer Missing. That was the bug: with 38 of 41
/// root-owned bundles refusing by plain POSIX, and the probe returning on the first
/// one it met, the answer was Missing forever — grant the permission, reboot, and the
/// banner stayed. A refusal is only meaningful when POSIX would have said yes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn appmgmt_verdict(p: AppMgmtProbe) -> Option<AppMgmtStatus> {
    match p {
        AppMgmtProbe::Wrote => Some(AppMgmtStatus::Granted),
        AppMgmtProbe::TccRefused => Some(AppMgmtStatus::Missing),
        AppMgmtProbe::PosixRefused | AppMgmtProbe::Inconclusive => None,
    }
}

/// macOS write-probe: try to create+remove a witness file inside a bundle's
/// `Contents/` that POSIX says we may write, and read WHICH error comes back.
/// EPERM → TCC refused → Missing; success → Granted; EACCES → not evidence, keep
/// looking; nothing conclusive anywhere → NotApplicable (never block on a guess).
#[cfg(target_os = "macos")]
fn probe_app_management() -> AppMgmtStatus {
    let apps = match std::fs::read_dir("/Applications") {
        Ok(rd) => rd,
        Err(_) => return AppMgmtStatus::NotApplicable,
    };
    for entry in apps.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("app") {
            continue;
        }
        let contents = path.join("Contents");
        if !contents.is_dir() {
            continue;
        }
        // Probe a bundle POSIX says we MAY write, and read WHICH refusal comes back.
        //
        // ⚠️ The previous rule — "only probe root-owned bundles" — looked right and was
        // the bug. On a real Mac, 38 of 41 root-owned bundles are root:wheel mode 755,
        // so the write is refused by plain POSIX (EACCES) no matter what TCC says. The
        // probe returned on the first one it met (read_dir order is arbitrary) and
        // answered Missing forever: grant App Management, reboot, banner still there.
        // Confirmed against TCC.db, which said auth_value=2 (granted) at the time.
        //
        // Note also why /Applications is a MIXTURE of owners, which is normal: brew or
        // a .pkg lays the outer .app down as root, then an app's own updater rewrites
        // `Contents/` as the logged-in user and becomes its owner. So ownership tells
        // you who wrote last, not whether the bundle is protected.
        //
        // What DOES mean something: a refusal on a path our uid+mode allow. That can
        // only come from TCC → EPERM. So skip anything POSIX would refuse anyway.
        let Ok(meta) = std::fs::metadata(&contents) else {
            continue; // unreadable → no evidence
        };
        // A valid target needs BOTH conditions, and each one rules out a real false
        // verdict measured on this machine:
        //   · POSIX must ALLOW us — else the refusal is EACCES noise (38 of 41
        //     root-owned bundles are root:wheel 755) and we would read "not the owner"
        //     as "permission missing". This was the reported bug.
        //   · the bundle must NOT be ours — a bundle we own accepts the write even when
        //     App Management is DENIED (verified: a client with TCC auth_value=0 wrote
        //     into a user-owned bundle without complaint), so it would answer Granted
        //     while the permission is refused. That is the older false-Granted bug the
        //     previous comment warned about, and it was right to.
        // Both together, the write can only be refused by TCC — which is the signal.
        if !posix_writable(&meta) || is_ours(&meta) {
            continue;
        }
        let witness = contents.join(".talos-appmgmt-probe");
        let outcome = match std::fs::File::create(&witness) {
            Ok(_) => {
                let _ = std::fs::remove_file(&witness);
                AppMgmtProbe::Wrote
            }
            Err(e) => classify_write_error(&e),
        };
        if let Some(verdict) = appmgmt_verdict(outcome) {
            return verdict;
        }
    }
    // Nothing conclusive anywhere → we cannot constate; never block on a guess.
    AppMgmtStatus::NotApplicable
}

/// Would POSIX alone let US write into this directory? Owner-writable and ours, or
/// group-writable and we are in the group, or world-writable. Used to skip probe
/// targets whose refusal would say nothing about App Management.
#[cfg(target_os = "macos")]
fn posix_writable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    let mode = m.mode();
    if mode & 0o002 != 0 {
        return true; // world-writable
    }
    // Our own uid/gid, without a direct `libc` dependency: compare against a file we
    // certainly own. $HOME's metadata gives both, and if it is unreadable we fall back
    // to "not writable", which only makes the probe skip this target — never a wrong
    // verdict.
    let Some(me) = std::env::var_os("HOME").and_then(|h| std::fs::metadata(h).ok()) else {
        return mode & 0o002 != 0;
    };
    (m.uid() == me.uid() && mode & 0o200 != 0) || (m.gid() == me.gid() && mode & 0o020 != 0)
}

/// Do WE own this directory? A bundle we own accepts the write regardless of App
/// Management, so it can never prove the permission is granted.
#[cfg(target_os = "macos")]
fn is_ours(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    match std::env::var_os("HOME").and_then(|h| std::fs::metadata(h).ok()) {
        Some(me) => m.uid() == me.uid(),
        None => false, // cannot tell → treat as not ours; the write outcome still decides
    }
}

/// EPERM vs EACCES — the distinction `ErrorKind::PermissionDenied` erases. EPERM on a
/// POSIX-writable path is TCC saying no; EACCES is the filesystem saying no.
#[cfg(target_os = "macos")]
fn classify_write_error(e: &std::io::Error) -> AppMgmtProbe {
    // Numeric on purpose: EPERM=1 and EACCES=13 are fixed by POSIX and identical on
    // every Darwin, and hardcoding them avoids a direct `libc` dependency for two ints.
    const EPERM: i32 = 1;
    const EACCES: i32 = 13;
    match e.raw_os_error() {
        Some(EPERM) => AppMgmtProbe::TccRefused,
        Some(EACCES) => AppMgmtProbe::PosixRefused,
        _ => AppMgmtProbe::Inconclusive,
    }
}

/// Anything not windows/darwin → linux (the shell family we support there). Never panics.
pub fn current_os() -> Os {
    match std::env::consts::OS {
        "windows" => Os::Windows,
        "macos" => Os::Darwin,
        _ => Os::Linux,
    }
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub cmd: String,
    pub args: Vec<String>,
}

/// Per-machine LOCAL data-dir — where selection/consent/history land. NEVER
/// the shared exe folder (OneDrive): the exe is launched by N machines, so everything
/// written must live on each machine's own disk.
/// Windows → %LOCALAPPDATA%\Talos ; Mac → ~/Library/Application Support/Talos ;
/// Linux → $XDG_DATA_HOME/Talos (or ~/.local/share/Talos). Port of localDataDir().
pub fn local_data_dir(os: Os) -> PathBuf {
    match os {
        Os::Windows => {
            let base = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let up = std::env::var_os("USERPROFILE")
                        .map(PathBuf::from)
                        .unwrap_or_default();
                    up.join("AppData").join("Local")
                });
            base.join("Talos")
        }
        Os::Darwin => home_dir()
            .join("Library")
            .join("Application Support")
            .join("Talos"),
        Os::Linux => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".local").join("share"))
            .join("Talos"),
    }
}

/// The user's home, where a terminal opened for a person starts. `USERPROFILE` on
/// Windows, `HOME` elsewhere; None when the environment does not say.
pub fn user_home(os: Os) -> Option<PathBuf> {
    let var = if os == Os::Windows {
        "USERPROFILE"
    } else {
        "HOME"
    };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The "next-to-the-exe" folder — where `bundles/` lives (hermetic boundary: the engine
/// changes rarely, the bundles often, so NEXT TO the binary, read at runtime, NEVER
/// sealed). Rust mirror of the TS `BUNDLES_DIR` (dirname(execPath) in compiled mode).
///
/// macOS wrinkle: in a `.app`, the exe is `Talos.app/Contents/MacOS/talos`. "Next to"
/// in the hermetic sense = next to the `.app` (editable without touching the signed bundle), not
/// `Contents/MacOS/`. So if we detect the pattern `…/X.app/Contents/MacOS/<exe>`, we
/// climb out of the `.app`. Otherwise (bare binary in dev, Windows/Linux exe): the direct parent.
/// PURE function (takes the exe path) → testable without launching a process.
pub fn exe_sibling_dir(exe: &std::path::Path) -> PathBuf {
    let parent = exe.parent().unwrap_or(std::path::Path::new("."));
    // .app pattern: parent = ".../Contents/MacOS", grandparent = ".../Contents",
    // great-grandparent = ".../X.app" → we want the folder THAT CONTAINS X.app.
    if parent.file_name().is_some_and(|n| n == "MacOS") {
        if let Some(contents) = parent.parent() {
            if contents.file_name().is_some_and(|n| n == "Contents") {
                if let Some(app) = contents.parent() {
                    // app = ".../X.app"; its parent = the folder where bundles/ goes.
                    if app.extension().is_some_and(|e| e == "app") {
                        return app.parent().unwrap_or(app).to_path_buf();
                    }
                }
            }
        }
    }
    parent.to_path_buf()
}

/// The Doctor's FLOOR: the shell the OS ships, started without a profile, by absolute path.
///
/// It exists for the case where no declared rescue candidate is present — the agent never
/// installed, its route broken, the network gone. A rescue that depends on an installation
/// having succeeded is not a rescue; this one depends only on what the OS ships. No
/// profile, so a broken rc file cannot stop it. None only if even that binary is missing.
/// Windows: `-NoProfile`. macOS: `zsh -f`. Linux: `bash --noprofile --norc` (measured), then
/// `/bin/sh`.
pub fn rescue_shell(os: Os) -> Option<(PathBuf, Vec<String>)> {
    let candidates: Vec<(PathBuf, &[&str])> = match os {
        Os::Windows => {
            let root = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            vec![(
                root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
                &["-NoProfile"][..],
            )]
        }
        Os::Darwin => vec![
            (PathBuf::from("/bin/zsh"), &["-f"][..]),
            (PathBuf::from("/bin/sh"), &[][..]),
        ],
        Os::Linux => vec![
            (PathBuf::from("/bin/bash"), &["--noprofile", "--norc"][..]),
            (PathBuf::from("/bin/sh"), &[][..]),
        ],
    };
    candidates
        .into_iter()
        .find(|(p, _)| p.is_file())
        .map(|(p, a)| (p, a.iter().map(|x| x.to_string()).collect()))
}

/// The folder next to the running exe — outside the `.app` on macOS. See `exe_sibling_dir`.
pub fn exe_sibling() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|p| exe_sibling_dir(&p))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where `catalog/` and `bundles/` are read from — ONE rule, two callers: the server at
/// boot, and `--check` from the command line. Each folder is looked for in `sibling` (next
/// to the exe, outside the `.app` on macOS — the packaged case); if absent there, it falls
/// back to the current directory — the DEV case (`cargo run` from the repo root, where the
/// exe is `target/debug/Talos` but the content is `./catalog` + `./bundles`). Resolved from
/// the REAL exe first, never the cwd alone: a `.app` launched by Finder has cwd=/ and a
/// relative "bundles" opened empty. Absent in both places → the relative name, which loads
/// empty. PURE over `sibling` → testable without launching a process.
pub fn content_dirs_in(sibling: &std::path::Path) -> (PathBuf, PathBuf) {
    let pick = |name: &str| -> PathBuf {
        let beside = sibling.join(name);
        if beside.is_dir() {
            beside
        } else {
            PathBuf::from(name)
        }
    };
    (pick("catalog"), pick("bundles"))
}

/// `content_dirs_in` for the running exe.
pub fn content_dirs() -> (PathBuf, PathBuf) {
    content_dirs_in(&exe_sibling())
}

// Windows PATH refresh: an install writes the registry but does NOT propagate the PATH
// to already-running processes → a freshly installed tool would read "absent" without this.
//
// ⚠️ It APPENDS, and that is right on its own merits: assigning threw away the live
// process PATH and rebuilt it from two registry keys, so anything living ONLY in the
// process environment disappeared — an MSIX/Store shim under WindowsApps, a directory
// an installer put on the session PATH only. A refresh must not be destructive.
//
// ⚠️ But this was NOT the cause of the "git not found" symptom it was first written
// for, and the correction matters more than the fix: the symptom RETURNED after
// beta.21 shipped this. Measured on the machine afterwards — no empty and no relative
// segment in the joined PATH, so no entry could be read as "current directory"; and
// `claude`'s own resolver does not consult the shell PATH at all, it calls `where.exe`
// with its process environment. The real cause was the working directory: see
// `pty::safe_working_dir`. A fix that does not make the symptom disappear was not the
// cause of it.
//
// The live PATH comes FIRST: it is the more specific answer (what this process was
// actually given), and PowerShell resolves left to right. Duplicates are harmless.
const WIN_PATH_REFRESH: &str =
    "$env:Path=$env:Path+';'+[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

/// The USER's POSIX shell (resolved from $SHELL, fallback /bin/zsh then
/// /bin/sh). "What the user sees in their terminal" — IT is the one that knows
/// the user's custom PATHs (~/.local/bin, added in their .zshrc/.bashrc).
fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Fallbacks: zsh (modern macOS default) if it exists, otherwise sh (always there).
            for cand in ["/bin/zsh", "/bin/bash"] {
                if std::path::Path::new(cand).exists() {
                    return Some(cand.to_string());
                }
            }
            None
        })
        .unwrap_or_else(|| "/bin/sh".into())
}

/// Builds the POSIX Probe for a given shell (PURE — OS and shell path are injected).
/// zsh/bash on macOS → `-ilc` (interactive + login): a GUI-launched app there inherits
/// NO user PATH, so only an interactive login shell sources /etc/profile (system PATH via
/// path_helper: /opt/homebrew) AND the user rc (.zshrc/.bashrc: ~/.local/bin, where claude,
/// uv, pip --user… live). ⚠️ `-lc` ALONE captures only the system PATH on macOS — that is
/// what produced a false "absent" on a tool installed in ~/.local/bin. Do not drop `-i` on
/// macOS to "simplify".
/// On Linux → `-lc` even for zsh/bash: the session already exports the user PATH
/// (uwsm/systemd import it), AND `bash -i` OUTSIDE a controlling terminal fails with
/// "bash: cannot set terminal process group … Inappropriate ioctl for device" and breaks
/// the probe (the upgrade scan exited 127). The interactive flag there buys nothing and costs
/// the probe.
/// A bare /bin/sh (neither zsh nor bash) → `-lc` alone (sh does not read the zsh/bash rc).
fn posix_probe(os: Os, shell: &str, command: &str) -> Probe {
    let is_rc_shell = shell.ends_with("zsh") || shell.ends_with("bash");
    let flags = if is_rc_shell && matches!(os, Os::Darwin) {
        "-ilc"
    } else {
        "-lc"
    };
    Probe {
        cmd: shell.to_string(),
        args: vec![flags.into(), command.into()],
    }
}

/// PS 5.1 (Windows's `powershell.exe`, path `v1.0`) does NOT know `&&` — the operator
/// only exists from PS 7 on. Yet install commands are written with `&&` (POSIX canonical,
/// run as-is on Mac via `bash -lc`). We translate it here into an equivalent PS string
/// that PRESERVES both the short-circuit AND the exit code: each `&&` becomes a guard that exits
/// early if the step failed. `A && B && C` → `A; if ($LASTEXITCODE -ne 0){exit …}; B; …; C`.
/// A command without `&&` passes through unchanged.
fn win_and_then(command: &str) -> String {
    command
        .split("&&")
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; ")
}

/// Wraps a STRING command into a Probe in the native shell. Windows: keeps the 127 guard
/// (try/catch Stop → exit 127) — a CommandNotFoundException does NOT set $LASTEXITCODE,
/// so "cmd; exit $LASTEXITCODE" would read 0 (false positive). POSIX: the USER's SHELL
/// in interactive+login (see posix_probe) → sees the system PATH AND ~/.local/bin.
pub fn shell_probe(os: Os, command: &str) -> Probe {
    match os {
        Os::Windows => {
            let command = win_and_then(command);
            let ps = format!(
                "{WIN_PATH_REFRESH} $ErrorActionPreference='Stop'; try {{ {command}; exit $LASTEXITCODE }} catch {{ exit 127 }}"
            );
            Probe {
                cmd: "powershell.exe".into(),
                args: vec!["-NoProfile".into(), "-Command".into(), ps],
            }
        }
        _ => posix_probe(os, &user_shell(), command),
    }
}

/// Wrap for the INTERACTIVE pty (install/upgrade/uninstall shown live). Diffs vs
/// shell_probe: NO 127 guard (the pty wants the real exit code); Windows keeps the
/// PATH refresh + exit $LASTEXITCODE. POSIX: SAME user shell in interactive+login as
/// shell_probe — otherwise we would detect claude (~/.local/bin) but could neither
/// install nor uninstall it (the install too must see the user's PATH).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn pty_shell(os: Os, command: &str) -> Probe {
    match os {
        Os::Windows => {
            let command = win_and_then(command);
            let ps = format!("{WIN_PATH_REFRESH} {command}; exit $LASTEXITCODE");
            Probe {
                cmd: "powershell.exe".into(),
                args: vec![
                    "-NoProfile".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-Command".into(),
                    ps,
                ],
            }
        }
        _ => posix_probe(os, &user_shell(), command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_shell_probe_login() {
        // shell_probe resolves the REAL shell from the env ($SHELL) → we test the CONTRACT,
        // not a fixed path: a login flag (-lc or -ilc) + the passed command.
        // (the -ilc vs -lc choice is covered precisely by posix_probe, pure.)
        let p = shell_probe(Os::Darwin, "brew list");
        assert!(
            p.args[0] == "-lc" || p.args[0] == "-ilc",
            "flag login, got {}",
            p.args[0]
        );
        assert_eq!(p.args[1], "brew list");
    }

    #[test]
    fn windows_shell_probe_has_127_guard() {
        let p = shell_probe(Os::Windows, "node --version");
        assert_eq!(p.cmd, "powershell.exe");
        assert!(p.args.last().unwrap().contains("exit 127"));
        assert!(p.args.last().unwrap().contains("node --version"));
    }

    #[test]
    fn windows_path_refresh_keeps_the_live_path() {
        // The assertion stands on its own: a refresh must not be destructive. Assigning
        // discarded the live process PATH and rebuilt it from two registry keys, losing
        // anything present only in the process environment (an MSIX/Store shim under
        // WindowsApps, a session-only directory). And it must still HAPPEN, or a freshly
        // installed tool would read absent until Talos restarted.
        //
        // ⚠️ Its original JUSTIFICATION was wrong, and is corrected here rather than
        // quietly dropped. This was written as the cause of `claude` answering "git not
        // found or is in an unsafe location" — it was not. The symptom returned after the
        // fix shipped (beta.21), and the measured cause is the working directory handed
        // to the pty: `claude` refuses a git living under the cwd, and never consults the
        // shell PATH for this at all. See `pty::safe_working_dir`.
        assert!(
            WIN_PATH_REFRESH.contains("$env:Path=$env:Path"),
            "the refresh must APPEND to the live PATH, never replace it: {WIN_PATH_REFRESH}"
        );
        // Both registry scopes still consulted — that is what the refresh is FOR.
        for scope in ["'Path','Machine'", "'Path','User'"] {
            assert!(
                WIN_PATH_REFRESH.contains(scope),
                "{scope} must still be read: {WIN_PATH_REFRESH}"
            );
        }
        // And it must reach the two wrappers, or the fix is theoretical: these are
        // the only two places that build a Windows command line.
        for line in [
            shell_probe(Os::Windows, "git --version")
                .args
                .pop()
                .unwrap(),
            pty_shell(Os::Windows, "git --version").args.pop().unwrap(),
        ] {
            assert!(
                line.contains("$env:Path=$env:Path"),
                "a wrapper lost the non-destructive refresh: {line}"
            );
        }
    }

    #[test]
    fn win_and_then_translates_double_ampersand() {
        // `&&` (POSIX canonical) → PS 5.1 guard that short-circuits on failure.
        let out = win_and_then("cargo uninstall ripgrep && cargo install ripgrep");
        assert!(!out.contains("&&"), "a && remains: {out}");
        assert!(out.contains("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"));
        assert!(out.starts_with("cargo uninstall ripgrep;"));
        assert!(out.ends_with("cargo install ripgrep"));
    }

    #[test]
    fn win_and_then_passes_through_simple_command() {
        // Without &&: unchanged (modulo trim), no spurious guard.
        assert_eq!(win_and_then("winget install Foo"), "winget install Foo");
    }

    #[test]
    fn windows_shell_probe_lowers_double_ampersand() {
        // End-to-end: a `&&` command must NEVER reach powershell.exe as-is
        // (PS 5.1: "The token '&&' is not a valid statement separator").
        let p = shell_probe(
            Os::Windows,
            "claude plugin marketplace add \"x\" && claude plugin install y --scope user",
        );
        assert!(
            !p.args.last().unwrap().contains("&&"),
            "&& leaked into PS: {:?}",
            p.args.last()
        );
    }

    #[test]
    fn posix_probe_zsh_interactive_login() {
        // macOS zsh/bash → -ilc: sources the user rc (.zshrc) → sees ~/.local/bin (claude, uv).
        let p = posix_probe(Os::Darwin, "/bin/zsh", "claude --version");
        assert_eq!(p.cmd, "/bin/zsh");
        assert_eq!(p.args, vec!["-ilc", "claude --version"]);
        let b = posix_probe(Os::Darwin, "/opt/homebrew/bin/bash", "node --version");
        assert_eq!(b.args[0], "-ilc");
    }

    #[test]
    fn posix_probe_linux_no_interactive() {
        // Linux zsh/bash → -lc, NOT -ilc: the session already exports PATH, and an
        // interactive shell outside a controlling terminal fails with "cannot set
        // terminal process group" (the upgrade scan exited 127). Regression guard.
        let z = posix_probe(Os::Linux, "/bin/zsh", "node --version");
        assert_eq!(z.args, vec!["-lc", "node --version"]);
        let b = posix_probe(Os::Linux, "/bin/bash", "node --version");
        assert_eq!(b.args[0], "-lc");
    }

    #[test]
    fn posix_probe_sh_login_only() {
        // bare /bin/sh does not read the zsh/bash rc → -lc alone (not -ilc, useless).
        let p = posix_probe(Os::Darwin, "/bin/sh", "node --version");
        assert_eq!(p.cmd, "/bin/sh");
        assert_eq!(p.args, vec!["-lc", "node --version"]);
    }

    /// The floor exists on the machine running the tests, and it is profile-free.
    #[test]
    fn the_rescue_shell_exists_and_skips_the_profile() {
        let (bin, args) = rescue_shell(current_os()).expect("an OS shell");
        assert!(bin.is_absolute(), "{}", bin.display());
        assert!(bin.is_file(), "{}", bin.display());
        // Every shell we know takes a "no profile" flag — `-NoProfile`, `--noprofile`,
        // `-f` — and /bin/sh has none to skip. Case-insensitive: the Windows CI job is the
        // one place this meets PowerShell's capitalised flag, and it caught a
        // lowercase-only check on 2026-09-08.
        let profile_free = args
            .iter()
            .any(|a| a.to_ascii_lowercase().contains("no") || a == "-f")
            || bin.ends_with("sh");
        assert!(profile_free, "{args:?}");
    }

    /// Beside the exe when the folder is there; the bare relative name otherwise — each of
    /// the two independently, so a kit with only `bundles/` beside still reads it.
    #[test]
    fn content_dirs_fall_back_per_folder() {
        let d = std::env::temp_dir().join(format!("talos-content-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bundles")).unwrap();
        let (catalog, bundles) = content_dirs_in(&d);
        assert_eq!(catalog, PathBuf::from("catalog"));
        assert_eq!(bundles, d.join("bundles"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn data_dir_ends_with_talos_per_os() {
        assert!(local_data_dir(Os::Darwin).ends_with("Library/Application Support/Talos"));
        assert!(local_data_dir(Os::Windows).ends_with("Talos"));
        assert!(local_data_dir(Os::Linux).ends_with("Talos"));
    }

    #[test]
    fn sibling_app_climbs_out_of_the_bundle() {
        // .app: bundles/ must land NEXT TO the .app, not in Contents/MacOS.
        let exe = std::path::Path::new("/Apps/OneDrive/Talos.app/Contents/MacOS/talos");
        assert_eq!(exe_sibling_dir(exe), PathBuf::from("/Apps/OneDrive"));
    }

    #[test]
    fn sibling_bare_binary_is_the_parent() {
        // Dev (cargo) or Windows/Linux exe: just the parent folder.
        // (POSIX paths: the test runs on Mac, where `\` is not a separator.)
        let exe = std::path::Path::new("/home/x/talos/target/release/talos");
        assert_eq!(
            exe_sibling_dir(exe),
            PathBuf::from("/home/x/talos/target/release")
        );
        let shared = std::path::Path::new("/mnt/onedrive/Talos/talos");
        assert_eq!(
            exe_sibling_dir(shared),
            PathBuf::from("/mnt/onedrive/Talos")
        );
    }

    #[test]
    fn appmgmt_status_variants_exist() {
        assert_eq!(AppMgmtStatus::Granted.as_str(), "granted");
        assert_eq!(AppMgmtStatus::Missing.as_str(), "missing");
        assert_eq!(AppMgmtStatus::NotApplicable.as_str(), "na");
    }

    #[test]
    fn appmgmt_verdict_ignores_a_plain_posix_refusal() {
        // THE bug this pins, measured on a real Mac: of 41 root-owned bundles in
        // /Applications, 38 refuse the write with EACCES — an ordinary POSIX refusal
        // (root:wheel, mode 755, we are not root). No App Management grant can ever
        // change that. Only 2 refuse with EPERM, which IS the TCC signal.
        //
        // Rust maps BOTH errnos to ErrorKind::PermissionDenied, so the old probe read
        // "I am not the owner" as "the permission is missing" — and since it returned
        // on the FIRST root-owned bundle it met (read_dir order is arbitrary), it
        // answered Missing forever, whatever the user configured. Exactly the reported
        // symptom: grant it, reboot, banner still there.
        use AppMgmtProbe::*;
        // Not writable by us anyway → the probe proves nothing about TCC. Keep looking.
        assert_eq!(appmgmt_verdict(PosixRefused), None);
        // Writable per POSIX, yet refused → that refusal can only be TCC.
        assert_eq!(appmgmt_verdict(TccRefused), Some(AppMgmtStatus::Missing));
        assert_eq!(appmgmt_verdict(Wrote), Some(AppMgmtStatus::Granted));
        // Anything else (transient IO, read-only volume…) is not evidence either.
        assert_eq!(appmgmt_verdict(Inconclusive), None);
    }

    #[test]
    fn appmgmt_non_macos_is_na() {
        assert_eq!(
            app_management_status(Os::Windows),
            AppMgmtStatus::NotApplicable
        );
        assert_eq!(
            app_management_status(Os::Linux),
            AppMgmtStatus::NotApplicable
        );
    }

    #[test]
    fn sibling_macos_without_app_pattern_stays_parent() {
        // A "MacOS" folder that is NOT inside a .app → no magic climb-out.
        let exe = std::path::Path::new("/random/MacOS/talos");
        assert_eq!(exe_sibling_dir(exe), PathBuf::from("/random/MacOS"));
    }
}

#[cfg(test)]
mod appmgmt_live {
    /// LIVE probe on this machine — ignored by default because the answer depends on
    /// the operator's own TCC state, so it must never gate CI. Run it deliberately:
    ///   cargo test --bins appmgmt_live -- --ignored --nocapture
    /// and compare with what TCC.db says for be.lemer.talos.
    #[test]
    #[ignore]
    fn what_does_this_machine_say() {
        let v = super::app_management_status(super::current_os());
        println!("app_management_status() = {}", v.as_str());
    }
}
