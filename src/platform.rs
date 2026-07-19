// Port de platform.ts — Os + le wrapping shell (shell_probe / pty_shell).
// La source unique de "quel OS et comment agir dessus".

use std::path::PathBuf;
use std::process::Command;

/// Construit une `Command` qui NE FAIT PAS surgir de fenêtre console sous Windows.
/// Talos.exe est en subsystem GUI, mais chaque enfant lancé via `Command`
/// (powershell/winget au scan) crée SA PROPRE console — d'où la fenêtre noire qui
/// apparaît "plus tard", pendant le scan. CREATE_NO_WINDOW (0x0800_0000) la supprime.
/// Ailleurs (macOS/Linux) : un simple `Command::new`, le flag n'existe pas.
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

/// Le dossier "à côté de l'exe" — où vit `bundles/` (frontière hermétique : le moteur
/// change rarement, les bundles souvent, donc À CÔTÉ du binaire, lus au runtime, JAMAIS
/// scellés). Miroir Rust de `BUNDLES_DIR` du TS (dirname(execPath) en mode compilé).
///
/// Pièce macOS : dans un `.app`, l'exe est `Talos.app/Contents/MacOS/talos`. "À côté"
/// au sens hermétique = à côté du `.app` (modifiable sans toucher au bundle signé), pas
/// `Contents/MacOS/`. Donc si on détecte le motif `…/X.app/Contents/MacOS/<exe>`, on
/// remonte hors du `.app`. Sinon (binaire nu en dev, exe Windows/Linux) : le parent direct.
/// Fonction PURE (prend le chemin de l'exe) → testable sans lancer de process.
pub fn exe_sibling_dir(exe: &std::path::Path) -> PathBuf {
    let parent = exe.parent().unwrap_or(std::path::Path::new("."));
    // Motif .app : parent = ".../Contents/MacOS", grand-parent = ".../Contents",
    // arrière-grand-parent = ".../X.app" → on veut le dossier QUI CONTIENT X.app.
    if parent.file_name().is_some_and(|n| n == "MacOS") {
        if let Some(contents) = parent.parent() {
            if contents.file_name().is_some_and(|n| n == "Contents") {
                if let Some(app) = contents.parent() {
                    // app = ".../X.app" ; son parent = le dossier où poser bundles/.
                    if app.extension().is_some_and(|e| e == "app") {
                        return app.parent().unwrap_or(app).to_path_buf();
                    }
                }
            }
        }
    }
    parent.to_path_buf()
}

// PATH refresh Windows : un install écrit le registre mais NE propage PAS le PATH
// aux process déjà lancés → un outil frais lirait "absent" sans ça.
const WIN_PATH_REFRESH: &str =
    "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

/// Le shell POSIX de l'UTILISATEUR (résolu depuis $SHELL, fallback /bin/zsh puis
/// /bin/sh). "Ce que l'utilisateur voit dans son terminal" — c'est LUI qui connaît
/// les PATH custom de l'user (~/.local/bin, ajouté dans son .zshrc/.bashrc).
fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Fallbacks : zsh (défaut macOS moderne) s'il existe, sinon sh (toujours là).
            for cand in ["/bin/zsh", "/bin/bash"] {
                if std::path::Path::new(cand).exists() {
                    return Some(cand.to_string());
                }
            }
            None
        })
        .unwrap_or_else(|| "/bin/sh".into())
}

/// Construit le Probe POSIX pour un shell donné (PUR — le chemin du shell est injecté).
/// zsh/bash → `-ilc` (interactive + login) : source /etc/profile (PATH système via
/// path_helper : /opt/homebrew) ET le rc utilisateur (.zshrc/.bashrc : ~/.local/bin,
/// où vivent claude, uv, pip --user…). Le fix Deno `/bin/sh -lc` ne captait QUE le
/// PATH système — d'où le faux "absent" sur un outil installé en ~/.local/bin.
/// Un /bin/sh nu (ni zsh ni bash) → `-lc` seul (sh ne lit pas les rc zsh/bash).
fn posix_probe(shell: &str, command: &str) -> Probe {
    let is_rc_shell = shell.ends_with("zsh") || shell.ends_with("bash");
    let flags = if is_rc_shell { "-ilc" } else { "-lc" };
    Probe {
        cmd: shell.to_string(),
        args: vec![flags.into(), command.into()],
    }
}

/// PS 5.1 (le `powershell.exe` de Windows, chemin `v1.0`) ne connaît PAS `&&` — l'opérateur
/// n'existe qu'à partir de PS 7. Or les commandes d'install sont écrites avec `&&` (canonique
/// POSIX, exécuté tel quel sur Mac via `bash -lc`). On le traduit ici en chaîne PS équivalente
/// qui PRÉSERVE le court-circuit ET le code de sortie : chaque `&&` devient un garde qui sort
/// tôt si l'étape a échoué. `A && B && C` → `A; if ($LASTEXITCODE -ne 0){exit …}; B; …; C`.
/// Une commande sans `&&` traverse inchangée.
fn win_and_then(command: &str) -> String {
    command
        .split("&&")
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; ")
}

