//! consent.rs — le journal d'installation + le consentement "partager", local machine.
//! Port de src/consent.ts.
//!
//! Deux faits vivent dans le data-dir LOCAL par machine (%LOCALAPPDATA%\Talos sur
//! Windows, ~/Library/Application Support/Talos sur Mac — JAMAIS le dossier exe
//! partagé) : si l'utilisateur a consenti à PARTAGER son historique, et l'historique
//! lui-même (un objet JSON par ligne). Quand (et seulement quand) il consent, chaque
//! entrée est AUSSI recopiée dans un fichier partagé à côté de l'exe —
//! logs/<host>/<user>.jsonl — pour qu'une équipe voie qui a installé quoi. Pas de
//! consentement → la copie partagée n'est jamais écrite ; le journal local est gardé.
//!
//! Même discipline que detect.rs / outdated.rs : le PARSING est pur (testé), l'IO
//! est une coquille fine qui ne panique jamais (un journal qui ne peut écrire ne
//! doit pas couler un install).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Une entrée d'historique (une ligne JSONL).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistEntry {
    pub at: String,      // timestamp ISO
    pub package: String, // nom du paquet
    #[serde(default)]
    pub version: String, // capturé pour install/upgrade ; "" si inconnu
    #[serde(default)]
    pub action: String, // install | uninstall | upgrade
    #[serde(default)]
    pub ok: bool,
}

/// L'état de consentement.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Consent {
    pub decided: bool, // l'utilisateur a-t-il répondu à la question "partager" ?
    pub share: bool,   // si décidé : partage-t-il son historique ?
}

/// Parse un fichier JSONL → entrées. Ignore les lignes vides/malformées et tout
/// objet sans nom de paquet, pour qu'une seule ligne corrompue ne coule pas le log.
pub fn parse_history(raw: &str) -> Vec<HistEntry> {
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue; // pas du JSON → ligne ignorée, on garde le reste
        };
        let pkg = v.get("package").and_then(|p| p.as_str()).unwrap_or("");
        if pkg.is_empty() {
            continue;
        }
        out.push(HistEntry {
            at: v.get("at").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            package: pkg.to_string(),
            version: v.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            action: v.get("action").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            ok: v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
        });
    }
    out
}

/// Parse le fichier de consentement → {decided, share}. Absent ou garbage → NON
/// DÉCIDÉ (decided:false), ce qui fait afficher le dialogue de premier démarrage.
pub fn parse_consent(raw: &str) -> Consent {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(share) = v.get("share").and_then(|s| s.as_bool()) {
            return Consent { decided: true, share };
        }
    }
    Consent::default()
}

/// Où atterrit une copie consentie, à côté de l'exe : logs/<host>/<user>.jsonl.
/// Construction de chemin pure (le séparateur suit l'OS via PathBuf join).
pub fn shared_log_path(exe_dir: &Path, host: &str, user: &str) -> PathBuf {
    exe_dir.join("logs").join(host).join(format!("{user}.jsonl"))
}

/// Tout ce dont l'IO a besoin, injecté — pour que la coquille soit testable contre
/// des dossiers temporaires et que le serveur passe ses vrais chemins/identité.
#[derive(Debug, Clone)]
pub struct ConsentStore {
    pub local_dir: PathBuf, // data-dir LOCAL par machine (%LOCALAPPDATA%\Talos)
    pub exe_dir: PathBuf,   // le dossier partagé où l'exe siège (OneDrive)
    pub host: String,       // nom machine — namespace le log partagé
    pub user: String,       // nom user — le nom de fichier du log partagé
}

fn consent_path(s: &ConsentStore) -> PathBuf {
    s.local_dir.join("consent.json")
}
fn local_hist_path(s: &ConsentStore) -> PathBuf {
    s.local_dir.join("history.jsonl")
}

/// Lit l'état de consentement. Absent/illisible → non décidé (dialogue 1er boot).
pub fn read_consent(s: &ConsentStore) -> Consent {
    match std::fs::read_to_string(consent_path(s)) {
        Ok(raw) => parse_consent(&raw),
        Err(_) => Consent::default(),
    }
}

/// Enregistre le choix de partage (marque AUSSI le consentement DÉCIDÉ).
pub fn write_consent(s: &ConsentStore, share: bool) {
    let _ = std::fs::create_dir_all(&s.local_dir);
    if let Ok(json) = serde_json::to_string(&serde_json::json!({ "share": share })) {
        let _ = std::fs::write(consent_path(s), json);
    }
}

