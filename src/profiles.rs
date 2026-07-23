//! profiles.rs — load bundles/profiles.yaml (the top-panel "needs": named,
//! additive package selections). Pure (serde_yaml), no pty/network — same
//! defensive mould as bundles.rs: broken/absent YAML → empty, never panics.
//!
//! NOTE these are called "profiles" on disk & wire today; the bundles/categories
//! reframe renames them to "needs/bundles" in a later plan. Kept as-is here so
//! the frontend (which consumes `msg.profiles`) needs no change.

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
struct RawProfile {
    // A bundle file uses `bundle:`; the legacy profiles.yaml used `profile:`.
    // Accept both so one shape reads either.
    #[serde(default, alias = "bundle")]
    profile: Option<String>,
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    usage: Option<String>,
    #[serde(default)]
    highlights: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    packages: Vec<String>,
    // Other bundles this one depends on: activating it activates them too (real
    // cascade, spec §23). Empty for standalone bundles.
    #[serde(default)]
    needs: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawProfilesFile {
    #[serde(default)]
    columns: Option<i64>,
    #[serde(default)]
    profiles: Vec<RawProfile>,
}

/// One resolved profile (a "need"): a named, additive selection of packages.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub emoji: String,
    pub usage: String,
    pub highlights: Vec<String>,
    pub description: String,
    pub packages: Vec<String>,
    pub needs: Vec<String>,
}

/// The whole profiles panel: the cards + the grid column count.
#[derive(Debug, Clone, Default)]
pub struct Profiles {
    pub columns: i64,
    pub items: Vec<Profile>,
}

/// Pure parse of the profiles.yaml TEXT. Defensive: bad YAML → empty panel.
/// A profile with no `profile:` name is dropped (a nameless card is useless and
/// would collide on the frontend's name-keyed map).
pub fn parse_profiles(raw: &str) -> Profiles {
    let file: RawProfilesFile = match serde_yaml::from_str(raw) {
        Ok(f) => f,
        Err(_) => return Profiles::default(),
    };
    let items = file
        .profiles
        .into_iter()
        .filter_map(|p| {
            let name = p.profile?;
            Some(Profile {
                name,
                emoji: p.emoji.unwrap_or_else(|| "🎯".into()),
                usage: p.usage.unwrap_or_default(),
                highlights: p.highlights,
                description: p.description.unwrap_or_default(),
                packages: p.packages,
                needs: p.needs,
            })
        })
        .collect();
    Profiles {
        columns: file.columns.unwrap_or(2),
        items,
    }
}

/// Parse ONE bundle file (spec Consolidation §1: the 4 profiles are now one file
/// each). A bundle file is `{bundle:, emoji:, usage:, highlights:, description:,
/// packages: [names]}`. Returns None if it has no name (not a bundle card).
pub fn parse_one_bundle(raw: &str) -> Option<Profile> {
    let p: RawProfile = serde_yaml::from_str(raw).ok()?;
    let name = p.profile?;
    Some(Profile {
        name,
        emoji: p.emoji.unwrap_or_else(|| "🎯".into()),
        usage: p.usage.unwrap_or_default(),
        highlights: p.highlights,
        description: p.description.unwrap_or_default(),
        packages: p.packages,
        needs: p.needs,
    })
}

