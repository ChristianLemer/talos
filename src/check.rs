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
    /// The label of `distribution.yaml` beside the catalog, when the file is there and
    /// clean. Reported, because the composing scripts run this check from an EMPTY
    /// directory: the name in the output is the proof the kit carried the file to where
    /// the engine reads it.
    pub distribution: Option<String>,
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
        if let Some(d) = &self.distribution {
            out.push_str(&format!("distribution: {d}\n"));
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

/// A key the engine does not know is a FIELD THAT DOES NOTHING: serde ignores it, the
/// runtime never sees it, and the author believes it took. `requries:`, `npm-flags:` for
/// `npmFlags:`, a field from a future release on an older engine — all read as "fine"
/// without this. `known` comes from the struct's hand-kept list.
fn unknown_keys(r: &mut Report, label: &str, raw: &str, known: impl Fn(&str) -> bool) {
    if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(raw) {
        for k in m.keys() {
            let key = match k {
                serde_yaml::Value::String(s) => s.clone(),
                serde_yaml::Value::Number(n) => n.to_string(),
                other => format!("{other:?}"),
            };
            if !known(&key) {
                r.errors.push(format!(
                    "{label}: unknown field `{key}` — the engine ignores it, so it does nothing"
                ));
            }
        }
    }
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
        unknown_keys(&mut r, &label, &raw, |k| {
            crate::bundles::RawPkg::KNOWN_KEYS.contains(&k) || k == "id"
        });
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
        let pin = classify_pin(cp.pkg.version.as_deref());
        if let Some(PinKind::Invalid(word)) = &pin {
            r.errors.push(format!(
                "{label}: version \"{word}\" is not a version, not `latest`, `pending` nor \
                 `self` — the runtime ignores it, which is not what you meant"
            ));
        }
        // `update:` is the tool's own update command, and it replaces the manager's upgrade
        // only for a package that updates itself. Anywhere else it would run never.
        if cp.pkg.update.is_some() && pin != Some(PinKind::SelfUpdating) {
            r.errors.push(format!(
                "{label}: `update:` runs only with `version: \"self\"` — here the manager \
                 upgrades the package and the command never runs"
            ));
        }
        // `update-on:` has one word, and nothing to run without `update:`.
        let each_apply = match cp.pkg.update_on.as_deref().map(str::trim) {
            None => false,
            Some(w) if w.eq_ignore_ascii_case("apply") => true,
            Some(w) => {
                r.errors.push(format!(
                    "{label}: `update-on: {w}` is not `apply` — the runtime ignores it and \
                     runs `update:` only when the tool is behind"
                ));
                false
            }
        };
        // Where an extension is the route that wins, the package carries no pin, so `self`
        // never applies and the route keeps its own update: the command is dropped there. Asked
        // per OS, of the route that runs — a manager or npm may win before it. Dropped on SOME
        // OSes is the layout working (like `brew:` on Windows); dropped on every one is dead.
        let extension_everywhere = [
            crate::platform::Os::Windows,
            crate::platform::Os::Darwin,
            crate::platform::Os::Linux,
        ]
        .into_iter()
        .all(|os| crate::bundles::is_extension_route_for(&cp.pkg, os));
        if cp.pkg.update.is_some() && extension_everywhere {
            r.errors.push(format!(
                "{label}: `update:` never runs — an extension is the route on every OS, and an \
                 extension carries no version, so `self` does not apply"
            ));
        }
        if cp.pkg.update_on.is_some() && cp.pkg.update.is_none() {
            r.errors.push(format!(
                "{label}: `update-on:` needs `update:` — there is no command to run"
            ));
        }
        // Without `update-on: apply`, it runs when the version the tool reports is behind what
        // winget or brew announces: both halves must exist, or the command is dead or runs for
        // nothing. On every Apply, neither is needed.
        if cp.pkg.update.is_some() && pin == Some(PinKind::SelfUpdating) && !each_apply {
            if cp.pkg.detect.is_none() {
                r.errors.push(format!(
                    "{label}: `update:` needs `detect:` — without it the version read is the \
                     manager's receipt, stale for a tool that updates itself, and the update \
                     would run for nothing"
                ));
            }
            if cp.pkg.winget.is_none() && cp.pkg.brew.is_none() {
                r.errors.push(format!(
                    "{label}: `update:` needs a `winget:` or `brew:` id — only their scan \
                     announces a newer version, so here the command never runs"
                ));
            }
        }
        // `scope:` says where winget puts the package. A word that is not one is no scope at
        // all at runtime, and on another route it means nothing — both are the author's
        // mistake, and both are said here rather than left to a machine to reveal.
        if let Some(w) = cp.pkg.scope.as_deref() {
            if crate::managers::InstallScope::from_word(w).is_none() {
                r.errors.push(format!(
                    "{label}: scope \"{w}\" is not `user` nor `machine` — winget would be \
                     given no scope, which is not what you meant"
                ));
            } else if cp.pkg.winget.is_none() {
                r.errors.push(format!(
                    "{label}: scope is a winget setting, and this package has no `winget:` id"
                ));
            }
        }
        // A DIRECT Copilot source installs a plugin that Copilot then knows by the name in the
        // plugin's own manifest — which the catalogue cannot derive, so the author says it.
        // And it registers no marketplace, so a `marketplace:` beside it is a misunderstanding.
        // Only where copilot-plugin IS the route: a package that also declares an earlier one
        // (claude-plugin, a manager) never runs this line, on any OS.
        let copilot_routed = [
            crate::platform::Os::Windows,
            crate::platform::Os::Darwin,
            crate::platform::Os::Linux,
        ]
        .into_iter()
        .any(|os| crate::bundles::route_for(&cp.pkg, os).as_deref() == Some("copilot-plugin"));
        if let Some(src) = cp.pkg.copilot_plugin.as_deref().filter(|_| copilot_routed) {
            // The route single-quotes every value; a `'` inside one ends the quoting early.
            let fields = [
                Some(src),
                cp.pkg.marketplace.as_deref(),
                cp.pkg.detect.as_deref(),
            ];
            if fields.iter().flatten().any(|v| v.contains('\'')) {
                r.errors.push(format!(
                    "{label}: a `'` in a copilot-plugin field — the route quotes every value \
                     with single quotes, and this one would end the quoting"
                ));
            }
            if crate::bundles::is_direct_copilot_source(src) {
                if cp.pkg.detect.as_deref().is_none_or(|d| d.trim().is_empty()) {
                    r.errors.push(format!(
                        "{label}: a direct `copilot-plugin:` needs `detect:` — the name \
                         `copilot plugin list` shows it under, which only its manifest knows"
                    ));
                }
                if cp.pkg.marketplace.is_some() {
                    r.errors.push(format!(
                        "{label}: `marketplace:` is never registered for a direct \
                         `copilot-plugin:` source — write `plugin@marketplace`, or drop it"
                    ));
                }
            }
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
        // The same guard as a catalog file's: `posture:` sat in an old README for months,
        // read by nothing, and this check let it through.
        unknown_keys(&mut r, &label, &raw, |k| {
            crate::profiles::KNOWN_KEYS.contains(&k)
        });
        // `default:` is `on` or `off`. The runtime reads any other value as off, leniently —
        // which is exactly why the strict reader must say so.
        if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
            if let Some(v) = m.get("default") {
                if !matches!(v, serde_yaml::Value::String(s) if s == "on" || s == "off") {
                    let shown = serde_yaml::to_string(v).unwrap_or_default();
                    r.errors.push(format!(
                        "{label}: `default: {}` — write `on` or `off`; anything else reads as off",
                        shown.trim()
                    ));
                }
            }
        }
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

    // ── distribution ───────────────────────────────────────────────────────────────────
    // Optional, and read STRICTLY here where the runtime reads leniently: a typo in the
    // one file that names the whole content must not load as "no name".
    let dist_path = crate::distribution::path_beside(catalog_dir);
    if dist_path.is_file() {
        match std::fs::read_to_string(&dist_path) {
            Err(e) => r
                .errors
                .push(format!("{}: unreadable — {e}", crate::distribution::FILE)),
            Ok(raw) => match crate::distribution::parse(&raw) {
                Ok(d) => r.distribution = Some(d.label()),
                Err(e) => r.errors.push(format!("{}: {e}", crate::distribution::FILE)),
            },
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

    /// What ships must read clean: the socle at the repo root (what a release publishes),
    /// the form-per-file fixture under tests/, and the plugin's scaffold template — or
    /// `--check` would refuse the very content this repo demonstrates, tests and hands out.
    ///
    /// ⭐ The template is here because `/talos:init` COPIES it rather than writing it. The
    /// first cut had the command retype the files from a prompt: every run produced slightly
    /// different bytes, nothing could be tested, and the first thing a stranger receives was
    /// the one artefact with no guard on it. Moving the files to disk made them ordinary
    /// content — and ordinary content is checked here, like the rest.
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
            (
                root.join("plugin/template/catalog"),
                root.join("plugin/template/bundles"),
                4,
            ),
        ] {
            let r = run(&catalog, &bundles);
            assert!(r.ok(), "{}: {}", catalog.display(), r.render());
            // Each tree is NAMED: the socle so a kit composed from it has a title, the
            // fixture so the form is exercised, the template so what `/talos:init` hands
            // out already carries the file an integrator is meant to edit.
            assert!(
                r.distribution.is_some(),
                "{}: no distribution.yaml beside the catalog",
                catalog.display()
            );
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

    /// The same completeness for bundles: every bundle key has a form in the fixture —
    /// `profile`, the legacy spelling of `bundle`, aside.
    #[test]
    fn the_fixture_covers_every_bundle_key() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/content/bundles");
        let mut seen = std::collections::BTreeSet::new();
        for path in yaml_files(&dir) {
            let raw = std::fs::read_to_string(&path).unwrap();
            if let Ok(serde_yaml::Value::Mapping(m)) =
                serde_yaml::from_str::<serde_yaml::Value>(&raw)
            {
                seen.extend(m.keys().filter_map(|k| k.as_str().map(str::to_string)));
            }
        }
        let missing: Vec<&str> = crate::profiles::KNOWN_KEYS
            .iter()
            .copied()
            .filter(|k| *k != "profile" && !seen.contains(*k))
            .collect();
        assert!(
            missing.is_empty(),
            "bundle keys with no form in the fixture: {missing:?}"
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
    fn a_scope_is_user_or_machine_and_only_on_winget() {
        let d = tmp("scope");
        write(
            &d,
            "catalog/a.yaml",
            "name: A\nwinget: A.A\nscope: everywhere\n",
        );
        write(&d, "catalog/b.yaml", "name: B\nbrew: b\nscope: user\n");
        write(
            &d,
            "catalog/c.yaml",
            "name: C\nwinget: C.C\nscope: machine\n",
        );
        let text = run(&d.join("catalog"), &d.join("bundles")).render();
        assert!(
            text.contains("catalog/a.yaml: scope \"everywhere\""),
            "{text}"
        );
        assert!(text.contains("catalog/b.yaml: scope"), "{text}");
        assert!(!text.contains("catalog/c.yaml"), "{text}");
    }

    #[test]
    fn a_direct_copilot_plugin_names_itself_and_registers_nothing() {
        let d = tmp("copilot");
        // Direct, no `detect:` — Copilot lists it by the name in ITS manifest, unknowable here.
        write(
            &d,
            "catalog/a.yaml",
            "name: A\ncopilot-plugin: o/r:plugins/a\n",
        );
        // Direct, with a `marketplace:` that the direct form never registers.
        write(
            &d,
            "catalog/b.yaml",
            "name: B\ncopilot-plugin: o/r\ndetect: b\nmarketplace: o/m\n",
        );
        // The two well-formed shapes.
        write(
            &d,
            "catalog/c.yaml",
            "name: C\ncopilot-plugin: o/r:p/c\ndetect: c\n",
        );
        write(
            &d,
            "catalog/e.yaml",
            "name: E\ncopilot-plugin: e@m\nmarketplace: o/m#beta\n",
        );
        let text = run(&d.join("catalog"), &d.join("bundles")).render();
        assert!(
            text.contains("catalog/a.yaml: a direct `copilot-plugin:` needs `detect:`"),
            "{text}"
        );
        assert!(
            text.contains("catalog/b.yaml: `marketplace:` is never registered"),
            "{text}"
        );
        assert!(!text.contains("catalog/c.yaml"), "{text}");
        assert!(!text.contains("catalog/e.yaml"), "{text}");
    }

    #[test]
    fn a_copilot_rule_holds_only_where_copilot_is_the_route() {
        let d = tmp("copilot-route");
        // claude-plugin comes first in the route table: the copilot line is never used, so
        // its missing `detect:` is no finding.
        write(
            &d,
            "catalog/a.yaml",
            "name: A\nclaude-plugin: a@m\ncopilot-plugin: o/r:p/a\n",
        );
        // A single quote would end the quoting the route relies on.
        write(
            &d,
            "catalog/b.yaml",
            "name: B\ncopilot-plugin: \"o/it's\"\ndetect: b\n",
        );
        let text = run(&d.join("catalog"), &d.join("bundles")).render();
        assert!(!text.contains("catalog/a.yaml"), "{text}");
        assert!(
            text.contains("catalog/b.yaml: a `'` in a copilot-plugin field"),
            "{text}"
        );
    }

    #[test]
    fn update_is_named_when_it_would_never_run() {
        let d = tmp("update");
        write(
            &d,
            "catalog/a.yaml",
            "name: A\nwinget: A.A\nupdate: a update\n",
        );
        write(
            &d,
            "catalog/b.yaml",
            "name: B\nwinget: B.B\ndetect: b --version\nversion: \"self\"\nupdate: b update\n",
        );
        write(
            &d,
            "catalog/c.yaml",
            "name: C\nwinget: C.C\nversion: self\n",
        );
        // Without `detect:` the version read is the manager's receipt — stale by construction
        // for a tool that rewrote itself — so the tool would read as behind on every scan.
        write(
            &d,
            "catalog/d.yaml",
            "name: D\nwinget: D.D\nversion: self\nupdate: d update\n",
        );
        // No winget nor brew id: nothing ever announces a newer version, so it never runs.
        write(
            &d,
            "catalog/e.yaml",
            "name: E\nnpm: e\ndetect: e --version\nversion: self\nupdate: e update\n",
        );
        let text = run(&d.join("catalog"), &d.join("bundles")).render();
        assert!(
            text.contains("catalog/a.yaml: `update:` runs only"),
            "{text}"
        );
        assert!(!text.contains("catalog/b.yaml"), "{text}");
        assert!(!text.contains("catalog/c.yaml"), "{text}");
        assert!(
            text.contains("catalog/d.yaml: `update:` needs `detect:`"),
            "{text}"
        );
        assert!(
            text.contains("catalog/e.yaml: `update:` needs a `winget:` or `brew:` id"),
            "{text}"
        );
    }

    #[test]
    fn update_on_is_named_when_it_means_nothing() {
        let d = tmp("update-on");
        write(
            &d,
            "catalog/a.yaml",
            "name: A\nwinget: A.A\nversion: self\nupdate-on: apply\n",
        );
        write(
            &d,
            "catalog/b.yaml",
            "name: B\nwinget: B.B\ndetect: b\nversion: self\nupdate: b update\nupdate-on: daily\n",
        );
        // On every Apply, no announcement is needed: no id, no `detect:` — an npm tool, or
        // one installed by mise, is the very case.
        write(
            &d,
            "catalog/c.yaml",
            "name: C\nnpm: c\nversion: self\nupdate: c update\nupdate-on: apply\n",
        );
        // A content route has no upgrade at all: the tool's command would never run.
        write(
            &d,
            "catalog/e.yaml",
            "name: E\nvscode-extension: e.e\nversion: self\nupdate: e update\nupdate-on: apply\n",
        );
        // npm wins over the skill on every OS: the command runs, no finding.
        write(
            &d,
            "catalog/f.yaml",
            "name: F\nnpm: f\nskill: o/f\nversion: self\nupdate: f update\nupdate-on: apply\n",
        );
        // brew wins where it is the system manager (macOS, Linux); on Windows the extension is
        // the route and keeps its own update. Not running where it cannot apply is no more an
        // error than `brew:` on Windows.
        write(
            &d,
            "catalog/g.yaml",
            "name: G\nbrew: g\nvscode-extension: g.g\ndetect: g\nversion: self\nupdate: g update\n",
        );
        // A released nu-plugin is an extension on every OS; a bundled one is not an extension.
        write(
            &d,
            "catalog/h.yaml",
            "name: H\nnu-plugin: h\nplugin-release: o/h@v1\nversion: self\nupdate: h update\nupdate-on: apply\n",
        );
        write(
            &d,
            "catalog/i.yaml",
            "name: I\nnu-plugin: i\nversion: self\nupdate: i update\nupdate-on: apply\n",
        );
        let text = run(&d.join("catalog"), &d.join("bundles")).render();
        assert!(
            text.contains("catalog/e.yaml: `update:` never runs — an extension is the route"),
            "{text}"
        );
        assert!(!text.contains("catalog/f.yaml"), "{text}");
        assert!(!text.contains("catalog/g.yaml"), "{text}");
        assert!(
            text.contains("catalog/h.yaml: `update:` never runs — an extension is the route"),
            "{text}"
        );
        assert!(
            !text.contains("catalog/i.yaml: `update:` never runs"),
            "{text}"
        );
        assert!(
            text.contains("catalog/a.yaml: `update-on:` needs `update:`"),
            "{text}"
        );
        assert!(
            text.contains("catalog/b.yaml: `update-on: daily` is not `apply`"),
            "{text}"
        );
        assert!(!text.contains("catalog/c.yaml"), "{text}");
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

    /// The bundle half of the same guard. `posture:` is the case that made it necessary: an
    /// old README promised it, the engine never read it, and `--check` said nothing.
    #[test]
    fn an_unknown_bundle_field_is_named_not_ignored() {
        let d = tmp("unknown-bundle-key");
        write(&d, "catalog/git.yaml", "name: Git\nbrew: git\n");
        write(
            &d,
            "bundles/base.yaml",
            "bundle: Base\nposture: opt-out\npackages: [Git]\n",
        );
        write(
            &d,
            "bundles/fine.yaml",
            "bundle: Fine\ndefault: on\nneeds: [Base]\npackages: [Git]\n",
        );
        let text = run(&d.join("catalog"), &d.join("bundles")).render();
        assert!(
            text.contains("bundles/base.yaml: unknown field `posture`"),
            "{text}"
        );
        assert!(!text.contains("bundles/fine.yaml"), "{text}");
    }

    /// `default:` takes `on` or `off`. The runtime reads anything else as off, so a `true`
    /// or a `yes` would ship a card the author believes is on.
    #[test]
    fn a_bundle_default_other_than_on_or_off_is_an_error() {
        let d = tmp("bundle-default");
        write(&d, "catalog/git.yaml", "name: Git\nbrew: git\n");
        write(
            &d,
            "bundles/a.yaml",
            "bundle: A\ndefault: true\npackages: [Git]\n",
        );
        write(
            &d,
            "bundles/b.yaml",
            "bundle: B\ndefault: yes\npackages: [Git]\n",
        );
        write(
            &d,
            "bundles/c.yaml",
            "bundle: C\ndefault: off\npackages: [Git]\n",
        );
        let r = run(&d.join("catalog"), &d.join("bundles"));
        let text = r.render();
        assert!(text.contains("bundles/a.yaml: `default: true`"), "{text}");
        assert!(text.contains("bundles/b.yaml: `default: yes`"), "{text}");
        assert!(!text.contains("bundles/c.yaml"), "{text}");
        assert_eq!(r.bundles, 3, "a bad default never drops the card");
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
    #[test]
    fn a_distribution_with_a_typo_is_an_error_named_by_file() {
        let d = tmp("dist-typo");
        write(&d, "catalog/good.yaml", "name: Good\nbrew: good\n");
        write(&d, "distribution.yaml", "name: Ops\nvesrion: 3\n");
        let r = run(&d.join("catalog"), &d.join("bundles"));
        assert_eq!(r.errors.len(), 1, "{}", r.render());
        assert!(
            r.errors[0].starts_with("distribution.yaml: ") && r.errors[0].contains("vesrion"),
            "{}",
            r.errors[0]
        );
        assert!(r.distribution.is_none(), "a refused file names nothing");
    }

    #[test]
    fn a_distribution_is_reported_before_the_summary() {
        let d = tmp("dist-ok");
        write(&d, "catalog/good.yaml", "name: Good\nbrew: good\n");
        write(&d, "distribution.yaml", "name: Ops\nversion: 3\n");
        let r = run(&d.join("catalog"), &d.join("bundles"));
        assert!(r.ok(), "{}", r.render());
        assert_eq!(r.distribution.as_deref(), Some("Ops 3"));
        let out = r.render();
        let lines: Vec<&str> = out.lines().collect();
        // The composing scripts run `--check` from an EMPTY directory: this line in the
        // output is the proof the kit carried the file to where the engine reads it.
        assert_eq!(lines[lines.len() - 2], "distribution: Ops 3", "{out}");
        // The summary stays LAST, so the exit code and the final line still agree.
        assert!(lines[lines.len() - 1].ends_with("0 errors"), "{out}");
    }

    #[test]
    fn no_distribution_is_not_a_finding() {
        let d = tmp("dist-none");
        write(&d, "catalog/good.yaml", "name: Good\nbrew: good\n");
        let r = run(&d.join("catalog"), &d.join("bundles"));
        assert!(r.ok(), "{}", r.render());
        assert!(r.distribution.is_none());
        assert!(!r.render().contains("distribution:"), "{}", r.render());
    }

    /// The same guard as for package keys, for the distribution file: the reference must
    /// name the file, and every key it takes, or the doc drifts from the engine.
    #[test]
    fn the_authoring_reference_covers_the_distribution_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let reference =
            std::fs::read_to_string(root.join("plugin/skills/talos-content/references/fields.md"))
                .unwrap();
        let at = reference
            .find(&format!("## `{}`", crate::distribution::FILE))
            .expect("fields.md has a section headed by the file name");
        let section = &reference[at..];
        for k in crate::distribution::KNOWN_KEYS {
            assert!(
                section.contains(&format!("`{k}:`")),
                "fields.md's distribution section does not mention `{k}:`"
            );
        }
    }
}
