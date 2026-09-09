//! catalog.rs — the FLAT package catalogue: one file per package under catalog/.
//! A catalog file is a package definition (same fields as a bundle's inline
//! package, i.e. bundles::RawPkg) plus a stable `id`. Bundles reference packages
//! by that id. Pure (serde_yaml), defensive: a bad file is skipped, never panics.

use crate::bundles::RawPkg;
use std::collections::BTreeMap;

/// A catalogue entry: its stable id + the raw package definition.
#[derive(Debug, Clone)]
pub struct CatalogPackage {
    pub id: String,
    pub pkg: RawPkg,
}

/// Parse ONE catalog file's text, given its file stem (used as id when the file declares
/// no explicit `id:`, or declares an empty one). Returns None on unparseable YAML.
pub fn parse_catalog_entry(raw: &str, stem: &str) -> Option<CatalogPackage> {
    // A catalog file may carry an explicit `id:`; if absent, the file stem is the id.
    #[derive(serde::Deserialize)]
    struct IdProbe {
        #[serde(default)]
        id: Option<String>,
    }
    let pkg: RawPkg = serde_yaml::from_str(raw).ok()?;
    // ⚠️ A BLANK `id:` falls back to the stem too, not just an absent one. The id is no
    // longer merely a map key: it NAMES a file (`behaviour/<id>.yaml`, beside
    // `catalog/<id>.yaml`), so an empty one would write `behaviour/.yaml` — a hidden file
    // on a shared folder that every package with a blank id would then overwrite in turn.
    // No shipped catalog file declares `id:` at all, so this is a guard, not a fix.
    let id = serde_yaml::from_str::<IdProbe>(raw)
        .ok()
        .and_then(|p| p.id)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| stem.to_string());
    Some(CatalogPackage { id, pkg })
}

/// The strict twin of `parse_catalog_entry`, for `--check`: the same reading, but a YAML
/// error comes back with serde's message instead of being swallowed. The runtime keeps
/// skipping a bad file (one broken row must not take the screen down); the check NAMES it.
pub fn parse_catalog_entry_strict(raw: &str, stem: &str) -> Result<CatalogPackage, String> {
    serde_yaml::from_str::<RawPkg>(raw).map_err(|e| e.to_string())?;
    parse_catalog_entry(raw, stem).ok_or_else(|| "unreadable".to_string())
}

