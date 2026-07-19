// Port de platform.ts — Os + le wrapping shell (shell_probe / pty_shell).
// La source unique de "quel OS et comment agir dessus".

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Darwin,
    Linux,
}

/// Anything not windows/darwin → linux (le shell family qu'on supporte là). Ne panique jamais.
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

/// Data-dir LOCAL par machine — où atterrissent selection/consent/history. JAMAIS
/// le dossier exe partagé (OneDrive) : l'exe est lancé par N machines, donc tout
/// ce qui est écrit doit vivre sur le disque propre de chaque machine.
/// Windows → %LOCALAPPDATA%\Talos ; Mac → ~/Library/Application Support/Talos ;
/// Linux → $XDG_DATA_HOME/Talos (ou ~/.local/share/Talos). Port de localDataDir().
pub fn local_data_dir(os: Os) -> PathBuf {
    match os {
        Os::Windows => {
            let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| {
                let up = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
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
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

// PATH refresh Windows : un install écrit le registre mais NE propage PAS le PATH
// aux process déjà lancés → un outil frais lirait "absent" sans ça.
const WIN_PATH_REFRESH: &str =
    "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

/// Wrap une commande STRING en Probe dans le shell natif. Windows: garde 127 guard
/// (try/catch Stop → exit 127) — un CommandNotFoundException ne pose PAS $LASTEXITCODE,
/// donc "cmd; exit $LASTEXITCODE" lirait 0 (faux positif). POSIX: login shell -lc
/// (un .app lancé par Finder a un PATH minimal → -lc rebuild via path_helper).
pub fn shell_probe(os: Os, command: &str) -> Probe {
    match os {
        Os::Windows => {
            let ps = format!(
                "{WIN_PATH_REFRESH} $ErrorActionPreference='Stop'; try {{ {command}; exit $LASTEXITCODE }} catch {{ exit 127 }}"
            );
            Probe {
                cmd: "powershell.exe".into(),
                args: vec!["-NoProfile".into(), "-Command".into(), ps],
            }
        }
        _ => Probe {
            cmd: "/bin/sh".into(),
            args: vec!["-lc".into(), command.into()],
        },
    }
}

/// Wrap pour le pty INTERACTIF (install/upgrade/uninstall montrés live). Diffs vs
/// shell_probe : /bin/bash (POSIX), PAS de 127 guard (le pty veut le vrai exit code),
/// Windows garde le PATH refresh + exit $LASTEXITCODE.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn pty_shell(os: Os, command: &str) -> Probe {
    match os {
        Os::Windows => {
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
        _ => Probe {
            cmd: "/bin/bash".into(),
            args: vec!["-lc".into(), command.into()],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_shell_probe_login() {
        let p = shell_probe(Os::Darwin, "brew list");
        assert_eq!(p.cmd, "/bin/sh");
        assert_eq!(p.args, vec!["-lc", "brew list"]);
    }

    #[test]
    fn windows_shell_probe_has_127_guard() {
        let p = shell_probe(Os::Windows, "node --version");
        assert_eq!(p.cmd, "powershell.exe");
        assert!(p.args.last().unwrap().contains("exit 127"));
        assert!(p.args.last().unwrap().contains("node --version"));
    }

    #[test]
    fn pty_shell_posix_bash() {
        let p = pty_shell(Os::Darwin, "brew install jq");
        assert_eq!(p.cmd, "/bin/bash");
        assert_eq!(p.args, vec!["-lc", "brew install jq"]);
    }

    #[test]
    fn data_dir_termine_par_talos_par_os() {
        assert!(local_data_dir(Os::Darwin).ends_with("Library/Application Support/Talos"));
        assert!(local_data_dir(Os::Windows).ends_with("Talos"));
        assert!(local_data_dir(Os::Linux).ends_with("Talos"));
    }
}
