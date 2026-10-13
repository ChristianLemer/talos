//! distribution.rs — the NAME of the content half. `distribution.yaml` sits beside
//! `catalog/` and `bundles/` and says what this content is called, and optionally which
//! edition of it this is. The engine is one binary for everyone; a distribution is that
//! binary plus a content somebody chose, named and dated — which is what a kit is, and
//! what the person in front of the window should see first in its title.
//!
//! Optional in both directions: an engine older than this file never looks for it, and
//! an engine newer than a content without it shows the title it always did. Runtime
//! reading is lenient (a broken file is no name, never a crash); `--check` reads the same
//! file strictly and names the finding.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The file, beside `catalog/` and `bundles/`. A name, not a dot-file: an author edits it.
pub const FILE: &str = "distribution.yaml";

/// Every key the file may carry. Listed by hand, like `RawPkg::KNOWN_KEYS`, so the
/// reference guard can walk it and `--check` can refuse a typo by name.
pub const KNOWN_KEYS: &[&str] = &["name", "version"];

#[derive(Debug, Clone, PartialEq)]
pub struct Distribution {
    /// What the people in front of it call this content. Required.
    pub name: String,
    /// Which edition of it: a number, a date, a codename — free text, or nothing.
    pub version: Option<String>,
}

impl Distribution {
    /// `Name` or `Name version` — the human-facing label, used wherever the content is
    /// named: the title, the footer, the `--check` report.
    pub fn label(&self) -> String {
        match &self.version {
            Some(v) => format!("{} {}", self.name, v),
            None => self.name.clone(),
        }
    }

    /// The wire shape for the front: name, version (null when absent) and the label, so
    /// the front composes nothing it could get wrong.
    pub fn json(&self) -> Value {
        json!({ "name": self.name, "version": self.version, "label": self.label() })
    }
}

/// The native window title and the document title, from ONE rule. The distribution
/// leads — it is what a colleague recognises — and the engine's tag follows, for support.
/// Without a distribution the title is the one every release had, byte for byte.
pub fn window_title(dist: Option<&Distribution>, tag: &str) -> String {
    match dist {
        Some(d) => format!("{} · Talos {tag}", d.label()),
        None => format!("Talos · {tag}"),
    }
}

/// A scalar read as text: `version: 2026` is a number to YAML and a label to us.
fn scalar_text(v: &serde_yaml::Value) -> Option<String> {
    let s = match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Strict reading: the name is required and not blank, an unknown key is an error that
/// names the key. This is what `--check` reports; the runtime wraps it leniently.
pub fn parse(yaml: &str) -> Result<Distribution, String> {
    let v: serde_yaml::Value = serde_yaml::from_str(yaml).map_err(|e| e.to_string())?;
    let serde_yaml::Value::Mapping(m) = v else {
        return Err("expected a mapping with `name:` (and optionally `version:`)".into());
    };
    for k in m.keys() {
        let key = match k {
            serde_yaml::Value::String(s) => s.clone(),
            other => format!("{other:?}"),
        };
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "unknown field `{key}` — the file takes {}",
                KNOWN_KEYS
                    .iter()
                    .map(|k| format!("`{k}:`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let name = m
        .get(serde_yaml::Value::String("name".into()))
        .and_then(scalar_text)
        .ok_or_else(|| "`name:` is required and must not be blank".to_string())?;
    let version = m
        .get(serde_yaml::Value::String("version".into()))
        .and_then(scalar_text);
    Ok(Distribution { name, version })
}

/// Where the file lives, given the resolved `catalog/` directory: beside it. One rule
/// for the server, the window and `--check`, whichever of the three resolved the catalog.
pub fn path_beside(catalog_dir: &Path) -> PathBuf {
    catalog_dir
        .parent()
        .map(|p| p.join(FILE))
        .unwrap_or_else(|| PathBuf::from(FILE))
}

/// The runtime reading: absent → None, unreadable or broken → None. A content with no
/// name, or a typo in it, must never cost the person in front of the screen the window.
pub fn read_beside(catalog_dir: &Path) -> Option<Distribution> {
    let raw = std::fs::read_to_string(path_beside(catalog_dir)).ok()?;
    parse(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_and_a_version_parse() {
        let d = parse("name: Data management\nversion: 2026.10 — Saul\n").unwrap();
        assert_eq!(d.name, "Data management");
        assert_eq!(d.version.as_deref(), Some("2026.10 — Saul"));
    }

    #[test]
    fn the_version_is_optional_and_free_text() {
        let d = parse("name: Ops\n").unwrap();
        assert_eq!(d.version, None);
        // A number is text too: `2026` must not become 2026.0 or an integer.
        let d = parse("name: Ops\nversion: 2026\n").unwrap();
        assert_eq!(d.version.as_deref(), Some("2026"));
        // Blank is absent, not an empty string in the title.
        let d = parse("name: Ops\nversion: '  '\n").unwrap();
        assert_eq!(d.version, None);
    }

    #[test]
    fn the_name_is_required_and_not_blank() {
        let e = parse("version: 1\n").unwrap_err();
        assert!(e.contains("name"), "{e}");
        let e = parse("name: '  '\n").unwrap_err();
        assert!(e.contains("name"), "{e}");
    }

    #[test]
    fn an_unknown_key_is_refused_by_name() {
        // Strict, like a catalog file under `--check`: a typo must not load silently.
        let e = parse("name: Ops\nvesrion: 1\n").unwrap_err();
        assert!(e.contains("vesrion"), "{e}");
        assert!(
            KNOWN_KEYS.contains(&"name") && KNOWN_KEYS.contains(&"version"),
            "the reference guard walks KNOWN_KEYS"
        );
    }

    #[test]
    fn the_label_is_name_then_version() {
        let d = Distribution {
            name: "Data management".into(),
            version: Some("2026.10".into()),
        };
        assert_eq!(d.label(), "Data management 2026.10");
        let d = Distribution {
            name: "Ops".into(),
            version: None,
        };
        assert_eq!(d.label(), "Ops");
    }

    #[test]
    fn the_window_title_leads_with_the_distribution() {
        let d = Distribution {
            name: "Data management".into(),
            version: Some("2026.10".into()),
        };
        // The people in front of it recognise their distribution; the engine's version
        // follows, for support.
        assert_eq!(
            window_title(Some(&d), "v0.0.1-beta.44"),
            "Data management 2026.10 · Talos v0.0.1-beta.44"
        );
        // Without a distribution, the title it always had — byte for byte.
        assert_eq!(
            window_title(None, "v0.0.1-beta.44"),
            "Talos · v0.0.1-beta.44"
        );
    }

    #[test]
    fn read_beside_finds_the_file_next_to_the_catalog() {
        let root = std::env::temp_dir().join(format!("talos-dist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        // Absent → None, quietly: an older content has no such file.
        assert!(read_beside(&root.join("catalog")).is_none());
        std::fs::write(root.join(FILE), "name: Ops\nversion: 3\n").unwrap();
        let d = read_beside(&root.join("catalog")).unwrap();
        assert_eq!(d.label(), "Ops 3");
        // Broken → None at runtime: no name is better than no window.
        std::fs::write(root.join(FILE), "name: [unclosed\n").unwrap();
        assert!(read_beside(&root.join("catalog")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_wire_shape_is_name_and_version() {
        let d = Distribution {
            name: "Ops".into(),
            version: None,
        };
        let j = d.json();
        assert_eq!(j["name"], "Ops");
        assert!(j["version"].is_null(), "absent stays null, never \"\"");
        assert_eq!(j["label"], "Ops");
    }
}
