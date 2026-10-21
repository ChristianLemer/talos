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
    // `default: on` — the card is active on a FIRST launch (no saved selection yet), and
    // pulls its `needs:` as a click would. Read as a YAML value, not a bool: `on` is a
    // string to YAML 1.2, and a misspelt value must leave the card off, not drop it.
    #[serde(default)]
    default: Option<serde_yaml::Value>,
}

/// Every YAML key a bundle file may carry, as `--check` enforces it. Kept by hand beside
/// the struct for the reason `RawPkg::KNOWN_KEYS` is; `known_keys_match_the_struct` fails
/// when the two drift. `profile` is the legacy spelling of `bundle`.
pub const KNOWN_KEYS: &[&str] = &[
    "bundle",
    "profile",
    "emoji",
    "usage",
    "highlights",
    "description",
    "packages",
    "needs",
    "default",
];

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
    /// `default: on` — active on a first launch. See `default_active`.
    pub default_on: bool,
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
                default_on: is_on(&p.default),
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
    parse_one_bundle_strict(raw).ok().flatten()
}

/// The strict twin, for `--check`: a YAML error comes back with serde's message; a file
/// with no `bundle:` name comes back as `Ok(None)` — that is what makes a yaml under
/// bundles/ "not a card", and the check reports it by file name instead of dropping it.
pub fn parse_one_bundle_strict(raw: &str) -> Result<Option<Profile>, String> {
    let p: RawProfile = serde_yaml::from_str(raw).map_err(|e| e.to_string())?;
    Ok(p.profile.map(|name| Profile {
        name,
        emoji: p.emoji.unwrap_or_else(|| "🎯".into()),
        usage: p.usage.unwrap_or_default(),
        highlights: p.highlights,
        description: p.description.unwrap_or_default(),
        packages: p.packages,
        default_on: is_on(&p.default),
        needs: p.needs,
    }))
}

/// The cards active on a first launch: every `default: on` card plus, transitively, what
/// it `needs:`.
///
/// ⭐ The engine decides this, not the front: which cards are on is the user's intention,
/// and the intention is an engine rule (one rule, every caller). The front only restores
/// the list it is handed. A need naming no card is dropped; a cycle stops where it closes.
pub fn default_active(profiles: &Profiles) -> Vec<String> {
    fn pull(profiles: &Profiles, name: &str, on: &mut std::collections::BTreeSet<String>) {
        let Some(p) = profiles.items.iter().find(|p| p.name == name) else {
            return;
        };
        if on.insert(p.name.clone()) {
            for n in &p.needs {
                pull(profiles, n, on);
            }
        }
    }
    let mut on = std::collections::BTreeSet::new();
    for p in profiles.items.iter().filter(|p| p.default_on) {
        pull(profiles, &p.name, &mut on);
    }
    // Card order, not alphabetical: the list is shown and persisted as it comes.
    profiles
        .items
        .iter()
        .filter(|p| on.contains(&p.name))
        .map(|p| p.name.clone())
        .collect()
}

/// `default: on` and nothing else. Lenient by design — `--check` names any other value.
fn is_on(v: &Option<serde_yaml::Value>) -> bool {
    matches!(v, Some(serde_yaml::Value::String(s)) if s == "on")
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

    #[test]
    fn parse_one_bundle_reads_default_on() {
        let on = parse_one_bundle("bundle: Base\ndefault: on\npackages: [Git]\n").unwrap();
        assert!(on.default_on);
        let off = parse_one_bundle("bundle: Base\ndefault: off\npackages: [Git]\n").unwrap();
        assert!(!off.default_on);
        let absent = parse_one_bundle("bundle: Base\npackages: [Git]\n").unwrap();
        assert!(!absent.default_on);
    }

    /// The runtime is lenient, `--check` is strict: a value that is not `on` — `true`, `yes`,
    /// a list — leaves the card OFF, and never takes the card off the screen.
    #[test]
    fn a_misspelt_default_keeps_the_card_and_leaves_it_off() {
        for v in ["true", "yes", "[on]", "On"] {
            let p = parse_one_bundle(&format!("bundle: Base\ndefault: {v}\npackages: [Git]\n"))
                .unwrap_or_else(|| panic!("`default: {v}` dropped the card"));
            assert!(!p.default_on, "`default: {v}` read as on");
        }
    }

    fn card(name: &str, default_on: bool, needs: &[&str]) -> Profile {
        Profile {
            name: name.into(),
            emoji: String::new(),
            usage: String::new(),
            highlights: vec![],
            description: String::new(),
            packages: vec![],
            needs: needs.iter().map(|n| n.to_string()).collect(),
            default_on,
        }
    }

    /// A default-on card pulls its `needs:` as a click on it does: Data needs Base, so a
    /// first launch with Data on has Base on too. Card order, each name once.
    #[test]
    fn default_active_follows_needs_transitively() {
        let p = Profiles {
            columns: 2,
            items: vec![
                card("Base", false, &[]),
                card("Data", false, &["Base"]),
                card("Team", true, &["Data"]),
                card("Legacy", false, &[]),
                card("Terminal", true, &[]),
            ],
        };
        assert_eq!(default_active(&p), vec!["Base", "Data", "Team", "Terminal"]);
    }

    #[test]
    fn default_active_survives_a_cycle_and_an_unknown_need() {
        let p = Profiles {
            columns: 2,
            items: vec![card("A", true, &["B", "Ghost"]), card("B", false, &["A"])],
        };
        assert_eq!(default_active(&p), vec!["A", "B"]);
        assert!(default_active(&Profiles::default()).is_empty());
    }

    /// The hand-kept key list and the struct agree — the same guard as `RawPkg`'s: a YAML
    /// carrying every known key parses, and the list counts one entry per struct field plus
    /// the legacy alias (`profile` and `bundle` are one field, so only one goes in the YAML).
    #[test]
    fn known_keys_match_the_struct() {
        let yaml: String = KNOWN_KEYS
            .iter()
            .filter(|k| **k != "profile")
            .map(|k| match *k {
                "highlights" | "packages" | "needs" => format!("{k}: []\n"),
                _ => format!("{k}: x\n"),
            })
            .collect();
        let parsed: Result<RawProfile, _> = serde_yaml::from_str(&yaml);
        assert!(parsed.is_ok(), "{parsed:?}");
        let fields = format!("{:?}", RawProfile::default()).matches(": ").count();
        assert_eq!(
            fields + 1,
            KNOWN_KEYS.len(),
            "RawProfile has {fields} fields"
        );
    }

    /// Every bundle key is named in the authoring reference, as `key:` — the bundle half of
    /// the guard `RawPkg` has. A field the reference does not name is a field nobody uses.
    #[test]
    fn every_bundle_key_is_documented_in_the_authoring_reference() {
        let reference =
            include_str!("../plugin/skills/talos-content/references/fields.md").replace('\r', "");
        let undocumented: Vec<&&str> = KNOWN_KEYS
            .iter()
            .filter(|k| !reference.contains(&format!("{k}:")))
            .collect();
        assert!(
            undocumented.is_empty(),
            "undocumented bundle keys: {undocumented:?}"
        );
    }
}
