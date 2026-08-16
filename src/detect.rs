// Tri-state presence (Some(true)/Some(false)/None). ASK the
// machine, never a journal. Never panics: a failed spawn → absent/undetermined.
use crate::bundles::Step;
use crate::managers::{managers, native_manager};
use crate::platform::{shell_probe, Os, Probe};
use std::sync::LazyLock;

// Compiled once, not per probe: a scan runs strip_ansi + the generic version
// fallback for every package (28+). Both patterns are constant literals, so the
// unwrap can't fail — it runs a single time when the static is first touched.
static ANSI_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
static GENERIC_VERSION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\d+(?:\.\d+)+(?:[-.\w]*)?").unwrap());

#[derive(Debug, Clone, Default)]
pub struct Presence {
    pub present: Option<bool>,
    pub reason: Option<String>,
    pub version: Option<String>,
    pub external: bool,
    /// The detection evidence: command run + output + code. Exposed to the operator
    /// (written to the row's terminal at scan time) — "which command decided?".
    pub diag: Option<ProbeResult>,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub ok: bool,
    pub code: i32,
    pub output: String,
    pub cmdline: String,
}

/// Runs a Probe, captures code + merged output (stdout+stderr).
pub fn run_probe_detailed(probe: &Probe) -> ProbeResult {
    let cmdline = format!("{} {}", probe.cmd, probe.args.join(" "));
    match crate::platform::quiet_command(&probe.cmd)
        .args(&probe.args)
        .output()
    {
        Ok(o) => ProbeResult {
            ok: o.status.success(),
            code: o.status.code().unwrap_or(-1),
            output: merge_streams(
                &String::from_utf8_lossy(&o.stdout),
                &String::from_utf8_lossy(&o.stderr),
            ),
            cmdline,
        },
        Err(e) => ProbeResult {
            ok: false,
            code: -1,
            output: e.to_string(),
            cmdline,
        },
    }
}

// Join stdout + stderr for the operator's evidence AND for version parsing.
// A newline MUST separate them: gluing them raw lets a token bleed across the
// boundary — e.g. stdout "v9.9.9" (no trailing newline) + a shell's stderr
// "bash: no job control" parsed to "9.9.9bash", because the generic version
// regex's `[-.\w]*` tail swallows the adjacent letters. Seen only on a
// tty-less Linux CI runner, never on a Mac tty — a classic porting regression.
fn merge_streams(stdout: &str, stderr: &str) -> String {
    if stdout.is_empty() {
        return stderr.to_owned();
    }
    if stderr.is_empty() {
        return stdout.to_owned();
    }
    let sep = if stdout.ends_with('\n') { "" } else { "\n" };
    format!("{stdout}{sep}{stderr}")
}

fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").into_owned()
}