/// Wrap une commande STRING en Probe dans le shell natif. Windows: garde 127 guard
/// (try/catch Stop → exit 127) — un CommandNotFoundException ne pose PAS $LASTEXITCODE,
/// donc "cmd; exit $LASTEXITCODE" lirait 0 (faux positif). POSIX: le SHELL DE L'USER
/// en interactive+login (voir posix_probe) → voit le PATH système ET ~/.local/bin.
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

/// Wrap pour le pty INTERACTIF (install/upgrade/uninstall montrés live). Diffs vs
/// shell_probe : PAS de 127 guard (le pty veut le vrai exit code) ; Windows garde le
/// PATH refresh + exit $LASTEXITCODE. POSIX : MÊME shell user en interactive+login que
/// shell_probe — sinon on détecterait claude (~/.local/bin) mais on ne pourrait ni
/// l'installer ni le désinstaller (l'install aussi doit voir le PATH de l'user).
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
        // shell_probe résout le shell RÉEL de l'env ($SHELL) → on teste le CONTRAT,
        // pas un chemin fixe : un flag login (-lc ou -ilc) + la commande transmise.
        // (le choix -ilc vs -lc est couvert précisément par posix_probe, pur.)
        let p = shell_probe(Os::Darwin, "brew list");
        assert!(p.args[0] == "-lc" || p.args[0] == "-ilc", "flag login, got {}", p.args[0]);
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
        // `&&` (canonique POSIX) → garde PS 5.1 qui court-circuite sur échec.
        let out = win_and_then("cargo uninstall ripgrep && cargo install ripgrep");
        assert!(!out.contains("&&"), "il reste un && : {out}");
        assert!(out.contains("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"));
        assert!(out.starts_with("cargo uninstall ripgrep;"));
        assert!(out.ends_with("cargo install ripgrep"));
    }

    #[test]
    fn win_and_then_passes_through_simple_command() {
        // Sans && : inchangé (modulo trim), pas de garde parasite.
        assert_eq!(win_and_then("winget install Foo"), "winget install Foo");
    }

    #[test]
    fn windows_shell_probe_lowers_double_ampersand() {
        // Bout-en-bout : une commande `&&` ne doit JAMAIS atteindre powershell.exe telle
        // quelle (PS 5.1 : "The token '&&' is not a valid statement separator").
        let p = shell_probe(Os::Windows, "claude plugin marketplace add \"x\" && claude plugin install y --scope user");
        assert!(!p.args.last().unwrap().contains("&&"), "&& a fui dans PS : {:?}", p.args.last());
    }

    #[test]
    fn posix_probe_zsh_interactive_login() {
        // zsh/bash → -ilc : source le rc user (.zshrc) → voit ~/.local/bin (claude, uv).
        let p = posix_probe("/bin/zsh", "claude --version");
        assert_eq!(p.cmd, "/bin/zsh");
        assert_eq!(p.args, vec!["-ilc", "claude --version"]);
        let b = posix_probe("/opt/homebrew/bin/bash", "node --version");
        assert_eq!(b.args[0], "-ilc");
    }

    #[test]
    fn posix_probe_sh_login_only() {
        // /bin/sh nu ne lit pas les rc zsh/bash → -lc seul (pas -ilc, inutile).
        let p = posix_probe("/bin/sh", "node --version");
        assert_eq!(p.cmd, "/bin/sh");
        assert_eq!(p.args, vec!["-lc", "node --version"]);
    }

    #[test]
    fn data_dir_termine_par_talos_par_os() {
        assert!(local_data_dir(Os::Darwin).ends_with("Library/Application Support/Talos"));
        assert!(local_data_dir(Os::Windows).ends_with("Talos"));
        assert!(local_data_dir(Os::Linux).ends_with("Talos"));
    }

    #[test]
    fn sibling_app_remonte_hors_du_bundle() {
        // .app : bundles/ doit se poser À CÔTÉ du .app, pas dans Contents/MacOS.
        let exe = std::path::Path::new("/Apps/OneDrive/Talos.app/Contents/MacOS/talos");
        assert_eq!(exe_sibling_dir(exe), PathBuf::from("/Apps/OneDrive"));
    }

    #[test]
    fn sibling_binaire_nu_est_le_parent() {
        // Dev (cargo) ou exe Windows/Linux : juste le dossier parent.
        // (chemins POSIX : le test tourne sur Mac, où `\` n'est pas un séparateur.)
        let exe = std::path::Path::new("/home/x/talos/target/release/talos");
        assert_eq!(exe_sibling_dir(exe), PathBuf::from("/home/x/talos/target/release"));
        let shared = std::path::Path::new("/mnt/onedrive/Talos/talos");
        assert_eq!(exe_sibling_dir(shared), PathBuf::from("/mnt/onedrive/Talos"));
    }

    #[test]
    fn sibling_macos_sans_motif_app_reste_parent() {
        // Un dossier "MacOS" qui n'est PAS dans un .app → pas de remontée magique.
        let exe = std::path::Path::new("/random/MacOS/talos");
        assert_eq!(exe_sibling_dir(exe), PathBuf::from("/random/MacOS"));
    }
}
