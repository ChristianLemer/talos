//! vscode.rs — what VS Code extensions are installed, read NATIVELY from the profile
//! manifest (`~/.vscode/extensions/extensions.json`), never by shelling out to
//! `code --list-extensions`.
//!
//! Two reasons the CLI is not the source, both measured (see the spec):
//!   · the listing is polluted on Windows by a Node DeprecationWarning
//!     (microsoft/vscode#247315, still open) — a parser would have to skip noise;
//!   · it costs an Electron start (~0.2 s) per call, and the scan is serial on purpose.
//!
//! ⚠️ And the sub-directories are NOT the source either: after a successful uninstall the
//! extension's FOLDER survives on disk (recorded in a hidden `.obsolete` file) while the
//! manifest reads `[]`. Listing directories — the obvious implementation, and the one the
//! `skill` route uses for ~/.claude/skills — would report an uninstalled extension as present.
//!
//! ⚠️ NEVER WRITE this file. VS Code's `ExtensionsWatcher` watches it and calls
//! `deleteExtensionsNotInProfiles()` on a third-party edit, so a running instance would delete
//! folders it judges orphaned. All writes go through the CLI.

// TEMPORARY, and it covers the WHOLE module for exactly ONE reason: this is the read half,
// landed on its own, and the only callers so far are its tests. The detection branch that
// reaches it comes next; the gesture that lands it DELETES this line, and clippy then names
// anything that stayed unreached. Six per-item allows would say the same thing six times and
// each would have to be found again to be removed.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// An installed extension, as the profile manifest records it.
#[derive(Debug, Clone, PartialEq)]
pub struct Extension {
    /// `publisher.name`, exactly as `identifier.id` holds it.
    pub id: String,
    /// The installed version ("" if the entry omits it).
    pub version: String,
}

/// Where the profile keeps its extensions, in VS Code's own resolution order.
///
/// PURE: the two overrides arrive as arguments so the order is testable without touching the
/// process environment (a `set_var` in a test races every other test in the binary). Exactly
/// ONE caller reads the real environment and passes the values down — the same discipline that
/// keeps behaviour_io ignorant of environment variables.
///
/// ⚠️ `--extensions-dir` is deliberately absent: it is a per-invocation flag of a `code`
/// command we are not the ones running, so guessing it would be inventing a fact.
///
/// ⭐ ONE expression for every OS. VS Code derives the default as
/// `homedir() + dataFolderName + "extensions"` with no platform branch, so Windows'
/// `%USERPROFILE%\.vscode\extensions` is this same join — `user_home()` in detect.rs already
/// reads HOME then USERPROFILE.
pub fn extensions_dir(
    home: &Path,
    vscode_extensions: Option<String>,
    portable: Option<String>,
) -> PathBuf {
    let clean = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    if let Some(dir) = clean(vscode_extensions) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = clean(portable) {
        return PathBuf::from(dir).join("extensions");
    }
    home.join(".vscode").join("extensions")
}

/// The manifest inside an extensions directory.
pub fn manifest_path(extensions_dir: &Path) -> PathBuf {
    extensions_dir.join("extensions.json")
}

