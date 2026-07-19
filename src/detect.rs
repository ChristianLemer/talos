// Port de detect.ts — présence tri-state (Some(true)/Some(false)/None). ASK the
// machine, jamais un journal. Ne panique jamais : un spawn échoué → absent/indéterminé.
use crate::bundles::Step;
use crate::managers::{managers, native_manager};
use crate::platform::{shell_probe, Os, Probe};

#[derive(Debug, Clone, Default)]
pub struct Presence {
    pub present: Option<bool>,
    pub reason: Option<String>,
    pub version: Option<String>,
    pub external: bool,
    /// La preuve de détection : commande lancée + sortie + code. Exposée à l'opérateur
    /// (écrite dans le terminal de la ligne au scan) — "quelle commande a décidé ?".
    pub diag: Option<ProbeResult>,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub ok: bool,
    pub code: i32,
    pub output: String,
    pub cmdline: String,
}

/// Exécute un Probe, capture code + sortie fusionnée (stdout+stderr).
pub fn run_probe_detailed(probe: &Probe) -> ProbeResult {
    let cmdline = format!("{} {}", probe.cmd, probe.args.join(" "));
    match std::process::Command::new(&probe.cmd)
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
    regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
        .unwrap()
        .replace_all(s, "")
        .into_owned()
}

/// Extrait la version révélée par la sortie du probe. Override regex bundle > manager > générique.
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
    // générique : premier token version-like (pas de \b initial — "v26.4.0").
    regex::Regex::new(r"\d+(?:\.\d+)+(?:[-.\w]*)?")
        .unwrap()
        .find(&clean)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
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

/// Présence détaillée. Ordre : check → binaire+manager → content-list → exit-code.
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
    // 2. binaire (route non content-detected) — 2 sondes combinées.
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
            // Absent : montrer la sonde binaire (celle qu'on interroge d'abord) — la
            // preuve la plus parlante du "pourquoi absent" (souvent "command not found").
            return Presence {
                present: Some(false),
                diag: bin.or(sd),
                ..Default::default()
            };
        }
        if system_ok {
            // Présence établie par la sonde SYSTÈME → c'est ELLE la preuve à montrer
            // (pas la binaire, qui a pu échouer). diag suit le verdict.
            let sd = sd.unwrap();
            let v = version_from(step, &sd.output);
            return Presence {
                present: Some(true),
                version: Some(v),
                diag: Some(sd),
                ..Default::default()
            };
        }
        // Présence établie par la sonde BINAIRE → montrer celle-là.
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
    // 3. content-detected (claude-plugin/skill) → indéterminé en Phase 1 (parse reporté).
    if matches!(step.route.as_deref(), Some("claude-plugin") | Some("skill")) {
        return Presence {
            present: None,
            ..Default::default()
        };
    }
    // 4. exit-code (system manager sans detect binaire).
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
        s.check = Some("true".into()); // exit 0 sur POSIX
        if !cfg!(target_os = "windows") {
            assert_eq!(detect_present_detailed(&s, Os::Darwin).present, Some(true));
        }
    }

    #[test]
    fn diag_capture_la_commande_de_la_sonde_binaire() {
        // La preuve montrée doit être la commande RÉELLEMENT lancée (transparence
        // opérateur : "quelle commande a décidé présent/absent ?").
        if cfg!(target_os = "windows") {
            return;
        }
        let mut s = step();
        s.detect = Some("printf v9.9.9".into()); // exit 0 → présent
        let p = detect_present_detailed(&s, Os::Darwin);
        assert_eq!(p.present, Some(true));
        let diag = p.diag.expect("diag capturé");
        assert!(diag.cmdline.contains("printf v9.9.9"), "cmdline = {}", diag.cmdline);
        assert!(diag.output.contains("v9.9.9"), "output = {}", diag.output);
        assert_eq!(p.version.as_deref(), Some("9.9.9"));
    }

    #[test]
    fn diag_sur_absent_montre_la_sonde() {
        if cfg!(target_os = "windows") {
            return;
        }
        let mut s = step();
        s.detect = Some("false".into()); // exit non-zéro → absent
        let p = detect_present_detailed(&s, Os::Darwin);
        assert_eq!(p.present, Some(false));
        assert!(p.diag.expect("diag").cmdline.contains("false"));
    }
}
