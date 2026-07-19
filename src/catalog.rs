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

/// Parse ONE catalog file's text, given its file stem (used as id when the file
/// declares no explicit `id:`). Returns None on unparseable YAML.
pub fn parse_catalog_entry(raw: &str, stem: &str) -> Option<CatalogPackage> {
    // A catalog file may carry an explicit `id:`; if absent, the file stem is the id.
    #[derive(serde::Deserialize)]
    struct IdProbe {
        #[serde(default)]
        id: Option<String>,
    }
    let pkg: RawPkg = serde_yaml::from_str(raw).ok()?;
    let id = serde_yaml::from_str::<IdProbe>(raw)
        .ok()
        .and_then(|p| p.id)
        .unwrap_or_else(|| stem.to_string());
    Some(CatalogPackage { id, pkg })
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
}