/// Extracts the version revealed by the probe's output. Regex override: bundle > manager > generic.
pub fn version_from(step: &Step, output: &str) -> String {
    let clean = strip_ansi(output);
    if let Some(re_src) = &step.version_regex {
        if let Ok(re) = regex::Regex::new(re_src) {
            if let Some(caps) = re.captures(&clean) {
                return caps
                    .get(1)
                    .or_else(|| caps.get(0))
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
            }
        }
        return String::new();
    }
    if let (Some(route), Some(sid)) = (&step.route, &step.system_id) {
        if let Some(mgr) = managers().into_iter().find(|m| m.route == route) {
            let v = mgr.parse_version(sid, &clean);
            if !v.is_empty() {
                return v;
            }
        }
    }
    // generic: first version-like token (no leading \b — "v26.4.0").
    GENERIC_VERSION_RE
        .find(&clean)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// The user's HOME (Mac/Linux $HOME, Windows %USERPROFILE%). The .app/.exe runs
/// in the user's HOME, so the agent content (~/.claude, ~/.agents) is relative to it.
pub fn user_home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

/// Detects a Claude plugin (route claude-plugin) or a skill (route skill) via
/// native DISK READ. `detect:` carries the name to look for. Builds a diag that
/// shows the evidence (file/dir consulted + verdict), like the command probes.
fn detect_agent_content(step: &Step) -> Presence {
    use crate::agent_content::{
        list_skills, parse_installed_plugins, plugin_version, plugins_json_path, skill_dirs,
        skill_path,
    };
    let name = step.detect.as_deref().unwrap_or("").trim();
    let home = user_home();

    if step.route.as_deref() == Some("claude-plugin") {
        let path = plugins_json_path(&home);
        let json = std::fs::read_to_string(&path).unwrap_or_default();
        let plugins = parse_installed_plugins(&json);
        let found = plugin_version(name, &plugins);
        let present = found.is_some();
        let version = found.clone().unwrap_or_default();
        // Evidence: the file read + the line found (or "not found").
        let evidence = match &found {
            Some(v) => format!("{name}  version {v}"),
            None => format!("{name}: not found"),
        };
        return Presence {
            present: Some(present),
            version: Some(version),
            diag: Some(ProbeResult {
                ok: present,
                code: if present { 0 } else { 1 },
                output: evidence,
                cmdline: format!("read {}", path.display()),
            }),
            ..Default::default()
        };
    }

    // route "skill": presence = a subdirectory of the same name under ~/.claude/skills
    // or ~/.agents/skills.
    let dirs = skill_dirs(&home);
    let skills = list_skills(&dirs);
    let found = skill_path(name, &skills);
    let present = found.is_some();
    let evidence = match &found {
        Some(p) => format!("{}", p.display()),
        None => format!("{name}: not found in ~/.claude/skills or ~/.agents/skills"),
    };
    Presence {
        present: Some(present),
        diag: Some(ProbeResult {
            ok: present,
            code: if present { 0 } else { 1 },
            output: evidence,
            cmdline: "scan skill dirs".into(),
        }),
        ..Default::default()
    }
}

/// Presence of a VS Code extension, from the scan's snapshot.
///
/// ⭐ Three of the four verdicts are NOT `Some(false)`, and that is deliberate. `Some(false)`
/// makes `action_for` return `Install`, so a wrong "absent" is not a cosmetic mislabel — it is
/// an action Talos takes. Only "the host answered and its manifest does not list the id" earns
/// it; every other uncertainty stays indeterminate and gets re-probed.
fn detect_vscode_extension(step: &Step, snap: Option<&crate::vscode::VscodeSnapshot>) -> Presence {
    use crate::vscode::HostedVerdict;
    let id = step.detect.as_deref().unwrap_or("").trim();
    // ⚠️ No id to look for ⇒ indeterminate, NOT absent. Stage 3b fires on the ROUTE alone
    // (unlike stage 2, which requires a `detect:`), so a package declaring the route and no id
    // arrives here with an empty string — and the manifest lookup would honestly find nothing,
    // which reads as Absent and makes Apply install on every pass for ever, since the next scan
    // finds it just as absent.
    //
    // ⭐ An empty id is a missing declaration in the CATALOGUE, not a fact about the MACHINE, and
    // only facts about the machine may earn `Some(false)`. `load_from_catalog` now falls back
    // `detect: p.detect.or(p.vscode_extension)`, so a package loaded from a catalogue file can no
    // longer reach this branch — it is a belt to that brace. It stays anyway: a hand-written YAML
    // that declares the route and omits BOTH fields still arrives here, and so does any caller
    // building a `Step` directly.
    if id.is_empty() {
        return Presence {
            present: None,
            reason: Some("no extension id to look for".into()),
            diag: Some(ProbeResult {
                ok: false,
                code: 1,
                output: "this package declares the vscode-extension route but no id".into(),
                cmdline: "read extensions.json".into(),
            }),
            ..Default::default()
        };
    }
    // No snapshot (the post-action re-probe path) ⇒ nothing is known. Honest, and re-probed by
    // seeds_for_rescan on the next Apply.
    let Some(snap) = snap else {
        return Presence {
            present: None,
            reason: Some("VS Code not consulted in this pass".into()),
            diag: Some(ProbeResult {
                ok: false,
                code: 1,
                output: format!("{id}: no VS Code snapshot for this probe"),
                cmdline: "read extensions.json".into(),
            }),
            ..Default::default()
        };
    };
    // ⭐ ONE call, destructured once. Asking twice (verdict for the tuple, verdict again for the
    // version) invites the two readings to drift, and the second would silently disagree.
    let (present, reason, version, evidence) = match snap.verdict(id) {
        HostedVerdict::Present(v) => {
            let evidence = format!("{id}  version {v}");
            (Some(true), None, v, evidence)
        }
        HostedVerdict::Absent => (
            Some(false),
            None,
            String::new(),
            format!("{id}: not in extensions.json"),
        ),
        // The manifest OUTLIVES the host: ~/.vscode is a user folder that survives
        // uninstalling the editor, so a manifest hit proves nothing without the host.
        HostedVerdict::NoHost => (
            None,
            Some("VS Code not found".into()),
            String::new(),
            format!("{id}: VS Code did not answer — the manifest cannot be trusted alone"),
        ),
        // ⚠️ Worded for what is KNOWN, not for the likeliest cause. `read_installed` collapses a
        // locked file, an EACCES and unparseable text into one `Err(())`, so "exists and was not
        // understood" would send an operator hunting for corruption on a permissions problem.
        HostedVerdict::Unreadable => (
            None,
            Some("extensions.json could not be read".into()),
            String::new(),
            format!("{id}: extensions.json could not be read"),
        ),
    };
    Presence {
        present,
        reason,
        version: Some(version),
        diag: Some(ProbeResult {
            ok: present == Some(true),
            code: i32::from(present != Some(true)),
            output: evidence,
            cmdline: "read extensions.json".into(),
        }),
        ..Default::default()
    }
}

fn presence_probe(detect_cmd: Option<&str>, os: Os) -> Option<Probe> {
    let cmd = detect_cmd.unwrap_or("").trim();
    if cmd.is_empty() {
        None
    } else {
        Some(shell_probe(os, cmd))
    }
}

fn system_probe(step: &Step, os: Os) -> Option<Probe> {
    let mgr = native_manager(os)?;
    let sid = step.system_id.as_ref()?;
    if step.route.as_deref() != Some(mgr.route) {
        return None;
    }
    Some(shell_probe(os, &mgr.presence_command(sid)))
}

/// The machine-wide installed table: lowercased manager id → version.
pub type BulkPresence = std::collections::HashMap<String, String>;

/// Answer the MANAGER half of presence from the machine-wide listing instead of
/// spawning a per-package probe.
///
/// `None` means "no answer here, probe as before" — either there is no table, or
/// this package is not on the native manager's route. `Some` carries the verdict
/// AND the evidence, whose cmdline names the listing rather than a command that
/// never ran.
fn system_from_bulk(step: &Step, os: Os, bulk: Option<&BulkPresence>) -> Option<ProbeResult> {
    let bulk = bulk?;
    let mgr = native_manager(os)?;
    let sid = step.system_id.as_deref()?;
    if step.route.as_deref() != Some(mgr.route) {
        return None;
    }
    let cmdline = mgr.presence_scan_command();
    Some(match bulk.get(&sid.to_lowercase()) {
        // In the listing: present, and the listing IS the evidence.
        Some(v) => ProbeResult {
            ok: true,
            code: 0,
            output: format!("{sid} {v}"),
            cmdline,
        },
        // Consulted and absent: no process needed to conclude that either.
        None => ProbeResult {
            ok: false,
            code: 1,
            output: format!("{sid}: not in the installed list"),
            cmdline,
        },
    })
}

/// Run the machine-wide presence listing(s) and parse them into one table.
///
/// `None` when there is no native manager, the command fails, or the output is not
/// recognised. Every one of those means "probe per package", never "nothing is
/// installed" — reading an unreadable listing as an empty machine would make Apply
/// offer to install software that is already there.
///
/// Serial on purpose, like everything in the scan: this removes processes, it adds
/// no concurrency (`talos-scan-serial-and-narrated`).
pub fn fetch_bulk_presence(os: Os) -> Option<BulkPresence> {
    let mgr = native_manager(os)?;
    let mut text = String::new();
    for cmd in std::iter::once(mgr.presence_scan_command()).chain(mgr.presence_scan_command_cask())
    {
        let probe = shell_probe(os, &cmd);
        let d = run_probe_detailed(&probe);
        // ⚠️ Do NOT bail on a non-zero exit. brew's cask listing can complain while
        // still printing usable lines, and `parse_presence` is the authority on
        // whether the text was understood — an exit code is not.
        text.push_str(&d.output);
        text.push('\n');
    }
    mgr.parse_presence(&text).ok()
}

/// Detailed presence. Order: check → binary+manager → content-list → exit-code.
pub fn detect_present_detailed(step: &Step, os: Os) -> Presence {
    detect_present_detailed_with(step, os, None)
}

/// Presence, with an optional machine-wide listing to consult instead of spawning a
/// per-package manager probe.
///
/// `bulk = None` ⇒ probe exactly as before. That is what makes this reversible: an
/// unreadable listing degrades to today's behaviour, never to a screen of false
/// absents (which would make Apply offer to install what is already there).
pub fn detect_present_detailed_with(step: &Step, os: Os, bulk: Option<&BulkPresence>) -> Presence {
    detect_present_detailed_with_vscode(step, os, bulk, None)
}

/// Presence, with the optional machine-wide listing AND the optional VS Code snapshot.
///
/// Both extra arguments are `Option`: `None` ⇒ probe/answer exactly as before. That is what
/// keeps this reversible, and it is not hypothetical — the post-action re-probe goes through
/// `detect_present_detailed`, which passes None for both.
pub fn detect_present_detailed_with_vscode(
    step: &Step,
    os: Os,
    bulk: Option<&BulkPresence>,
    vscode: Option<&crate::vscode::VscodeSnapshot>,
) -> Presence {
    // 1. check (config-atom) — verbatim, exit 0 = converged.
    if let Some(check) = &step.check {
        let probe = shell_probe(os, check);
        let d = run_probe_detailed(&probe);
        return Presence {
            present: Some(d.ok),
            diag: Some(d),
            ..Default::default()
        };
    }
    // 2. binary (non content-detected route) — 2 combined probes.
    //
    // ⚠️ THE PREDICATE, NOT A LIST. This guard used to enumerate the content routes it
    // excludes — an allowlist-by-NEGATION, so every future content route was wrong BY DEFAULT
    // until someone remembered to extend it here. And "wrong" is loud: a content route's
    // `detect:` holds an id, not a command, so the binary probe would run `publisher.name` as
    // an executable, read "command not found", and report installed software as ABSENT.
    //
    // ⭐ Of the three route enumerations in this file, only this one is dangerous. The other two
    // DISPATCH — a missing route there means "not handled", which reads as unknown. A missing
    // route HERE means "handled WRONG". So this is the site that must consult
    // `bundles::is_extension_route`, the single place the class is decided.
    if step.detect.is_some() && !crate::bundles::is_extension_route(step.route.as_deref()) {
        let bin = presence_probe(step.detect.as_deref(), os).map(|p| run_probe_detailed(&p));
        let bin_ok = bin.as_ref().map(|d| d.ok).unwrap_or(false);
        // The MANAGER half. A machine-wide listing answers it without a process; no
        // listing (or a package on another route) falls back to probing.
        //
        // ⚠️ The BINARY half above stays per-package on purpose: presence is
        // `bin_ok || system_ok`, and a package can be present outside its manager
        // (git from Xcode CLT — the hole talos-scope-second-axis closed). Dropping
        // it would re-open that. `sp` still decides `external`, so it is computed
        // even when the listing answered: it says "this package HAS a manager
        // route", which is a fact about the catalogue, not a probe of the machine.
        let sp = system_probe(step, os);
        let sd = system_from_bulk(step, os, bulk).or_else(|| sp.as_ref().map(run_probe_detailed));
        let system_ok = sd.as_ref().map(|d| d.ok).unwrap_or(false);
        let present = bin_ok || system_ok;
        if !present {
            // Absent: show the binary probe (the one queried first) — the most
            // telling evidence of "why absent" (often "command not found").
            return Presence {
                present: Some(false),
                diag: bin.or(sd),
                ..Default::default()
            };
        }
        if system_ok {
            // Presence established by the SYSTEM probe → IT is the evidence to show
            // (not the binary, which may have failed). diag follows the verdict.
            let sd = sd.unwrap();
            // ⭐ Field-reported defect (beta.32): A manager listing is UNORDERED and
            // reports every installed version, but a binary reports what actually RUNS.
            // When the binary probe answered (bin_ok), the VERSION comes from IT,
            // because "what executes" is singular and known, while "what is installed"
            // is plural and can only guess. Presence stays unchanged: bin_ok || system_ok.
            // If the binary answered but printed no parseable version, fall back to
            // the table rather than losing information.
            let bin_version = bin.as_ref().and_then(|d| {
                let v = version_from(step, &d.output);
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            });
            let v = if bin_ok {
                bin_version.unwrap_or_else(|| version_from(step, &sd.output))
            } else {
                version_from(step, &sd.output)
            };
            return Presence {
                present: Some(true),
                version: Some(v),
                diag: Some(sd),
                ..Default::default()
            };
        }
        // Presence established by the BINARY probe → show that one.
        let v = bin
            .as_ref()
            .map(|d| version_from(step, &d.output))
            .unwrap_or_default();
        return Presence {
            present: Some(true),
            version: Some(v),
            external: sp.is_some(),
            diag: bin,
            ..Default::default()
        };
    }
    // 3. content-detected (claude-plugin/skill) → NATIVE disk read (no
    //    shell-out: arch decision "everything parsed natively as JSON"). The step's
    //    `detect:` carries the name (plugin id "chiron@tekton"/"chiron", or skill name).
    if matches!(step.route.as_deref(), Some("claude-plugin") | Some("skill")) {
        return detect_agent_content(step);
    }
    // 3b. vscode-extension → the once-per-scan snapshot (host verdict + profile manifest).
    //     No shell-out here at all: the host was probed once, for every row.
    if step.route.as_deref() == Some("vscode-extension") {
        return detect_vscode_extension(step, vscode);
    }
    // 4. exit-code (system manager without a binary detect). This stage is a pure
    //    manager probe, so the listing replaces it whole — there is no binary half
    //    to preserve here.
    if let Some(d) = system_from_bulk(step, os, bulk) {
        let v = version_from(step, &d.output);
        return Presence {
            present: Some(d.ok),
            version: Some(v),
            diag: Some(d),
            ..Default::default()
        };
    }
    if let Some(probe) = system_probe(step, os) {
        let d = run_probe_detailed(&probe);
        let v = version_from(step, &d.output);
        return Presence {
            present: Some(d.ok),
            version: Some(v),
            diag: Some(d),
            ..Default::default()
        };
    }
    // Nothing above could answer. ⚠️ Reachable for a CONTENT ROUTE THAT HAS NO DISPATCH ARM —
    // the stage-2 guard excludes every extension route by class (`is_extension_route`), so a
    // future one arrives here rather than being run as a command. That is the safe direction
    // and it is deliberate, but a row with no reason teaches nothing, so name it.
    //
    // ⭐ Only when there IS a route: a package with no `check:`, no `detect:` and no route at all
    // also lands here, and "no way to detect a  package" would be noise about a row that simply
    // declares nothing. `requires_reason` still outranks this at the wire (server.rs:1491), so a
    // dependency explanation is never displaced by it.
    Presence {
        present: None,
        reason: step
            .route
            .as_deref()
            .map(|r| format!("no way to detect a {r} package")),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::{Overrides, Posture, Step};

    fn step() -> Step {
        Step {
            id: "n".into(),
            bundle: "t".into(),
            name: "n".into(),
            description: String::new(),
            install: None,
            uninstall: None,
            upgrade: None,
            downgrade: None,
            route: None,
            system_id: None,
            detect: None,
            check: None,
            is_config: false,
            is_extension: false,
            version_regex: None,
            pin: None,
            requires: vec![],
            posture: Posture::OptIn,
            categories: vec!["misc".into()],
            overrides: Overrides::default(),
        }
    }

    #[test]
    fn a_bulk_hit_replaces_the_manager_probe() {
        let mut s = step();
        s.route = Some("brew".into());
        s.system_id = Some("jq".into());
        s.detect = None; // no binary probe: isolate the manager half
        let mut bulk = std::collections::HashMap::new();
        bulk.insert("jq".to_string(), "1.8.2".to_string());
        let p = detect_present_detailed_with(&s, Os::Darwin, Some(&bulk));
        assert_eq!(p.present, Some(true));
        assert_eq!(p.version.as_deref(), Some("1.8.2"));
        // The evidence must name the listing, not a per-package command that never ran.
        assert!(
            p.diag.as_ref().unwrap().cmdline.contains("brew list"),
            "{:?}",
            p.diag
        );
    }

    #[test]
    fn a_package_absent_from_the_table_is_absent() {
        let mut s = step();
        s.route = Some("brew".into());
        s.system_id = Some("not-installed".into());
        s.detect = None;
        let bulk = std::collections::HashMap::new();
        let p = detect_present_detailed_with(&s, Os::Darwin, Some(&bulk));
        assert_eq!(p.present, Some(false));
    }

    #[test]
    fn no_table_falls_back_to_the_per_package_probe() {
        // ⭐ The reversibility guarantee: None means "probe as before", so an
        // unreadable listing degrades to today's behaviour rather than to a screen
        // of false absents.
        let mut s = step();
        s.route = Some("brew".into());
        s.system_id = Some("definitely-not-a-package-xyz".into());
        s.detect = None;
        let p = detect_present_detailed_with(&s, Os::Darwin, None);
        // Whatever the real machine answers, the point is that it ASKED: a probe ran,
        // so there is a per-package command line in the evidence.
        assert!(
            p.diag
                .as_ref()
                .is_some_and(|d| d.cmdline.contains("definitely-not-a-package-xyz")),
            "the fallback must run a per-package probe: {:?}",
            p.diag
        );
    }

    #[test]
    fn the_binary_probe_still_answers_for_a_package_outside_its_manager() {
        // ⭐ The hole talos-scope-second-axis was written for: `git` from Xcode CLT
        // is PRESENT while brew never heard of it. The bulk table says absent, the
        // binary probe says present, and `bin_ok || system_ok` must still win.
        if cfg!(target_os = "windows") {
            return;
        }
        let mut s = step();
        s.route = Some("brew".into());
        s.system_id = Some("git".into());
        s.detect = Some("printf v9.9.9".into()); // stands in for a binary that answers
        let bulk = std::collections::HashMap::new(); // brew knows nothing
        let p = detect_present_detailed_with(&s, Os::Darwin, Some(&bulk));
        assert_eq!(
            p.present,
            Some(true),
            "a binary outside its manager is still present: {:?}",
            p.diag
        );
        assert!(p.external, "present, but not through its manager");
    }

    #[test]
    fn when_binary_and_table_disagree_on_version_the_binary_decides() {
        // ⭐ Field-reported defect (beta.31 fixed sorting, beta.32 fixes precedence):
        // the bulk table is UNORDERED and reports every installed version, but the
        // binary reports what actually RUNS. A manager listing says "what is installed"
        // (plural), a binary says "what executes" (singular). When the binary probe
        // answered, the VERSION must come from it — presence stays `bin_ok || system_ok`.
        if cfg!(target_os = "windows") {
            return;
        }
        let mut s = step();
        s.route = Some("brew".into());
        s.system_id = Some("nushell".into());
        s.detect = Some("printf 0.113.1".into()); // binary reports 0.113.1
        let mut bulk = std::collections::HashMap::new();
        bulk.insert("nushell".to_string(), "0.114.0".to_string()); // table reports 0.114.0
        let p = detect_present_detailed_with(&s, Os::Darwin, Some(&bulk));
        assert_eq!(p.present, Some(true), "present is unchanged");
        assert_eq!(
            p.version.as_deref(),
            Some("0.113.1"),
            "version comes from the BINARY, not the table"
        );
    }

    #[test]
    fn version_generic_token() {
        let mut s = step();
        s.detect = Some("node --version".into());
        assert_eq!(version_from(&s, "v26.4.0"), "26.4.0");
    }

    #[test]
    fn version_regex_override() {
        let mut s = step();
        s.version_regex = Some(r"custom (\d+\.\d+)".into());
        assert_eq!(version_from(&s, "custom 1.2 blah"), "1.2");
    }

    #[test]
    fn check_present_when_exit_zero() {
        let mut s = step();
        s.check = Some("true".into()); // exit 0 on POSIX
        if !cfg!(target_os = "windows") {
            assert_eq!(detect_present_detailed(&s, Os::Darwin).present, Some(true));
        }
    }

    #[test]
    fn diag_captures_the_binary_probe_command() {
        // The evidence shown must be the command ACTUALLY run (operator
        // transparency: "which command decided present/absent?").
        if cfg!(target_os = "windows") {
            return;
        }
        let mut s = step();
        s.detect = Some("printf v9.9.9".into()); // exit 0 → present
        let p = detect_present_detailed(&s, Os::Darwin);
        assert_eq!(p.present, Some(true));
        let diag = p.diag.expect("diag captured");
        assert!(
            diag.cmdline.contains("printf v9.9.9"),
            "cmdline = {}",
            diag.cmdline
        );
        assert!(diag.output.contains("v9.9.9"), "output = {}", diag.output);
        assert_eq!(p.version.as_deref(), Some("9.9.9"));
    }

    #[test]
    fn merge_streams_separates_so_a_token_cannot_bleed() {
        // Regression (Linux CI): stdout "v9.9.9" (no trailing \n) glued to a
        // tty-less shell's stderr produced "9.9.9bash" once the generic regex
        // ran. A newline between the streams keeps the version clean.
        let merged = merge_streams("v9.9.9", "bash: no job control");
        assert_eq!(merged, "v9.9.9\nbash: no job control");
        let mut s = step();
        s.detect = Some("x".into()); // generic path (no route/regex)
        assert_eq!(version_from(&s, &merged), "9.9.9");
    }

    #[test]
    fn merge_streams_no_double_newline_or_empty_noise() {
        assert_eq!(merge_streams("out\n", "err"), "out\nerr"); // already newline-terminated
        assert_eq!(merge_streams("out", ""), "out"); // stderr empty → no trailing sep
        assert_eq!(merge_streams("", "err"), "err"); // stdout empty → stderr verbatim
    }

    #[test]
    fn a_vscode_extension_reads_its_presence_from_the_snapshot() {
        use crate::vscode::{Extension, VscodeSnapshot};
        let mut s = step();
        s.route = Some("vscode-extension".into());
        s.detect = Some("thenuprojectcontributors.vscode-nushell-lang".into());
        let snap = VscodeSnapshot {
            host_present: true,
            installed: Ok(vec![Extension {
                id: "thenuprojectcontributors.vscode-nushell-lang".into(),
                version: "2.0.5".into(),
            }]),
        };
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, Some(&snap));
        assert_eq!(p.present, Some(true));
        assert_eq!(p.version.as_deref(), Some("2.0.5"));
    }

    #[test]
    fn a_vscode_extension_without_its_host_is_indeterminate_with_a_reason() {
        use crate::vscode::{Extension, VscodeSnapshot};
        let mut s = step();
        s.route = Some("vscode-extension".into());
        s.detect = Some("some.ext".into());
        // The manifest still lists it — VS Code was uninstalled, ~/.vscode survived.
        let snap = VscodeSnapshot {
            host_present: false,
            installed: Ok(vec![Extension {
                id: "some.ext".into(),
                version: "1.0".into(),
            }]),
        };
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, Some(&snap));
        // ⭐ None, NOT Some(false). Some(false) makes action_for return Install, so a wrong
        // "absent" is not a mislabel — it is an action Talos would take. None is rendered by the
        // front already (app.js: `if (msg.present === null && msg.reason)`) and re-probed by
        // seeds_for_rescan.
        assert_eq!(p.present, None);
        let reason = p.reason.expect("an indeterminate row must say why");
        assert!(reason.to_lowercase().contains("code"), "reason: {reason}");
    }

    #[test]
    fn an_unreadable_manifest_is_indeterminate_never_absent() {
        use crate::vscode::VscodeSnapshot;
        let id = "some.ext";
        let mut s = step();
        s.route = Some("vscode-extension".into());
        s.detect = Some(id.into());
        let snap = VscodeSnapshot {
            host_present: true,
            installed: Err(()),
        };
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, Some(&snap));
        assert_eq!(
            p.present, None,
            "an unreadable manifest must never blank the machine"
        );
        // ⚠️ And the wording must not diagnose a cause it cannot know. `read_installed` returns
        // Err(()) for a LOCKED file, an EACCES and a truncated one alike, so "not understood"
        // would send an operator hunting for corruption on a permissions problem.
        let reason = p.reason.expect("an indeterminate row must say why");
        assert!(
            !reason.contains("not understood"),
            "the cause is unknown at this layer: {reason}"
        );
        let diag = p.diag.expect("evidence, even when nothing is known");
        // ⚠️ The diag is what the row's TERMINAL prints, and it was the half of Presence with
        // no coverage: `ok: true, code: 0` for a machine that was never asked would read as a
        // successful probe. Derived from the verdict, so it cannot disagree with it.
        assert!(
            !diag.ok,
            "an indeterminate verdict is not a successful probe"
        );
        assert_ne!(diag.code, 0);
        // ⚠️ And the EVIDENCE must not name a cause this layer cannot know either — the
        // `reason` assertion above pins only one of the two strings the operator reads.
        assert!(
            !diag.output.contains("not understood"),
            "the evidence must not name a cause read_installed cannot distinguish: {}",
            diag.output
        );
        // The cmdline names the gesture that decided. Never a command line — this route
        // shells out NOTHING; the host was probed once, for the whole scan.
        assert!(
            !diag.cmdline.contains("--list-extensions") && !diag.cmdline.contains(id),
            "no command was run to decide this: {}",
            diag.cmdline
        );
    }

    #[test]
    fn a_host_that_answers_and_a_manifest_that_does_not_list_it_is_honestly_absent() {
        use crate::vscode::VscodeSnapshot;
        let mut s = step();
        s.route = Some("vscode-extension".into());
        s.detect = Some("nosuch.ext".into());
        let snap = VscodeSnapshot {
            host_present: true,
            installed: Ok(vec![]),
        };
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, Some(&snap));
        // The ONLY actionable verdict: the host answered and its manifest genuinely lacks the id.
        assert_eq!(p.present, Some(false));
    }

    #[test]
    fn an_extension_row_with_no_id_to_look_for_is_indeterminate_not_absent() {
        // ⚠️ Stage 3b fires on the ROUTE alone, unlike stage 2 which needs a `detect:`. So a
        // package that declares the route and no id lands here with an empty string — and
        // `extension_version("", …)` correctly finds nothing, which would read as Absent and
        // make Apply install something on every single pass, for ever, since the next scan finds
        // it just as absent.
        //
        // ⭐ An empty id is not a fact about the MACHINE, it is a missing declaration in the
        // CATALOGUE, and only facts about the machine may earn `Some(false)`.
        use crate::vscode::{Extension, VscodeSnapshot};
        let mut s = step();
        s.route = Some("vscode-extension".into());
        s.detect = None;
        let snap = VscodeSnapshot {
            host_present: true,
            installed: Ok(vec![Extension {
                id: "some.ext".into(),
                version: "1.0".into(),
            }]),
        };
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, Some(&snap));
        assert_eq!(p.present, None, "a missing id must not read as absent");
        assert!(p.reason.is_some(), "and it must say why");
    }

    #[test]
    fn a_vscode_extension_never_falls_into_the_binary_probe_branch() {
        // ⚠️ THE TRAP. detect.rs's stage 2 fires on `detect.is_some() && !is_extension_route(…)`,
        // so without the exclusion a vscode-extension package would shell out its own ID as a
        // COMMAND — `thenuprojectcontributors.vscode-nushell-lang` — read "command not found",
        // and report a perfectly installed extension as absent. The id is not a command.
        let mut s = step();
        s.route = Some("vscode-extension".into());
        s.detect = Some("definitely.not-a-command-xyz".into());
        // No snapshot: the fallback path (the post-action re-probe at server.rs:2229 uses it).
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, None);
        // Without a snapshot nothing is known — which is honest. What must NOT happen is
        // Some(false) obtained by running the id as a command.
        assert_eq!(p.present, None);
        let diag = p.diag.expect("evidence");
        assert!(
            !diag.cmdline.contains("definitely.not-a-command-xyz"),
            "the id must never be executed: {}",
            diag.cmdline
        );
    }

    #[test]
    fn a_content_route_with_no_dispatch_arm_says_so_instead_of_going_silent() {
        // ⚠️ The dead end the widened stage-2 guard made reachable. `is_extension_route` excludes
        // the whole CLASS, so a future `code-insiders` route — excluded at stage 2, matched by no
        // dispatch arm, no manager probe — falls all the way through. That is the safe direction
        // (silent unknown beats running `publisher.name` as a command), but a row drawn "—" with
        // nothing to explain it brushes "every gesture leaves a trace".
        // A route no arm handles — which no catalogue declares yet, so it is built directly here.
        // `code-insiders` is the concrete candidate `is_extension_route`'s own doc names as the
        // next one to arrive.
        let mut s = step();
        s.route = Some("code-insiders".into());
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, None);
        assert_eq!(p.present, None, "unknown, not absent");
        let reason = p.reason.expect("a dead end must still name itself");
        assert!(
            reason.contains("code-insiders"),
            "the reason must name the route that has no detection: {reason}"
        );
    }

    #[test]
    fn a_package_that_declares_nothing_gets_no_invented_reason() {
        // ⭐ The other side of the line above. A step with no route, no `check:` and no `detect:`
        // reaches the same fall-through, and "no way to detect a  package" would be noise about a
        // row that simply declares nothing to detect. Silence is right HERE and wrong there.
        let s = step(); // route: None, check: None, detect: None
        let p = detect_present_detailed_with_vscode(&s, Os::Darwin, None, None);
        assert_eq!(p.present, None);
        assert_eq!(
            p.reason, None,
            "nothing was declared, so there is nothing to explain"
        );
    }

    #[test]
    fn diag_on_absent_shows_the_probe() {
        if cfg!(target_os = "windows") {
            return;
        }
        let mut s = step();
        s.detect = Some("false".into()); // non-zero exit → absent
        let p = detect_present_detailed(&s, Os::Darwin);
        assert_eq!(p.present, Some(false));
        assert!(p.diag.expect("diag").cmdline.contains("false"));
    }
}
