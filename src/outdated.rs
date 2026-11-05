// A machine-wide scan via the native manager.
//
// ⚠️ It used to be best-effort in the WORST sense: no manager, a failed command
// and an output the parser could not read all returned the same empty map, which
// is indistinguishable from "nothing is outdated". So a machine whose package
// source hiccupped showed a screen full of reassuring green rows and never said
// why — a silence field-hit once already (an unstable winget source printing
// "Failed in attempting to update the source", no table, empty map, no trace).
//
// The scan now reports WHY it is empty. The safe direction is unchanged — an
// unreadable scan still marks nothing outdated — but the caller can say so
// instead of implying everything is current.
use std::collections::HashMap;

use crate::managers::{native_manager, Outdated};
use crate::platform::{shell_probe, Os};

/// Why a scan produced nothing. Carries the evidence, because "it failed" alone
/// cannot be acted on: the first line of real output is what identifies the case.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanFailure {
    /// This OS has no native package manager — nothing to ask, not an error.
    NoManager,
    /// The command could not be run at all (absent binary, spawn refused).
    CommandFailed(String),
    /// The command ran and failed. Carries its exit code and first output line.
    NonZeroExit { code: Option<i32>, head: String },
    /// The command ran, said something, and the parser found no header it knows.
    /// The likeliest real-world case, and the one that used to be invisible.
    Unparseable(String),
}

impl ScanFailure {
    /// One line for the operator, naming the cause and quoting the machine.
    pub fn message(&self) -> String {
        match self {
            Self::NoManager => "no native package manager on this OS".into(),
            Self::CommandFailed(e) => format!("the upgrade scan could not run: {e}"),
            Self::NonZeroExit { code, head } => match code {
                Some(c) => format!("the upgrade scan exited {c}: {head}"),
                None => format!("the upgrade scan was terminated: {head}"),
            },
            Self::Unparseable(head) => {
                format!("the upgrade scan output could not be read: {head}")
            }
        }
    }
}

/// The first non-empty line of an output, trimmed — enough to identify the case
/// in a report without dumping a whole table into the UI.
fn head_of(s: &str) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(no output)");
    if line.chars().count() > 160 {
        let cut: String = line.chars().take(160).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// Scan the machine for upgradable packages.
///
/// `Ok(map)` — the scan was understood. An EMPTY map here genuinely means
/// "nothing is outdated", which is the distinction this signature exists for.
/// `Err(reason)` — nothing could be concluded; the caller must not present the
/// result as "everything is current".
pub fn scan_outdated(os: Os) -> Result<HashMap<String, Outdated>, ScanFailure> {
    let Some(mgr) = native_manager(os) else {
        return Err(ScanFailure::NoManager);
    };
    let probe = shell_probe(os, &mgr.outdated_scan_command());
    let out = crate::platform::quiet_command(&probe.cmd)
        .args(&probe.args)
        .output()
        .map_err(|e| ScanFailure::CommandFailed(e.to_string()))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed = mgr.parse_outdated(&stdout);

    // Order matters: a non-zero exit that STILL produced a readable table is
    // usable (winget warns about its source and prints the table anyway), so the
    // parse result is consulted before the exit code is held against it.
    if !parsed.is_empty() {
        return Ok(parsed);
    }
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let head = if stderr.trim().is_empty() {
            head_of(&stdout)
        } else {
            head_of(&stderr)
        };
        return Err(ScanFailure::NonZeroExit {
            code: out.status.code(),
            head,
        });
    }
    // Exit 0, no entries. Either the machine is genuinely current, or the output
    // is a shape the parser does not know. `is_recognised` decides which, so
    // "everything up to date" is only ever claimed when it was actually read.
    if mgr.is_recognised(&stdout) {
        Ok(parsed)
    } else {
        Err(ScanFailure::Unparseable(head_of(&stdout)))
    }
}

/// A package's systemId (lowercased) → its outdated entry, or None.
///
/// brew's `outdated --json=v2` names a formula by its full name but a CASK by its short token
/// (brew's `Cask#outdated_info`), so a tap cask (`stablyai/orca/orca`) is found under `orca`.
/// Only a cask entry is taken that way: brew keeps one Caskroom/<token> per machine, so the
/// outdated `orca` IS the installed one, and the row's presence (the full-name listing) has
/// already said whose it is. A formula entry under the short name is another formula.
pub fn outdated_for<'a>(
    system_id: Option<&str>,
    scan: &'a HashMap<String, Outdated>,
) -> Option<&'a Outdated> {
    let id = system_id?.to_lowercase();
    scan.get(&id).or_else(|| {
        crate::managers::tap_short_name(&id)
            .and_then(|short| scan.get(short))
            .filter(|o| o.is_cask)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_cask_is_found_by_its_token_a_formula_is_not() {
        let od = |is_cask| Outdated {
            current: "1.4.237".into(),
            available: "1.4.249".into(),
            is_cask,
        };
        // brew's outdated JSON: a cask under its short token, a formula under its full name.
        let mut scan = HashMap::new();
        scan.insert("orca".to_string(), od(true));
        scan.insert("charmbracelet/tap/gum".to_string(), od(false));
        scan.insert("jq".to_string(), od(false));
        assert!(outdated_for(Some("stablyai/orca/orca"), &scan).is_some());
        assert!(outdated_for(Some("charmbracelet/tap/gum"), &scan).is_some());
        assert!(outdated_for(Some("orca"), &scan).is_some());
        // A formula found only by short name is another formula.
        assert!(outdated_for(Some("someone/tap/jq"), &scan).is_none());
        assert!(outdated_for(None, &scan).is_none());
    }

    #[test]
    fn head_of_takes_the_first_meaningful_line() {
        assert_eq!(
            head_of("\n\n  Failed to update source  \nrest"),
            "Failed to update source"
        );
        assert_eq!(head_of("   "), "(no output)");
    }

    #[test]
    fn head_of_caps_a_runaway_line() {
        let long = "x".repeat(400);
        let h = head_of(&long);
        assert_eq!(h.chars().count(), 161, "160 chars plus the ellipsis");
        assert!(h.ends_with('…'));
    }

    #[test]
    fn every_failure_names_its_cause() {
        // The operator-facing text must identify the case, not just say "error".
        assert!(ScanFailure::NoManager
            .message()
            .contains("no native package manager"));
        assert!(ScanFailure::CommandFailed("nope".into())
            .message()
            .contains("could not run"));
        assert!(ScanFailure::NonZeroExit {
            code: Some(1),
            head: "source broken".into()
        }
        .message()
        .contains("exited 1"));
        // ⭐ The case that used to be a silent empty map.
        assert!(ScanFailure::Unparseable("gibberish".into())
            .message()
            .contains("could not be read"));
    }

    #[test]
    fn a_terminated_scan_reads_as_terminated_not_as_exit_zero() {
        let m = ScanFailure::NonZeroExit {
            code: None,
            head: "killed".into(),
        }
        .message();
        assert!(m.contains("terminated"), "{m}");
    }
}