/// Load every `*.yaml` under `dir` into an id→CatalogPackage map. Absent dir or
/// bad files → skipped. The map keeps deterministic order (BTreeMap) for stable
/// emission.
pub fn load_catalog(dir: &str) -> BTreeMap<String, CatalogPackage> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(cp) = parse_catalog_entry(&raw, stem) {
            out.insert(cp.id.clone(), cp);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_from_stem_when_absent() {
        let cp = parse_catalog_entry("name: Git\nbrew: git\n", "git").unwrap();
        assert_eq!(cp.id, "git");
        assert_eq!(cp.pkg.name, "Git");
    }

    #[test]
    fn explicit_id_wins_over_stem() {
        let cp = parse_catalog_entry("id: git-scm\nname: Git\nbrew: git\n", "git").unwrap();
        assert_eq!(cp.id, "git-scm");
    }

    #[test]
    fn a_blank_id_falls_back_to_the_stem() {
        // The id NAMES a file now (behaviour/<id>.yaml), so an empty one would write
        // `behaviour/.yaml` — hidden, and shared by every package with a blank id. An
        // explicit-but-empty `id:` is indistinguishable from a typo, and the stem is
        // always available, so the stem wins.
        for blank in [
            "id:\nname: Git\nbrew: git\n",
            "id: \"\"\nname: Git\n",
            "id: \"   \"\nname: Git\n",
        ] {
            let cp = parse_catalog_entry(blank, "git").unwrap();
            assert_eq!(
                cp.id, "git",
                "blank id must fall back to the stem: {blank:?}"
            );
        }
    }

    #[test]
    fn category_flows_through() {
        let cp = parse_catalog_entry("name: Git\nbrew: git\ncategory: [vcs, terminal]\n", "git")
            .unwrap();
        assert_eq!(cp.pkg.category, vec!["vcs", "terminal"]);
    }

    #[test]
    fn bad_yaml_is_none() {
        assert!(parse_catalog_entry("name: [unterminated", "x").is_none());
    }

    #[test]
    fn load_reads_dir_by_id() {
        let dir = std::env::temp_dir().join("talos-test-catalog-load");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("git.yaml"), "name: Git\nbrew: git\n").unwrap();
        std::fs::write(dir.join("node.yaml"), "name: Node.js\nbrew: node\n").unwrap();
        std::fs::write(dir.join("README.md"), "ignored").unwrap();
        let cat = load_catalog(dir.to_str().unwrap());
        assert_eq!(cat.len(), 2);
        assert_eq!(cat.get("git").unwrap().pkg.name, "Git");
        assert_eq!(cat.get("node").unwrap().pkg.name, "Node.js");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- The SHIPPED content, on both trees ----
    //
    // These read real YAML from disk. The Bun regression that broke plugin hooks on
    // Windows (2026-07-26) was invisible to every unit test, because every unit test built
    // its own fixture: a wrong PREMISE in a shipped YAML has no fixture to contradict it.
    // So assert on the real files — the socle at the repo root, what a release publishes,
    // and the form-per-file fixture under tests/, which must obey the same rules.

    fn trees() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"),
                concat!(env!("CARGO_MANIFEST_DIR"), "/bundles"),
            ),
            (
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/content/catalog"
                ),
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/content/bundles"
                ),
            ),
        ]
    }

    fn base_packages(bundles: &str) -> Vec<String> {
        #[derive(serde::Deserialize)]
        struct Bundle {
            #[serde(default)]
            packages: Vec<String>,
        }
        let raw = std::fs::read_to_string(format!("{bundles}/base.yaml"))
            .expect("bundles/base.yaml is there");
        serde_yaml::from_str::<Bundle>(&raw).unwrap().packages
    }

    /// Node.js must be IN the socle. Not because Talos needs it — because published
    /// Claude plugins run their hooks with a hardcoded `node`. Removing it once made
    /// the dependency implicit and undetected; this test is the guard against a
    /// second time. See catalog/node.yaml for the whole argument.
    #[test]
    fn the_base_bundle_ships_node() {
        for (_, bundles) in trees() {
            assert!(
                base_packages(bundles).iter().any(|p| p == "Node.js"),
                "{bundles}/base.yaml must keep Node.js — third-party plugin hooks invoke `node`"
            );
        }
    }

    /// Every `requires:` must name a package of the same tree. A dangling name does not
    /// fail loudly: requires_reason prints "requires X (unknown)" on a row and the user
    /// is left to guess.
    #[test]
    fn every_shipped_requirement_names_a_shipped_package() {
        for (catalog, _) in trees() {
            let cat = load_catalog(catalog);
            let names: Vec<&str> = cat.values().map(|c| c.pkg.name.as_str()).collect();
            for c in cat.values() {
                for req in &c.pkg.requires {
                    assert!(
                        names.contains(&req.as_str()),
                        "{catalog}: {} requires \"{}\" which no file declares",
                        c.pkg.name,
                        req
                    );
                }
            }
        }
    }

    /// Same for Base: a bundle that pulls a name nothing declares pulls nothing.
    #[test]
    fn every_base_package_is_in_the_catalog() {
        for (catalog, bundles) in trees() {
            let cat = load_catalog(catalog);
            let names: Vec<&str> = cat.values().map(|c| c.pkg.name.as_str()).collect();
            for p in base_packages(bundles) {
                assert!(
                    names.contains(&p.as_str()),
                    "{bundles}/base.yaml pulls \"{p}\" which no catalog file declares"
                );
            }
        }
    }

    /// A package installed through the Bun route must REQUIRE Bun. This is the
    /// shape of the bug, generalised: the route said one runtime, the machine had
    /// another, and nothing tied the two together. Route-agnostic on purpose — the
    /// npm half of the same rule lives with the npm route (see the shipped_ tests
    /// added alongside it), so reverting a routing decision cannot take this guard
    /// down with it.
    #[test]
    fn bun_routed_packages_require_bun() {
        for (catalog, _) in trees() {
            for c in load_catalog(catalog).values() {
                let p = &c.pkg;
                if p.bun.is_some() {
                    assert!(
                        p.requires.iter().any(|r| r == "Bun"),
                        "{catalog}: {} routes through bun but does not require Bun",
                        p.name
                    );
                }
            }
        }
    }
}
