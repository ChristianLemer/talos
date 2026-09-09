//! check.rs — `Talos --check`: read the content the way the server would, and REFUSE what
//! the server tolerates.
//!
//! The runtime is deliberately lenient: a bad catalog file is skipped, an unknown name in a
//! bundle is ignored, an invalid `version:` word is logged and dropped — one broken row must
//! never take the screen down for the person in front of it. That leniency is right at
//! runtime and wrong at authoring time, where a silently skipped file is a package that
//! quietly stopped existing. This module is the authoring-time reading: the same parsers,
//! every finding named by file, and a non-zero exit so a pipeline can refuse the push.
//!
//! Pure over two directories (disk read + YAML) — no pty, no network, no window — so it
//! runs on a CI runner and in a unit test alike.

use std::collections::BTreeMap;
use std::path::Path;

use crate::bundles::{classify_pin, PinKind};

/// What `--check` found. `errors` is the list a pipeline reads; the counts are the
/// reassurance a human reads when the list is empty.
#[derive(Debug, Default)]
pub struct Report {
    pub packages: usize,
    pub bundles: usize,
    pub errors: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// One finding per line, then the summary. The summary comes LAST so the exit code and
    /// the final line agree, whichever of the two a reader looks at.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for e in &self.errors {
            out.push_str(e);
            out.push('\n');
        }
        out.push_str(&format!(
            "{} packages, {} bundles, {} error{}\n",
            self.packages,
            self.bundles,
            self.errors.len(),
            if self.errors.len() == 1 { "" } else { "s" }
        ));
        out
    }
}

/// The `*.yaml` files of a directory, sorted, so findings come out in a stable order.
/// Sidecars (`.nu`) and the README are not content and are not read.
fn yaml_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn file_label(dir: &Path, path: &Path) -> String {
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("content");
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    format!("{dir_name}/{file}")
}