/// Lit l'historique d'installation local (l'ordre d'affichage est le job de l'UI).
pub fn read_history(s: &ConsentStore) -> Vec<HistEntry> {
    match std::fs::read_to_string(local_hist_path(s)) {
        Ok(raw) => parse_history(&raw),
        Err(_) => Vec::new(),
    }
}

/// Journalise UNE issue : TOUJOURS dans le fichier local ; et — seulement si
/// l'utilisateur a consenti à partager — AUSSI en append dans le fichier partagé à
/// côté de l'exe. Les deux best-effort : un journal qui ne peut écrire ne doit
/// jamais couler un install.
pub fn append_history(s: &ConsentStore, entry: &HistEntry) {
    use std::io::Write;
    let Ok(mut line) = serde_json::to_string(entry) else {
        return;
    };
    line.push('\n');
    // Local : toujours.
    let _ = std::fs::create_dir_all(&s.local_dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(local_hist_path(s))
    {
        let _ = f.write_all(line.as_bytes());
    }
    // Partagé : seulement si consenti (le dossier partagé peut être hors-ligne →
    // la copie locale reste gardée quoi qu'il arrive).
    if read_consent(s).share {
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        if let Some(parent) = shared.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&shared) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Efface l'historique LOCAL seulement. Le log d'équipe partagé est un enregistrement
/// dont d'autres dépendent — effacer sa propre vue ne doit pas effacer ce que
/// l'équipe a déjà vu.
pub fn clear_history(s: &ConsentStore) {
    let _ = std::fs::remove_file(local_hist_path(s));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> ConsentStore {
        ConsentStore {
            local_dir: dir.to_path_buf(),
            exe_dir: dir.join("shared"),
            host: "hostA".into(),
            user: "userB".into(),
        }
    }

    #[test]
    fn parse_history_ignore_lignes_corrompues() {
        let raw = "\n{bad json}\n{\"package\":\"node\",\"at\":\"t\",\"action\":\"install\",\"ok\":true}\n{\"no\":\"pkg\"}\n";
        let h = parse_history(raw);
        assert_eq!(h.len(), 1, "seule la ligne valide-avec-package survit");
        assert_eq!(h[0].package, "node");
        assert!(h[0].ok);
    }

    #[test]
    fn parse_consent_defensif() {
        assert_eq!(parse_consent("garbage"), Consent { decided: false, share: false });
        assert_eq!(parse_consent("{}"), Consent { decided: false, share: false });
        assert_eq!(parse_consent(r#"{"share":true}"#), Consent { decided: true, share: true });
        assert_eq!(parse_consent(r#"{"share":false}"#), Consent { decided: true, share: false });
    }

    #[test]
    fn shared_path_namespace_host_user() {
        let p = shared_log_path(Path::new("/exe"), "H", "U");
        assert_eq!(p, PathBuf::from("/exe/logs/H/U.jsonl"));
    }

    #[test]
    fn consent_roundtrip_marque_decided() {
        let dir = std::env::temp_dir().join("talos-test-consent-rt");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        assert_eq!(read_consent(&s), Consent::default(), "absent → non décidé");
        write_consent(&s, true);
        assert_eq!(read_consent(&s), Consent { decided: true, share: true });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_local_toujours_partage_seulement_si_consenti() {
        let dir = std::env::temp_dir().join("talos-test-consent-append");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        let e = HistEntry {
            at: "2026-07-19T12:00".into(),
            package: "bat".into(),
            version: "0.24".into(),
            action: "install".into(),
            ok: true,
        };
        // Pas encore de consentement → local écrit, partagé ABSENT.
        append_history(&s, &e);
        assert_eq!(read_history(&s).len(), 1);
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        assert!(!shared.exists(), "sans consentement, pas de copie partagée");
        // Consent share → la prochaine entrée est AUSSI partagée.
        write_consent(&s, true);
        append_history(&s, &e);
        assert_eq!(read_history(&s).len(), 2);
        assert!(shared.exists(), "avec consentement, copie partagée écrite");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_efface_local_pas_partage() {
        let dir = std::env::temp_dir().join("talos-test-consent-clear");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        write_consent(&s, true);
        let e = HistEntry {
            at: "t".into(),
            package: "bat".into(),
            version: "".into(),
            action: "install".into(),
            ok: true,
        };
        append_history(&s, &e);
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        assert!(shared.exists());
        clear_history(&s);
        assert_eq!(read_history(&s).len(), 0, "local vidé");
        assert!(shared.exists(), "le log partagé d'équipe reste intact");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