/// Load the bundle cards: every `*.yaml` under `dir` that names a bundle becomes
/// a card (spec Consolidation §1 — the 4 profiles are now 4 bundle files). Read
/// in sorted path order for a stable card order. Absent dir / bad file → skipped.
/// The legacy single `profiles.yaml` (if present) is also read via parse_profiles
/// for back-compat, but the current model ships one file per bundle.
pub fn load_profiles(dir: &str) -> Profiles {
    let mut items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect();
        paths.sort();
        for path in paths {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            if stem == "profiles" {
                // Legacy multi-profile file: expand it into cards too.
                items.extend(parse_profiles(&raw).items);
            } else if let Some(p) = parse_one_bundle(&raw) {
                items.push(p);
            }
        }
    }
    Profiles { columns: 2, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_columns_and_profiles() {
        let raw = r#"
columns: 3
profiles:
  - profile: Essential AI
    emoji: 🌱
    usage: Talk to an AI agent.
    highlights: [Claude Code, VS Code]
    description: The minimum.
    packages:
      - Node.js
      - Claude Code
"#;
        let p = parse_profiles(raw);
        assert_eq!(p.columns, 3);
        assert_eq!(p.items.len(), 1);
        let first = &p.items[0];
        assert_eq!(first.name, "Essential AI");
        assert_eq!(first.emoji, "🌱");
        assert_eq!(first.usage, "Talk to an AI agent.");
        assert_eq!(first.highlights, vec!["Claude Code", "VS Code"]);
        assert_eq!(first.packages, vec!["Node.js", "Claude Code"]);
    }

    #[test]
    fn columns_defaults_to_2_when_absent() {
        let raw = "profiles:\n  - profile: X\n    packages: [Git]\n";
        let p = parse_profiles(raw);
        assert_eq!(p.columns, 2);
        assert_eq!(p.items.len(), 1);
    }

    #[test]
    fn nameless_profile_dropped() {
        let raw = "profiles:\n  - emoji: 🎯\n    packages: [Git]\n";
        let p = parse_profiles(raw);
        assert!(p.items.is_empty());
    }

    #[test]
    fn garbage_yaml_yields_empty() {
        // Invalid YAML → Profiles::default() (columns = i64 default 0, items empty).
        // The columns value is moot: the frontend hides the panel when items is
        // empty, so it never reaches --cols. The meaningful guarantee is: no cards.
        assert!(parse_profiles("not: [valid").items.is_empty());
        assert_eq!(parse_profiles("not: [valid").columns, 0);
        assert!(parse_profiles("").items.is_empty());
    }

    #[test]
    fn load_reads_profiles_yaml_from_dir() {
        let dir = std::env::temp_dir().join("talos-test-profiles-load");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("profiles.yaml"),
            "columns: 2\nprofiles:\n  - profile: Dev\n    packages: [Git, Node.js]\n",
        )
        .unwrap();
        let p = load_profiles(dir.to_str().unwrap());
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].name, "Dev");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_absent_file_yields_empty() {
        let p = load_profiles("/nonexistent/talos/dir");
        assert!(p.items.is_empty());
        assert_eq!(p.columns, 2);
    }

    #[test]
    fn parse_one_bundle_reads_bundle_key() {
        let raw = "bundle: Essential AI\nemoji: 🌱\nusage: talk to AI\npackages: [Node.js, Claude Code]\n";
        let p = parse_one_bundle(raw).unwrap();
        assert_eq!(p.name, "Essential AI");
        assert_eq!(p.emoji, "🌱");
        assert_eq!(p.usage, "talk to AI");
        assert_eq!(p.packages, vec!["Node.js", "Claude Code"]);
    }

    #[test]
    fn parse_one_bundle_none_without_name() {
        assert!(parse_one_bundle("emoji: 🎯\npackages: [Git]\n").is_none());
    }

    #[test]
    fn parse_one_bundle_reads_needs() {
        let p = parse_one_bundle("bundle: Data\nneeds: [Documents]\npackages: [uv]\n").unwrap();
        assert_eq!(p.needs, vec!["Documents"]);
        // absent needs → empty
        let q = parse_one_bundle("bundle: Base\npackages: [Git]\n").unwrap();
        assert!(q.needs.is_empty());
    }

    #[test]
    fn load_reads_multiple_bundle_files_sorted() {
        let dir = std::env::temp_dir().join("talos-test-bundle-cards");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a-first.yaml"), "bundle: First\npackages: [Git]\n").unwrap();
        std::fs::write(
            dir.join("b-second.yaml"),
            "bundle: Second\npackages: [Node.js]\n",
        )
        .unwrap();
        std::fs::write(dir.join("README.md"), "ignored").unwrap();
        let p = load_profiles(dir.to_str().unwrap());
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].name, "First"); // sorted path order
        assert_eq!(p.items[1].name, "Second");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