pub fn run(catalog_dir: &Path, bundles_dir: &Path) -> Report {
    let mut r = Report::default();

    // ── catalog ────────────────────────────────────────────────────────────────────────
    if !catalog_dir.is_dir() {
        r.errors.push(format!(
            "catalog directory not found: {}",
            catalog_dir.display()
        ));
    }
    // id → file, name → file: the two keys a package is reached by, each unique.
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    // (file, requires) — resolved after every name is known, so order on disk is irrelevant.
    let mut requires: Vec<(String, Vec<String>)> = Vec::new();
    for path in yaml_files(catalog_dir) {
        let label = file_label(catalog_dir, &path);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                r.errors.push(format!("{label}: unreadable — {e}"));
                continue;
            }
        };
        // A key the engine does not know is a FIELD THAT DOES NOTHING: serde ignores it,
        // the runtime never sees it, and the author believes it took. `requries:`,
        // `npm-flags:` for `npmFlags:`, a field from a future release on an older
        // engine — all read as "fine" without this. The names come from the struct.
        if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
            for k in m.keys() {
                let key = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    serde_yaml::Value::Number(n) => n.to_string(),
                    other => format!("{other:?}"),
                };
                if !crate::bundles::RawPkg::KNOWN_KEYS.contains(&key.as_str()) && key != "id" {
                    r.errors.push(format!(
                        "{label}: unknown field `{key}` — the engine ignores it, so it does nothing"
                    ));
                }
            }
        }
        let cp = match crate::catalog::parse_catalog_entry_strict(&raw, stem) {
            Ok(cp) => cp,
            Err(e) => {
                r.errors.push(format!("{label}: {e}"));
                continue;
            }
        };
        r.packages += 1;
        if let Some(prev) = ids.insert(cp.id.clone(), label.clone()) {
            r.errors.push(format!(
                "{label}: id `{}` already declared by {prev}",
                cp.id
            ));
        }
        if let Some(prev) = names.insert(cp.pkg.name.clone(), label.clone()) {
            r.errors.push(format!(
                "{label}: name `{}` already declared by {prev} — bundles and `requires:` \
                 reach a package by name, so two packages with one name are one package",
                cp.pkg.name
            ));
        }
        if let Some(PinKind::Invalid(word)) = classify_pin(cp.pkg.version.as_deref()) {
            r.errors.push(format!(
                "{label}: version \"{word}\" is not a version, not `latest`, not `pending` \
                 — the runtime ignores it, which is not what you meant"
            ));
        }
        // `doctor:` is a capability the engine acts on, so its two inputs are checked
        // here rather than discovered at launch: a binary to resolve (the first word of
        // `detect:`) and, if a clean env var is declared, a name that IS a variable name.
        // And it is never a category — display must not carry behaviour.
        if let Some(d) = cp.pkg.doctor.as_ref() {
            let word = cp
                .pkg
                .detect
                .as_deref()
                .and_then(|s| s.split_whitespace().next());
            if word.is_none() {
                r.errors.push(format!(
                    "{label}: `doctor:` without a `detect:` — the Doctor launches the first \
                     word of detect, so there is nothing to launch"
                ));
            }
            if let Some(var) = d.clean.as_ref().and_then(|c| c.env.as_deref()) {
                let ok = !var.is_empty()
                    && !var.starts_with(|c: char| c.is_ascii_digit())
                    && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !ok {
                    r.errors.push(format!(
                        "{label}: doctor clean.env \"{var}\" is not an environment variable name"
                    ));
                }
            }
        }
        if cp
            .pkg
            .category
            .iter()
            .any(|c| c.eq_ignore_ascii_case("doctor"))
        {
            r.errors.push(format!(
                "{label}: `doctor` is a capability (the `doctor:` block), not a category — a \
                 category never decides a behaviour"
            ));
        }
        requires.push((label, cp.pkg.requires.clone()));
    }
    for (label, reqs) in requires {
        for req in reqs {
            if !names.contains_key(&req) {
                r.errors.push(format!(
                    "{label}: requires `{req}` — no package with that name in the catalog"
                ));
            }
        }
    }

    // ── bundles ────────────────────────────────────────────────────────────────────────
    if !bundles_dir.is_dir() {
        r.errors.push(format!(
            "bundles directory not found: {}",
            bundles_dir.display()
        ));
    }
    let mut bundle_names: BTreeMap<String, String> = BTreeMap::new();
    let mut needs: Vec<(String, Vec<String>)> = Vec::new();
    for path in yaml_files(bundles_dir) {
        let label = file_label(bundles_dir, &path);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem == "profiles" {
            // The legacy multi-card file: still loaded by the runtime, not checked here —
            // the current model is one file per bundle, and that is what a kit should ship.
            r.errors.push(format!(
                "{label}: legacy multi-bundle file — split it into one file per bundle"
            ));
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                r.errors.push(format!("{label}: unreadable — {e}"));
                continue;
            }
        };
        let profile = match crate::profiles::parse_one_bundle_strict(&raw) {
            Ok(Some(p)) => p,
            Ok(None) => {
                r.errors.push(format!(
                    "{label}: no `bundle:` name — the runtime would drop it"
                ));
                continue;
            }
            Err(e) => {
                r.errors.push(format!("{label}: {e}"));
                continue;
            }
        };
        r.bundles += 1;
        if let Some(prev) = bundle_names.insert(profile.name.clone(), label.clone()) {
            r.errors.push(format!(
                "{label}: bundle `{}` already declared by {prev}",
                profile.name
            ));
        }
        for pkg in &profile.packages {
            if !names.contains_key(pkg) {
                r.errors.push(format!(
                    "{label}: package `{pkg}` — no package with that name in the catalog"
                ));
            }
        }
        needs.push((label, profile.needs.clone()));
    }
    for (label, ns) in needs {
        for n in ns {
            if !bundle_names.contains_key(&n) {
                r.errors
                    .push(format!("{label}: needs `{n}` — no bundle with that name"));
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("talos-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("catalog")).unwrap();
        std::fs::create_dir_all(d.join("bundles")).unwrap();
        d
    }

    fn write(root: &Path, rel: &str, body: &str) {
        std::fs::write(root.join(rel), body).unwrap();
    }

    /// What ships must read clean: the socle at the repo root (what a release publishes)
    /// and the form-per-file fixture under tests/ — or `--check` would refuse the very
    /// content this repo demonstrates and tests with.
    #[test]
    fn the_shipped_content_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for (catalog, bundles, min_packages) in [
            (root.join("catalog"), root.join("bundles"), 4),
            (
                root.join("tests/fixtures/content/catalog"),
                root.join("tests/fixtures/content/bundles"),
                15,
            ),
        ] {
            let r = run(&catalog, &bundles);
            assert!(r.ok(), "{}: {}", catalog.display(), r.render());
            assert!(
                r.packages >= min_packages,
                "{}: {} packages",
                catalog.display(),
                r.packages
            );
            assert!(
                r.bundles >= 1,
                "{}: {} bundles",
                catalog.display(),
                r.bundles
            );
        }
    }

    /// The fixture is complete by construction: every key the engine accepts appears in
    /// at least one of its files. A field added to `RawPkg` without a form here fails
    /// this, not an integrator's `--check` months later.
    #[test]
    fn the_fixture_covers_every_known_key() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/content/catalog");
        let mut seen = std::collections::BTreeSet::new();
        for path in yaml_files(&dir) {
            let raw = std::fs::read_to_string(&path).unwrap();
            if let Ok(serde_yaml::Value::Mapping(m)) =
                serde_yaml::from_str::<serde_yaml::Value>(&raw)
            {
                for k in m.keys() {
                    if let serde_yaml::Value::String(s) = k {
                        seen.insert(s.clone());
                    } else if let serde_yaml::Value::Number(n) = k {
                        seen.insert(n.to_string());
                    }
                }
            }
        }
        let missing: Vec<&str> = crate::bundles::RawPkg::KNOWN_KEYS
            .iter()
            .copied()
            .filter(|k| !seen.contains(*k))
            .collect();
        assert!(
            missing.is_empty(),
            "keys with no form in the fixture: {missing:?}"
        );
    }

    #[test]
    fn a_broken_yaml_is_named_not_skipped() {
        let d = tmp("broken");
        write(&d, "catalog/good.yaml", "name: Good\nbrew: good\n");
        write(&d, "catalog/bad.yaml", "name: [unclosed\n");
        let r = run(&d.join("catalog"), &d.join("bundles"));
        assert_eq!(r.packages, 1);
        assert_eq!(r.errors.len(), 1, "{}", r.render());
        assert!(
            r.errors[0].starts_with("catalog/bad.yaml: "),
            "{}",
            r.errors[0]
        );
    }

    #[test]
    fn a_bundle_naming_an_unknown_package_is_an_error() {
        let d = tmp("unknown");
        write(&d, "catalog/git.yaml", "name: Git\nbrew: git\n");
        write(
            &d,
            "bundles/base.yaml",
            "bundle: Base\npackages:\n  - Git\n  - Ghost\n",
        );
        let r = run(&d.join("catalog"), &d.join("bundles"));
        assert_eq!(r.bundles, 1);
        assert_eq!(
            r.errors,
            vec!["bundles/base.yaml: package `Ghost` — no package with that name in the catalog"]
        );
    }

    #[test]
    fn requires_needs_and_names_are_cross_checked() {
        let d = tmp("cross");
        write(
            &d,
            "catalog/a.yaml",
            "name: A\nbrew: a\nrequires:\n  - B\n  - Nobody\n",
        );
        write(&d, "catalog/b.yaml", "name: B\nbrew: b\nversion: soon\n");
        write(&d, "catalog/b2.yaml", "name: B\nbrew: b2\n");
        write(&d, "bundles/x.yaml", "bundle: X\nneeds:\n  - Y\n");
        write(&d, "bundles/nameless.yaml", "emoji: 🎯\n");
        let r = run(&d.join("catalog"), &d.join("bundles"));
        let text = r.render();
        assert!(text.contains("catalog/a.yaml: requires `Nobody`"), "{text}");
        assert!(text.contains("catalog/b.yaml: version \"soon\""), "{text}");
        assert!(
            text.contains("catalog/b2.yaml: name `B` already declared by catalog/b.yaml"),
            "{text}"
        );
        assert!(text.contains("bundles/x.yaml: needs `Y`"), "{text}");
        assert!(
            text.contains("bundles/nameless.yaml: no `bundle:` name"),
            "{text}"
        );
        assert!(!r.ok());
    }

    #[test]
    fn an_unknown_field_is_named_not_ignored() {
        let d = tmp("unknown-key");
        write(
            &d,
            "catalog/typo.yaml",
            "name: Typo\nbrew: t\nrequries:\n  - Git\n",
        );
        write(
            &d,
            "catalog/camel.yaml",
            "name: Camel\nnpm: c\nnpm-flags: -g\n",
        );
        write(
            &d,
            "catalog/fine.yaml",
            "name: Fine\nbrew: f\nrunUninstall: rm\n\"403\": true\n",
        );
        let r = run(&d.join("catalog"), &d.join("bundles"));
        let text = r.render();
        assert!(
            text.contains("catalog/typo.yaml: unknown field `requries`"),
            "{text}"
        );
        assert!(
            text.contains("catalog/camel.yaml: unknown field `npm-flags`"),
            "{text}"
        );
        assert!(!text.contains("catalog/fine.yaml"), "{text}");
    }

    #[test]
    fn a_doctor_declaration_needs_a_binary_and_a_real_variable() {
        let d = tmp("doctor");
        write(
            &d,
            "catalog/ok.yaml",
            "name: Ok\nbrew: ok\ndetect: ok --version\ndoctor:\n  clean: { env: OK_HOME }\n",
        );
        write(
            &d,
            "catalog/blind.yaml",
            "name: Blind\nbrew: blind\ndoctor: {}\n",
        );
        write(
            &d,
            "catalog/badvar.yaml",
            "name: BadVar\nbrew: b\ndetect: b\ndoctor:\n  clean: { env: \"1 bad\" }\n",
        );
        write(
            &d,
            "catalog/cat.yaml",
            "name: Cat\nbrew: c\ndetect: c\ncategory: [Doctor]\n",
        );
        let r = run(&d.join("catalog"), &d.join("bundles"));
        let text = r.render();
        assert!(
            text.contains("catalog/blind.yaml: `doctor:` without a `detect:`"),
            "{text}"
        );
        assert!(
            text.contains("catalog/badvar.yaml: doctor clean.env \"1 bad\""),
            "{text}"
        );
        assert!(
            text.contains("catalog/cat.yaml: `doctor` is a capability"),
            "{text}"
        );
        assert!(!text.contains("catalog/ok.yaml"), "{text}");
    }

    #[test]
    fn a_missing_directory_is_the_first_finding() {
        let d = tmp("missing");
        let r = run(&d.join("nope"), &d.join("bundles"));
        assert!(
            r.errors[0].starts_with("catalog directory not found:"),
            "{}",
            r.render()
        );
    }
}
