// Port of decision.js + version.js — Talos's PURE rule, SINGLE SOURCE (at the
// core, multi-frontend decision: clients display, they do not decide).
// Two questions: what is the DESIRED state (posture + toggle), and what ACTION
// an Apply would take (desire × machine reality × pin).

/// Compares two dotted versions, NUMERICALLY segment by segment (2.10 > 2.9).
/// Segment = numeric prefix (parseInt: "0-beta" → 0). Missing segment = 0.
/// Returns -1 (a<b) / 0 (==) / 1 (a>b). Not full semver (see talos-version-pin).
pub fn compare_versions(a: &str, b: &str) -> i32 {
    let seg = |v: &str| -> Vec<i64> {
        v.split('.')
            .map(|s| {
                let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<i64>().unwrap_or(0)
            })
            .collect()
    };
    let (as_, bs) = (seg(a), seg(b));
    let n = as_.len().max(bs.len());
    for i in 0..n {
        let x = as_.get(i).copied().unwrap_or(0);
        let y = bs.get(i).copied().unwrap_or(0);
        if x < y {
            return -1;
        }
        if x > y {
            return 1;
        }
    }
    0
}

// NOTE — the "intention resolution" half (posture → desire) is NOT here anymore.
// In the bundle-driven model the FRONT resolves the desired state (bundles pull +
// manual toggles, see public/decision.js + model.js) and sends on/off to the
// server, which builds `Desired` directly from those sets (server.rs). The old
// Rust twin (`posture_default_in`/`is_locked`/`toggle_in`/`desired_state`)
// encoded the SUPERSEDED posture-default model (mandatory locked-in, opt-in
// default-out) and its tests blessed it → false-green. Removed 2026-07-21 (see
// the Windows review model note, §2). If the server ever
// takes over desire resolution ("path B"), re-derive it from the CURRENT model
// (decision.js): only `forbidden` locks; nothing is wanted until pulled.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Desired {
    Present,
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Install,
    Uninstall,
    Upgrade,
    Downgrade,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Uninstall => "uninstall",
            Action::Upgrade => "upgrade",
            Action::Downgrade => "downgrade",
        }
    }
}

pub struct MachineFacts<'a> {
    pub present: bool,
    pub outdated: bool,
    pub can_uninstall: bool,
    pub pin: Option<&'a str>,
    pub installed_version: &'a str,
}

/// The action an Apply would take — or None. The rule the server EXECUTES and
/// the front previews (single source). A pin reframes "outdated": the
/// reference becomes the pin, not "latest".
///   present desired & absent            → install
///   present desired & present & <pin    → upgrade (to the pin)
///   present desired & present & >pin    → downgrade (MANUAL, filtered out of Apply)
///   present desired & present & stale (no pin) → upgrade (latest)
///   absent desired  & present & removable → uninstall
pub fn action_for(desired: Desired, f: &MachineFacts) -> Option<Action> {
    if desired == Desired::Present && !f.present {
        return Some(Action::Install);
    }
    if desired == Desired::Present && f.present {
        if let Some(pin) = f.pin.filter(|p| !p.is_empty()) {
            return match compare_versions(f.installed_version, pin) {
                c if c < 0 => Some(Action::Upgrade),
                c if c > 0 => Some(Action::Downgrade),
                _ => None, // at the pin → satisfied
            };
        }
        if f.outdated {
            return Some(Action::Upgrade);
        }
    }
    if desired == Desired::Absent && f.present && f.can_uninstall {
        return Some(Action::Uninstall);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_numeric_not_string() {
        assert_eq!(compare_versions("2.10", "2.9"), 1); // numeric, not string
        assert_eq!(compare_versions("1.8", "1.8.0"), 0); // missing segment = 0
        assert_eq!(compare_versions("1.0", "1.0.1"), -1);
        assert_eq!(compare_versions("0-beta", "0"), 0); // pre-release suffix ignored
    }

    #[test]
    fn action_install_when_absent() {
        let f = MachineFacts {
            present: false,
            outdated: false,
            can_uninstall: false,
            pin: None,
            installed_version: "",
        };
        assert_eq!(action_for(Desired::Present, &f), Some(Action::Install));
    }

    #[test]
    fn action_upgrade_when_stale() {
        let f = MachineFacts {
            present: true,
            outdated: true,
            can_uninstall: false,
            pin: None,
            installed_version: "1.0",
        };
        assert_eq!(action_for(Desired::Present, &f), Some(Action::Upgrade));
    }

    #[test]
    fn action_pin_below_upgrades_above_downgrades() {
        let below = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: false,
            pin: Some("2.0"),
            installed_version: "1.0",
        };
        assert_eq!(action_for(Desired::Present, &below), Some(Action::Upgrade));
        let above = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: false,
            pin: Some("2.0"),
            installed_version: "3.0",
        };
        assert_eq!(
            action_for(Desired::Present, &above),
            Some(Action::Downgrade)
        );
        let at = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: false,
            pin: Some("2.0"),
            installed_version: "2.0",
        };
        assert_eq!(action_for(Desired::Present, &at), None);
    }

    #[test]
    fn action_uninstall_only_when_removable() {
        let removable = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: true,
            pin: None,
            installed_version: "",
        };
        assert_eq!(
            action_for(Desired::Absent, &removable),
            Some(Action::Uninstall)
        );
        let not = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: false,
            pin: None,
            installed_version: "",
        };
        assert_eq!(action_for(Desired::Absent, &not), None);
    }
}
