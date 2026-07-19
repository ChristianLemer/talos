//! agent_content.rs — détection du CONTENU agent présent : plugins Claude Code
//! (route `claude-plugin`) et skills standalone (route `skill`). Port Rust de
//! agent-content.ts, mais NATIF : lecture disque + serde_json, JAMAIS de shell-out
//! vers `claude`/`npx` (décision archi : tout en JSON parsé nativement — voir
//! mémoire talos-parsing-native-json). Parsing PUR et défensif (un fichier corrompu
//! ne "voit" rien de présent — direction sûre : jamais présent si incertain). L'IO
//! (lire les chemins réels) est une coquille fine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Un plugin installé, tel que lu dans installed_plugins.json.
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub id: String,      // ex. "chiron@tekton"
    pub version: String, // ex. "0.0.2" ("" si inconnue)
}

/// Parse ~/.claude/plugins/installed_plugins.json → liste de plugins. Le format :
/// `{ "version": 2, "plugins": { "<id>": [ { "version": "...", ... } ], ... } }`.
/// Défensif : JSON malformé → aucun plugin (jamais "présent" si incertain).
pub fn parse_installed_plugins(json: &str) -> Vec<Plugin> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(map) = v.get("plugins").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, entries) in map {
        // La valeur est un tableau d'installations ; on prend la version de la 1re.
        let version = entries
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e.get("version"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        out.push(Plugin { id: id.clone(), version });
    }
    out
}

/// Un plugin est-il présent ? `detect` peut être l'id complet "plugin@marketplace"
/// ou juste la partie "plugin" avant @ (ce qu'un bundle déclare souvent). Retourne
/// la version trouvée (Some), ou None si absent. Port de pluginPresent (+ version).
pub fn plugin_version(detect: &str, plugins: &[Plugin]) -> Option<String> {
    plugins
        .iter()
        .find(|p| p.id == detect || p.id.split('@').next() == Some(detect))
        .map(|p| p.version.clone())
}

/// Les noms de skills installées : chaque sous-dossier des répertoires de skills est
/// une skill (ex. ~/.claude/skills/<name>/, ~/.agents/skills/<name>/). Lecture disque
/// pure (pas de `npx skills list`). Dédupliqué (une skill peut être dans 2 racines).
pub fn list_skills(dirs: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue; // dossier absent → rien, pas d'erreur
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    out.entry(name.to_string()).or_insert_with(|| entry.path());
                }
            }
        }
    }
    out
}

/// Une skill nommée est-elle présente ? Retourne le chemin trouvé (preuve), ou None.
pub fn skill_path(name: &str, skills: &BTreeMap<String, PathBuf>) -> Option<PathBuf> {
    skills.get(name).cloned()
}

/// Chemins disque où lire le contenu agent, dérivés du HOME. Cross-platform via HOME
/// (Mac/Linux) / USERPROFILE (Windows). Le .app/.exe tourne dans le HOME de l'user.
pub fn plugins_json_path(home: &Path) -> PathBuf {
    home.join(".claude").join("plugins").join("installed_plugins.json")
}
pub fn skill_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".claude").join("skills"),
        home.join(".agents").join("skills"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "version": 2,
        "plugins": {
            "chiron@tekton": [{ "version": "0.0.2", "scope": "user" }],
            "superpowers@superpowers-marketplace": [{ "version": "5.1.0" }],
            "github@claude-plugins-official": [{ "version": "unknown" }]
        }
    }"#;

    #[test]
    fn parse_trouve_les_plugins_et_versions() {
        let ps = parse_installed_plugins(SAMPLE);
        assert_eq!(ps.len(), 3);
        assert_eq!(plugin_version("chiron@tekton", &ps), Some("0.0.2".into()));
    }

    #[test]
    fn plugin_match_id_partiel_avant_arobase() {
        // Un bundle déclare souvent "chiron" (sans @marketplace).
        let ps = parse_installed_plugins(SAMPLE);
        assert_eq!(plugin_version("chiron", &ps), Some("0.0.2".into()));
        assert_eq!(plugin_version("superpowers", &ps), Some("5.1.0".into()));
        assert_eq!(plugin_version("absent", &ps), None);
    }

    #[test]
    fn parse_defensif_sur_garbage() {
        assert_eq!(parse_installed_plugins("not json").len(), 0);
        assert_eq!(parse_installed_plugins("{}").len(), 0);
        assert_eq!(parse_installed_plugins(r#"{"plugins":42}"#).len(), 0);
    }

    #[test]
    fn list_skills_lit_les_sous_dossiers() {
        // Monte une arbo temporaire : deux racines, une skill partagée.
        let base = std::env::temp_dir().join("talos-test-skills");
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("claude").join("skills");
        let b = base.join("agents").join("skills");
        std::fs::create_dir_all(a.join("herdr")).unwrap();
        std::fs::create_dir_all(a.join("homey-cli")).unwrap();
        std::fs::create_dir_all(b.join("herdr")).unwrap(); // doublon → dédupliqué
        std::fs::create_dir_all(b.join("gws-gmail")).unwrap();
        let skills = list_skills(&[a.clone(), b.clone()]);
        assert_eq!(skills.len(), 3, "herdr dédupliqué"); // herdr, homey-cli, gws-gmail
        assert!(skill_path("herdr", &skills).is_some());
        assert!(skill_path("gws-gmail", &skills).is_some());
        assert!(skill_path("absent", &skills).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn list_skills_dossier_absent_ne_panique_pas() {
        let skills = list_skills(&[PathBuf::from("/nonexistent/xyz/skills")]);
        assert_eq!(skills.len(), 0);
    }

    #[test]
    fn chemins_derives_du_home() {
        let home = Path::new("/Users/x");
        assert!(plugins_json_path(home).ends_with(".claude/plugins/installed_plugins.json"));
        let dirs = skill_dirs(home);
        assert!(dirs[0].ends_with(".claude/skills"));
        assert!(dirs[1].ends_with(".agents/skills"));
    }
}
