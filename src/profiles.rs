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
    #[serde(default)]
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
            })
        })
        .collect();
    Profiles {
        columns: file.columns.unwrap_or(2),
        items,
    }
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
}
