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
/// ⚠️ EXACT id, but case-INSENSITIVELY. A prefix comparison would make `ms-python` find
/// `ms-python.python`, so `starts_with` is out — an id is `publisher.name` and prefixes collide
/// by construction. But the case must NOT be exact: MEASURED on a real profile, the manifest
/// lowercases every id (30/30) while the marketplace keeps the publisher's own casing —
/// `TheNuProjectContributors.vscode-nushell-lang` is stored `thenuprojectcontributors.…`, and
/// the extension's own package.json agrees with the marketplace, not the manifest. An author
/// copying the id off the marketplace page would otherwise get a false ABSENT — and a false
/// absent makes Apply install what is already there. The `code` CLI is itself case-insensitive
/// here (measured: an all-caps id matched the installed one), so exact casing would be stricter
/// than the host we model.
///
/// ⚠️ `eq_ignore_ascii_case`, not a `to_lowercase()` comparison: it allocates nothing, and
/// extension ids are ASCII by the marketplace's own naming rules.
pub fn extension_version(id: &str, installed: &[Extension]) -> Option<String> {
    installed
        .iter()
        .find(|e| e.id.eq_ignore_ascii_case(id))
        .map(|e| e.version.clone())
}

/// What one scan learned about the VS Code host, shared read-only across every row.
///
/// Built ONCE per scan beside the bulk presence listing, for the same reason that exists:
/// fewer processes, not more threads. Probing the host per row would cost one `code --version`
/// (~0.2 s) per extension on a scan that is serial on purpose, and would walk into
/// microsoft/vscode#302026 — where `code` resolved to `Code.exe`, the GUI, and OPENED A WINDOW
/// instead of printing — once per row rather than once per scan.
#[derive(Debug, Clone)]
pub struct VscodeSnapshot {
    /// Did the host answer? A shell-out, the only one this route makes.
    pub host_present: bool,
    /// The profile manifest, or the fact that it could not be read.
    pub installed: Result<Vec<Extension>, ()>,
}

/// What can honestly be said about one extension id.
#[derive(Debug, Clone, PartialEq)]
pub enum HostedVerdict {
    /// Listed in the manifest, host present. Carries the installed version.
    Present(String),
    /// Host present, manifest read, id not in it. The only ACTIONABLE verdict.
    Absent,
    /// No host ⇒ nothing can be claimed. The manifest outlives the editor.
    NoHost,
    /// The manifest is there and could not be read (unreadable OR not understood —
    /// `read_installed` collapses both into `Err(())`, so this layer cannot tell them apart
    /// and must not claim to).
    Unreadable,
}

impl VscodeSnapshot {
    /// The verdict for one id.
    ///
    /// ⭐ Order matters: the HOST is asked first. The rule is "not up to date if the tool is not
    /// there", and it is structural rather than pedantic, because `~/.vscode/extensions` is a
    /// USER folder that survives uninstalling VS Code itself. A manifest-first reading would
    /// paint a green row per extension for an editor that is gone.
    pub fn verdict(&self, id: &str) -> HostedVerdict {
        if !self.host_present {
            return HostedVerdict::NoHost;
        }
        match &self.installed {
            Err(()) => HostedVerdict::Unreadable,
            Ok(list) => match extension_version(id, list) {
                Some(v) => HostedVerdict::Present(v),
                None => HostedVerdict::Absent,
            },
        }
    }
}

/// The snapshot, from facts already gathered. PURE — no process, no environment.
///
/// Split out from `snapshot` so the half that CAN be tested is: the host verdict arrives as a
/// boolean, so both sides of "presence needs the host" are reachable without uninstalling VS Code,
/// and the directory arrives as an argument, so the read can be pointed at a fixture.
pub fn snapshot_from(host_present: bool, extensions_dir: &Path) -> VscodeSnapshot {
    VscodeSnapshot {
        host_present,
        installed: read_installed(extensions_dir),
    }
}

