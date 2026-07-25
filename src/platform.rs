// Port of platform.ts — Os + the shell wrapping (shell_probe / pty_shell).
// The single source of "which OS and how to act on it".

use std::path::PathBuf;
use std::process::Command;

/// Builds a `Command` that does NOT pop up a console window on Windows.
/// Talos.exe is in the GUI subsystem, but each child launched via `Command`
/// (powershell/winget at scan time) creates ITS OWN console — hence the black window
/// that appears "later", during the scan. CREATE_NO_WINDOW (0x0800_0000) suppresses it.
/// Elsewhere (macOS/Linux): a plain `Command::new`, the flag does not exist.
pub fn quiet_command(program: &str) -> Command {
    let cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        return cmd;
    }
    #[cfg(not(target_os = "windows"))]
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

/// macOS write-probe: find a real .app in /Applications we don't own, try to
/// create+remove a witness file inside its Contents/. PermissionDenied → Missing;
/// success → Granted; nothing suitable to probe → NotApplicable (don't block).
#[cfg(target_os = "macos")]
fn probe_app_management() -> AppMgmtStatus {
    use std::io::ErrorKind;
    use std::os::unix::fs::MetadataExt;
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
        // ONLY probe ROOT-OWNED bundles. A user-owned app is always writable by
        // its owner regardless of App Management, so writing there proves nothing
        // (the false-Granted bug: with the permission denied, a user-owned app
        // still accepts the write). App Management gates modifying apps you don't
        // own — root-owned bundles (pkg/self-updater installed, e.g. VS Code) are
        // exactly that protected set.
        match std::fs::metadata(&contents) {
            Ok(m) if m.uid() == 0 => {}
            _ => continue, // not root-owned (or unreadable) → not a valid probe target
        }
        let witness = contents.join(".talos-appmgmt-probe");
        match std::fs::File::create(&witness) {
            Ok(_) => {
                let _ = std::fs::remove_file(&witness);
                return AppMgmtStatus::Granted;
            }
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                return AppMgmtStatus::Missing;
            }
            Err(_) => continue,
        }
    }
    // No root-owned bundle to probe → we can't constate; don't block.
    AppMgmtStatus::NotApplicable
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

// Windows PATH refresh: an install writes the registry but does NOT propagate the PATH
// to already-running processes → a freshly installed tool would read "absent" without this.
const WIN_PATH_REFRESH: &str =
    "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

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

/// Builds the POSIX Probe for a given shell (PURE — the shell path is injected).
/// zsh/bash → `-ilc` (interactive + login): sources /etc/profile (system PATH via
/// path_helper: /opt/homebrew) AND the user rc (.zshrc/.bashrc: ~/.local/bin,
/// where claude, uv, pip --user… live). The Deno fix `/bin/sh -lc` captured ONLY the
/// system PATH — hence the false "absent" on a tool installed in ~/.local/bin.
/// A bare /bin/sh (neither zsh nor bash) → `-lc` alone (sh does not read the zsh/bash rc).
fn posix_probe(shell: &str, command: &str) -> Probe {
    let is_rc_shell = shell.ends_with("zsh") || shell.ends_with("bash");
    let flags = if is_rc_shell { "-ilc" } else { "-lc" };
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
        _ => posix_probe(&user_shell(), command),
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
        _ => posix_probe(&user_shell(), command),
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
        // zsh/bash → -ilc: sources the user rc (.zshrc) → sees ~/.local/bin (claude, uv).
        let p = posix_probe("/bin/zsh", "claude --version");
        assert_eq!(p.cmd, "/bin/zsh");
        assert_eq!(p.args, vec!["-ilc", "claude --version"]);
        let b = posix_probe("/opt/homebrew/bin/bash", "node --version");
        assert_eq!(b.args[0], "-ilc");
    }

    #[test]
    fn posix_probe_sh_login_only() {
        // bare /bin/sh does not read the zsh/bash rc → -lc alone (not -ilc, useless).
        let p = posix_probe("/bin/sh", "node --version");
        assert_eq!(p.cmd, "/bin/sh");
        assert_eq!(p.args, vec!["-lc", "node --version"]);
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
