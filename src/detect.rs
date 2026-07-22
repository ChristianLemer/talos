// Port of detect.ts — tri-state presence (Some(true)/Some(false)/None). ASK the
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
        Ok(o) => {
            let mut output = String::from_utf8_lossy(&o.stdout).into_owned();
            output.push_str(&String::from_utf8_lossy(&o.stderr));
            ProbeResult {
                ok: o.status.success(),
                code: o.status.code().unwrap_or(-1),
                output,
                cmdline,
            }
        }
        Err(e) => ProbeResult {
            ok: false,
            code: -1,
            output: e.to_string(),
            cmdline,
        },
    }
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
fn user_home() -> std::path::PathBuf {
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

/// Detailed presence. Order: check → binary+manager → content-list → exit-code.
pub fn detect_present_detailed(step: &Step, os: Os) -> Presence {
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
    if step.detect.is_some()
        && !matches!(step.route.as_deref(), Some("claude-plugin") | Some("skill"))
    {
        let bin = presence_probe(step.detect.as_deref(), os).map(|p| run_probe_detailed(&p));
        let bin_ok = bin.as_ref().map(|d| d.ok).unwrap_or(false);
        let sp = system_probe(step, os);
        let sd = sp.as_ref().map(run_probe_detailed);
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
            let v = version_from(step, &sd.output);
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
    // 4. exit-code (system manager without a binary detect).
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
    Presence {
        present: None,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::{Posture, Step};

    fn step() -> Step {
        Step {
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
            version_regex: None,
            pin: None,
            requires: vec![],
            posture: Posture::OptIn,
            categories: vec!["misc".into()],
        }
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
        assert!(diag.cmdline.contains("printf v9.9.9"), "cmdline = {}", diag.cmdline);
        assert!(diag.output.contains("v9.9.9"), "output = {}", diag.output);
        assert_eq!(p.version.as_deref(), Some("9.9.9"));
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