/// Build the snapshot for the machine we are running on: probe the host, resolve the directory
/// from the environment, then delegate to `snapshot_from`.
///
/// The IMPURE shim, and it holds everything untestable so that nothing else has to: one process
/// and two environment reads. Everything it decides afterwards lives in `snapshot_from`.
///
/// ⚠️ The host probe goes through `platform::shell_probe` — the ONE shell wrapping. Never a
/// hand-built command line (it would bypass the Windows PATH refresh, and the installer writes
/// `{app}\bin` to the registry PATH, which is exactly what that refresh picks up), and never
/// `std::process::Command::new("code")`: Rust does not apply PATHEXT, so it would not find
/// `code.cmd` on Windows at all.
///
/// ⚠️ NO TIMEOUT, because `run_probe_detailed` uses `.output()` and none of the probes have one.
/// Pre-existing and shared, but newly pointed at a command KNOWN to misbehave: this module's own
/// header cites microsoft/vscode#302026, where `code` resolved to the GUI. A `code --version`
/// that opens a window instead of printing would stall a scan that is serial on purpose — one
/// hang, the whole scan. Not fixed here (a timeout belongs to `run_probe_detailed`, where it
/// would change every probe in the app); named so the next reader does not have to rediscover it.
///
/// ⭐ The `#[allow(dead_code)]` this carried until the wiring landed is GONE, and its absence is
/// the proof the wiring is real: rustc treats an `allow(dead_code)` item as a LIVE ROOT, so that
/// one attribute re-livened the whole read half of this module (`extensions_dir`,
/// `manifest_path`, `parse_installed_extensions`, `read_installed`, `snapshot_from`) — tests do
/// not count, they are a separate compilation. Both scans in `server.rs` now call this, so the
/// chain is reached from the bin and clippy is silent without any allow. If a future refactor
/// unwires it, clippy will name the whole chain rather than one function — that is the signal.
pub fn snapshot(os: crate::platform::Os, home: &Path) -> VscodeSnapshot {
    let probe = crate::platform::shell_probe(os, "code --version");
    let d = crate::detect::run_probe_detailed(&probe);
    let dir = extensions_dir(
        home,
        std::env::var("VSCODE_EXTENSIONS").ok(),
        std::env::var("VSCODE_PORTABLE").ok(),
    );
    snapshot_from(d.ok, &dir)
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
        // a non-NotFound io error: no chmod (root ignores mode bits) and no permissions games.
        // ⭐ What makes it portable is that the test does not care WHICH kind comes back — any
        // kind other than NotFound satisfies it. (Measured here: `IsADirectory`, errno 21 on
        // macOS; Windows reports a different kind, and the assertion holds either way.)
        // ⚠️ pid-suffixed: two concurrent `cargo test` runs by the same user would otherwise
        // race on one fixed path, and one would delete the other's directory mid-assert.
        let tmp = std::env::temp_dir().join(format!(
            "talos-vscode-unreadable-manifest-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(manifest_path(&tmp)).expect("a directory where the file goes");
        assert_eq!(read_installed(&tmp), Err(()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_marketplace_cased_id_still_matches_the_lowercased_manifest() {
        // MEASURED on a real profile: all 30 manifest ids are lowercase, while the marketplace
        // API and the extension's own package.json both spell this publisher
        // `TheNuProjectContributors`. The shipped example package uses this id, so exact-case
        // matching would make its own row read falsely absent — and a false absent makes Apply
        // install what is already there.
        let exts = vec![Extension {
            id: "thenuprojectcontributors.vscode-nushell-lang".into(),
            version: "2.0.5".into(),
        }];
        assert_eq!(
            extension_version("TheNuProjectContributors.vscode-nushell-lang", &exts),
            Some("2.0.5".into())
        );
        // ⚠️ And case-insensitive must not become loose: a prefix is still not an id.
        assert_eq!(extension_version("TheNuProjectContributors", &exts), None);
    }

    #[test]
    fn json_that_is_not_an_array_is_an_error_not_an_empty_profile() {
        // The OTHER half of "Err ≠ empty": valid JSON of the wrong shape. Only the
        // unparseable case was pinned, so this branch could have returned Ok(vec![]) —
        // thirty rows to absent, and Apply offering what is already installed.
        for text in ["{}", "null", "42", "\"[]\""] {
            assert!(parse_installed_extensions(text).is_err(), "shape {text}");
        }
    }

    #[test]
    fn the_manifest_is_named_extensions_json_inside_the_dir() {
        // ⚠️ A typo in this ONE string fails SILENTLY: every read becomes NotFound, which is
        // the deliberate empty answer, so every extension row would read absent and nothing
        // would report an error. The filename is the aim of the whole module.
        assert!(manifest_path(Path::new("/x/exts")).ends_with("exts/extensions.json"));
    }

    #[test]
    fn an_entry_without_an_id_is_skipped_and_its_neighbours_survive() {
        // The promise the skip makes: one malformed record must not blank a machine — and
        // must not invent a blank-id row either.
        let exts = parse_installed_extensions(
            r#"[{"version":"1"},{"identifier":{},"version":"2"},{"identifier":{"id":"a.b"},"version":"3"}]"#,
        )
        .expect("recognised");
        assert_eq!(exts.len(), 1, "only the well-formed entry: {exts:?}");
        assert_eq!(extension_version("a.b", &exts), Some("3".into()));
        assert_eq!(extension_version("", &exts), None, "no blank-id ghost row");
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
    fn a_snapshot_carries_the_host_verdict_and_the_manifest_together() {
        // ⭐ ONE object, because both facts are once-per-SCAN: the manifest is one file for every
        // extension, and "is VS Code here?" is one question for every extension. Same shape as
        // `fetch_bulk_presence`, which beta.20 introduced to stop asking per package.
        let s = VscodeSnapshot {
            host_present: true,
            installed: Ok(vec![Extension {
                id: "a.b".into(),
                version: "1.0".into(),
            }]),
        };
        assert_eq!(s.verdict("a.b"), HostedVerdict::Present("1.0".into()));
        assert_eq!(s.verdict("c.d"), HostedVerdict::Absent);

        // Host gone ⇒ INDETERMINATE, never present and never absent. The manifest OUTLIVES the
        // host (~/.vscode is a user folder, independent of the app), so a manifest-only reading
        // would paint thirty green rows for an editor that is no longer installed.
        let gone = VscodeSnapshot {
            host_present: false,
            installed: Ok(vec![Extension {
                id: "a.b".into(),
                version: "1.0".into(),
            }]),
        };
        assert_eq!(gone.verdict("a.b"), HostedVerdict::NoHost);

        // Manifest unreadable ⇒ indeterminate too, with its own verdict so the row can say WHY.
        let broken = VscodeSnapshot {
            host_present: true,
            installed: Err(()),
        };
        assert_eq!(broken.verdict("a.b"), HostedVerdict::Unreadable);
    }

    #[test]
    fn the_snapshot_composes_a_real_directory_with_either_host_verdict() {
        // ⭐ The FIRST test that runs the composition rather than a hand-built struct: the
        // fixtures directory really is read, and both host values are reachable because
        // `snapshot_from` takes the verdict as a boolean instead of probing for it.
        //
        // ⚠️ The fixtures dir holds new-format.json etc., NOT extensions.json — so the read is a
        // legitimate NotFound, which is the EMPTY answer. That makes this a test of the wiring
        // (host verdict + directory → snapshot), and the manifest parsing is covered above.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vscode");
        let with_host = snapshot_from(true, &dir);
        assert!(with_host.host_present);
        assert_eq!(
            with_host.installed,
            Ok(Vec::new()),
            "no extensions.json here"
        );
        assert_eq!(with_host.verdict("a.b"), HostedVerdict::Absent);

        // ⭐ The same directory, the host gone: the verdict FLIPS from actionable to
        // indeterminate. That is the whole design in one assertion — presence needs the host,
        // and it is now reachable without uninstalling VS Code.
        assert_eq!(
            snapshot_from(false, &dir).verdict("a.b"),
            HostedVerdict::NoHost
        );

        // And a real manifest in a real directory, to prove the read is wired to the argument
        // and not to a constant path: copy a fixture in under the name the reader looks for.
        // ⚠️ pid-suffixed, like the unreadable-manifest test: two concurrent `cargo test` runs
        // by the same user would otherwise race on one fixed path.
        let tmp =
            std::env::temp_dir().join(format!("talos-vscode-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("a temp profile");
        std::fs::write(manifest_path(&tmp), fixture("new-format.json")).expect("a manifest");
        let s = snapshot_from(true, &tmp);
        assert_eq!(
            s.verdict("ms-python.python"),
            HostedVerdict::Present("2026.4.0".into()),
            "the manifest in the directory PASSED IN must be the one read"
        );
        // Host gone ⇒ that same present extension is no longer a claim we make.
        assert_eq!(
            snapshot_from(false, &tmp).verdict("ms-python.python"),
            HostedVerdict::NoHost,
            "the manifest OUTLIVES the host, so a hit in it proves nothing alone"
        );
        let _ = std::fs::remove_dir_all(&tmp);
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
