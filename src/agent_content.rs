//! agent_content.rs — detection of the agent CONTENT present: Claude Code plugins
//! (route `claude-plugin`) and standalone skills (route `skill`). Rust port of
//! NATIVE: disk reads + serde_json, NEVER a shell-out
//! to `claude`/`npx` (archi decision: everything as natively-parsed JSON — see
//! memory talos-parsing-native-json). PURE and defensive parsing (a corrupted file
//! "sees" nothing present — safe direction: never present if uncertain). The IO
//! (reading the real paths) is a thin shell.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An installed plugin, as read from installed_plugins.json.
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub id: String,      // e.g. "chiron@tekton"
    pub version: String, // e.g. "0.0.2" ("" if unknown)
}

/// Parse ~/.claude/plugins/installed_plugins.json → list of plugins. The format:
/// `{ "version": 2, "plugins": { "<id>": [ { "version": "...", ... } ], ... } }`.
/// Defensive: malformed JSON → no plugin (never "present" if uncertain).
pub fn parse_installed_plugins(json: &str) -> Vec<Plugin> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(map) = v.get("plugins").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, entries) in map {
        // The value is an array of installations; we take the version of the 1st.
        let version = entries
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e.get("version"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        out.push(Plugin {
            id: id.clone(),
            version,
        });
    }
    out
}

/// Is a plugin present? `detect` can be the full id "plugin@marketplace"
/// or just the "plugin" part before @ (what a bundle often declares). Returns
/// the found version (Some), or None if absent. Port of pluginPresent (+ version).
pub fn plugin_version(detect: &str, plugins: &[Plugin]) -> Option<String> {
    plugins
        .iter()
        .find(|p| p.id == detect || p.id.split('@').next() == Some(detect))
        .map(|p| p.version.clone())
}

/// The names of installed skills: each subfolder of the skill directories is
/// a skill (e.g. ~/.claude/skills/<name>/, ~/.agents/skills/<name>/). Pure disk
/// read (no `npx skills list`). Deduplicated (a skill can be in 2 roots).
pub fn list_skills(dirs: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue; // folder absent → nothing, no error
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

/// Is a named skill present? Returns the found path (proof), or None.
pub fn skill_path(name: &str, skills: &BTreeMap<String, PathBuf>) -> Option<PathBuf> {
    skills.get(name).cloned()
}

/// Disk paths where to read the agent content, derived from HOME. Cross-platform via HOME
/// (Mac/Linux) / USERPROFILE (Windows). The .app/.exe runs in the user's HOME.
pub fn plugins_json_path(home: &Path) -> PathBuf {
    home.join(".claude")
        .join("plugins")
        .join("installed_plugins.json")
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
    fn parse_finds_plugins_and_versions() {
        let ps = parse_installed_plugins(SAMPLE);
        assert_eq!(ps.len(), 3);
        assert_eq!(plugin_version("chiron@tekton", &ps), Some("0.0.2".into()));
    }

    #[test]
    fn plugin_match_partial_id_before_at() {
        // A bundle often declares "chiron" (without @marketplace).
        let ps = parse_installed_plugins(SAMPLE);
        assert_eq!(plugin_version("chiron", &ps), Some("0.0.2".into()));
        assert_eq!(plugin_version("superpowers", &ps), Some("5.1.0".into()));
        assert_eq!(plugin_version("absent", &ps), None);
    }

    #[test]
    fn parse_defensive_on_garbage() {
        assert_eq!(parse_installed_plugins("not json").len(), 0);
        assert_eq!(parse_installed_plugins("{}").len(), 0);
        assert_eq!(parse_installed_plugins(r#"{"plugins":42}"#).len(), 0);
    }

    #[test]
    fn list_skills_lit_les_sous_dossiers() {
        // Sets up a temporary tree: two roots, one shared skill.
        let base = std::env::temp_dir().join("talos-test-skills");
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("claude").join("skills");
        let b = base.join("agents").join("skills");
        std::fs::create_dir_all(a.join("herdr")).unwrap();
        std::fs::create_dir_all(a.join("homey-cli")).unwrap();
        std::fs::create_dir_all(b.join("herdr")).unwrap(); // duplicate → deduplicated
        std::fs::create_dir_all(b.join("gws-gmail")).unwrap();
        let skills = list_skills(&[a.clone(), b.clone()]);
        assert_eq!(skills.len(), 3, "herdr deduplicated"); // herdr, homey-cli, gws-gmail
        assert!(skill_path("herdr", &skills).is_some());
        assert!(skill_path("gws-gmail", &skills).is_some());
        assert!(skill_path("absent", &skills).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn list_skills_missing_dir_does_not_panic() {
        let skills = list_skills(&[PathBuf::from("/nonexistent/xyz/skills")]);
        assert_eq!(skills.len(), 0);
    }

    #[test]
    fn paths_derived_from_home() {
        let home = Path::new("/Users/x");
        assert!(plugins_json_path(home).ends_with(".claude/plugins/installed_plugins.json"));
        let dirs = skill_dirs(home);
        assert!(dirs[0].ends_with(".claude/skills"));
        assert!(dirs[1].ends_with(".agents/skills"));
    }
}