/// Parse the manifest text.
///
/// `Ok(vec![])` is a REAL answer — a profile with no extensions. `Err(())` means the text was
/// not understood, and the caller must present it as indeterminate.
///
/// ⭐ This is the one deliberate divergence from `agent_content::parse_installed_plugins`, which
/// returns `Vec::new()` on malformed JSON. The blast radius is the reason: a manifest carries
/// ~30 entries, so "unreadable" read as "nothing installed" would flip thirty rows to absent
/// and make Apply offer to install software that is already there — the exact defect
/// `managers::parse_presence` returns a Result to prevent.
///
/// ⚠️ The format is UNDOCUMENTED and self-migrating: VS Code rewrites the file when it lacks
/// `relativeLocation`. So only `identifier.id` and `version` are read, and nothing is derived
/// from `location` — both fields exist in both shapes.
#[allow(clippy::result_unit_err)] // recognised-or-not; there is no second cause to name
pub fn parse_installed_extensions(json: &str) -> Result<Vec<Extension>, ()> {
    let parsed: serde_json::Value = serde_json::from_str(json).map_err(|_| ())?;
    let arr = parsed.as_array().ok_or(())?;
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        // An entry without an id is skipped, not fatal: one malformed record must not blank a
        // whole machine, and the id is what every lookup keys on.
        let Some(id) = e
            .get("identifier")
            .and_then(|i| i.get("id"))
            .and_then(|x| x.as_str())
        else {
            continue;
        };
        out.push(Extension {
            id: id.to_string(),
            version: e
                .get("version")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(out)
}

/// Read the manifest from a real directory.
///
/// A MISSING file is `Ok(vec![])`, not an error: a profile that never installed an extension
/// has no manifest, and treating that as unreadable would leave a normal machine indeterminate
/// forever. Everything else — a manifest that is there but unreadable, or there and not
/// understood — is `Err`. The two halves of that distinction are pinned by a test each.
#[allow(clippy::result_unit_err)] // same shape as parse_installed_extensions
pub fn read_installed(extensions_dir: &Path) -> Result<Vec<Extension>, ()> {
    match std::fs::read_to_string(manifest_path(extensions_dir)) {
        Ok(text) => parse_installed_extensions(&text),
        // ⚠️ NotFound ALONE is the empty answer. Every other io error means the manifest is
        // THERE and we could not read it — a locked file, EACCES, a directory where a file
        // belongs, an I/O error on a cloud-backed profile — and answering "nothing installed"
        // to those is the exact defect this Result exists to prevent: thirty rows flipping to
        // absent, and Apply offering to install software that is already there.
        //
        // ⚠️ Deliberately UNLIKE timings.rs and behaviour_io.rs, which both collapse every io
        // error to empty. Those read CACHES, where empty is a good answer ("we know nothing
        // about this package yet"); this reads an OBSERVATION of the machine, where empty is a
        // false claim about it. Do not "align" this branch with theirs.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(()),
    }
}

/// The installed version of `id`, or None if the manifest does not list it.
///
/// ⚠️ EXACT match. A prefix comparison would make `ms-python` find `ms-python.python`, and an
/// extension id is `publisher.name` — prefixes collide by construction.
pub fn extension_version(id: &str, installed: &[Extension]) -> Option<String> {
    installed
        .iter()
        .find(|e| e.id == id)
        .map(|e| e.version.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/vscode")
            .join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("fixture {}: {e}", p.display()))
    }

    #[test]
    fn the_new_format_parses_and_the_lookup_is_exact() {
        let exts = parse_installed_extensions(&fixture("new-format.json")).expect("recognised");
        assert_eq!(exts.len(), 3);
        // ⭐ EXACT id match. The two ms-python entries share a publisher prefix precisely so
        // that a `starts_with` or `contains` implementation fails this line.
        assert_eq!(
            extension_version("ms-python.python", &exts),
            Some("2026.4.0".to_string())
        );
        assert_eq!(
            extension_version("ms-python.vscode-pylance", &exts),
            Some("2026.3.1".to_string())
        );
        assert_eq!(
            extension_version("ms-python", &exts),
            None,
            "a prefix is not an id"
        );
        assert_eq!(extension_version("nosuch.thing", &exts), None);
    }

    #[test]
    fn the_old_format_parses_too_because_relative_location_is_optional() {
        // VS Code's scanner reads `relativeLocation` when present and REWRITES the file when
        // it is not. So a profile that has not been touched by a recent build legitimately
        // lacks it, and treating that as corruption would blank a real machine.
        let exts = parse_installed_extensions(&fixture("old-format.json")).expect("recognised");
        assert_eq!(
            extension_version("tamasfe.even-better-toml", &exts),
            Some("0.21.2".to_string())
        );
    }

    #[test]
    fn an_empty_array_is_a_real_answer_not_a_failure() {
        // The state right after an uninstall. Ok(empty) ⇒ the row is honestly ABSENT and
        // actionable; Err would make it indeterminate forever.
        let exts = parse_installed_extensions(&fixture("empty.json")).expect("recognised");
        assert!(exts.is_empty());
    }

    #[test]
    fn a_truncated_manifest_is_an_error_never_an_empty_list() {
        // ⭐ THE load-bearing test. `parse_installed_plugins` returns Vec::new() on malformed
        // JSON; this must NOT, because the blast radius differs by an order of magnitude: a
        // manifest holds ~30 entries, so "unreadable read as nothing installed" would flip 30
        // rows to absent and make Apply offer to install what is already there. Same reason
        // `parse_presence` returns a Result.
        assert!(parse_installed_extensions(&fixture("truncated.json")).is_err());
    }

    #[test]
    fn a_missing_file_is_an_empty_profile_not_an_error() {
        // A profile that never installed an extension has NO manifest. Calling that
        // "unreadable" would leave a normal machine indeterminate forever.
        let dir = Path::new("/definitely/not/a/real/path/xyz");
        assert_eq!(read_installed(dir), Ok(Vec::new()));
    }

    #[test]
    fn a_manifest_that_exists_but_cannot_be_read_is_an_error_not_an_empty_profile() {
        // ⭐ The distinction NotFound draws. A missing manifest means "no extensions yet";
        // an UNREADABLE one means we know nothing — and answering "nothing installed" to the
        // second would flip every extension row to absent and make Apply offer to install
        // what is already there.
        //
        // A directory standing where extensions.json belongs is the portable way to provoke
        // a non-NotFound io error: no chmod (root ignores mode bits), no permissions games,
        // and it behaves the same on Windows.
        let tmp = std::env::temp_dir().join("talos-vscode-unreadable-manifest");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(manifest_path(&tmp)).expect("a directory where the file goes");
        assert_eq!(read_installed(&tmp), Err(()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_directory_resolution_follows_vscodes_own_order() {
        // VS Code resolves: --extensions-dir → VSCODE_EXTENSIONS → VSCODE_PORTABLE → HOME.
        // `--extensions-dir` is per-invocation and not ours to guess, so we honour the two
        // environment overrides and the default.
        //
        // ⚠️ The values are ARGUMENTS, never read from the environment here: std::env::set_var
        // in a test races every other test in the same binary. Same discipline as
        // TALOS_BEHAVIOUR, which server.rs reads once and passes down so that behaviour_io
        // "stays ignorant of environment variables".
        let home = PathBuf::from("/home/u");
        assert_eq!(
            extensions_dir(&home, Some("/central/exts".into()), None),
            PathBuf::from("/central/exts"),
            "VSCODE_EXTENSIONS wins — documented for centrally-managed enterprise estates"
        );
        assert_eq!(
            extensions_dir(&home, None, Some("/portable".into())),
            PathBuf::from("/portable/extensions")
        );
        assert_eq!(
            extensions_dir(
                &home,
                Some("/central/exts".into()),
                Some("/portable".into())
            ),
            PathBuf::from("/central/exts"),
            "VSCODE_EXTENSIONS outranks VSCODE_PORTABLE"
        );
        // ⭐ The default is ONE expression for Mac AND Windows: VS Code derives it as
        // homedir() + dataFolderName + "extensions" with no platform branch, so
        // %USERPROFILE%\.vscode\extensions is this same join.
        assert_eq!(
            extensions_dir(&home, None, None),
            PathBuf::from("/home/u/.vscode/extensions")
        );
    }

    #[test]
    fn an_empty_env_override_is_ignored_not_obeyed() {
        // An exported-but-empty VSCODE_EXTENSIONS must not resolve the manifest to "/extensions".
        let home = PathBuf::from("/home/u");
        assert_eq!(
            extensions_dir(&home, Some("".into()), Some("  ".into())),
            PathBuf::from("/home/u/.vscode/extensions")
        );
    }
}
