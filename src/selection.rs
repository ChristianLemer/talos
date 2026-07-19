//! selection.rs — l'INTENTION de l'utilisateur (les paquets qu'il a basculés in/out
//! à la main), persistée localement à la machine. Port de src/selection.ts.
//!
//! Distinct de la présence machine, toujours re-détectée live ("detect, don't
//! remember" gouverne la PRÉSENCE, PAS l'intention : un choix ne se re-observe pas,
//! donc il faut le mémoriser). Même discipline que consent.rs : le PARSING est pur
//! et défensif (un fichier corrompu ne coule jamais une session), la coquille IO
//! ne panique jamais. Stocké dans le data-dir LOCAL, JAMAIS le dossier exe partagé.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Intention persistée : clé stable "bundle::name" → bascule "in"/"out".
/// BTreeMap (ordre déterministe à la sérialisation → écriture stable, diffs propres).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub pkgs: BTreeMap<String, String>,
}

/// Pur, défensif : tout ce qui n'est pas un {pkgs:{clé:"in"|"out"}} bien formé → vide.
/// Ne garde que les valeurs exactement "in" ou "out" (comme le filtre du TS).
pub fn parse_selection(raw: &str) -> Selection {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Selection::default();
    };
    let mut pkgs = BTreeMap::new();
    if let Some(src) = v.get("pkgs").and_then(|p| p.as_object()) {
        for (k, val) in src {
            if let Some(s) = val.as_str() {
                if s == "in" || s == "out" {
                    pkgs.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    Selection { pkgs }
}

/// Chemin du fichier d'intention, dans le data-dir local.
fn selection_path(local_dir: &Path) -> PathBuf {
    local_dir.join("selection.json")
}

/// Lit l'intention persistée. Absent/illisible → vide (defaults de l'auteur).
pub fn read_selection(local_dir: &Path) -> Selection {
    match std::fs::read_to_string(selection_path(local_dir)) {
        Ok(raw) => parse_selection(&raw),
        Err(_) => Selection::default(),
    }
}

/// Persiste l'intention. Best-effort : une écriture ratée ne coule jamais une
/// session (au pire on re-repart des defaults de l'auteur au prochain démarrage).
pub fn write_selection(local_dir: &Path, sel: &Selection) {
    let _ = std::fs::create_dir_all(local_dir);
    if let Ok(json) = serde_json::to_string(sel) {
        let _ = std::fs::write(selection_path(local_dir), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_garde_seulement_in_out() {
        let sel = parse_selection(r#"{"pkgs":{"a::x":"in","b::y":"out","c::z":"maybe"}}"#);
        assert_eq!(sel.pkgs.get("a::x"), Some(&"in".to_string()));
        assert_eq!(sel.pkgs.get("b::y"), Some(&"out".to_string()));
        assert_eq!(sel.pkgs.get("c::z"), None, "valeur hors in/out rejetée");
    }

    #[test]
    fn parse_defensif_sur_garbage() {
        assert_eq!(parse_selection("pas du json").pkgs.len(), 0);
        assert_eq!(parse_selection("{}").pkgs.len(), 0);
        assert_eq!(parse_selection(r#"{"pkgs":42}"#).pkgs.len(), 0);
        assert_eq!(parse_selection(r#"{"pkgs":{"k":true}}"#).pkgs.len(), 0);
    }

    #[test]
    fn read_absent_est_vide() {
        let dir = std::env::temp_dir().join("talos-test-sel-absent");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_selection(&dir).pkgs.len(), 0);
    }

    #[test]
    fn write_puis_read_roundtrip() {
        let dir = std::env::temp_dir().join("talos-test-sel-rt");
        let _ = std::fs::remove_dir_all(&dir);
        let mut sel = Selection::default();
        sel.pkgs.insert("core::node".into(), "in".into());
        sel.pkgs.insert("extra::obsidian".into(), "out".into());
        write_selection(&dir, &sel);
        let back = read_selection(&dir);
        assert_eq!(back, sel);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
