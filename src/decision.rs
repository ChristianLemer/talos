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

/// Installed vs PIN, at the pin's own precision: the installed version is cut to as many
/// segments as the author wrote, so `2.50.1.windows.1` IS the pin "2.50.1" and a pin of
/// "1.8" is any 1.8.x. Since 2026-10-10 the Apply batches a downgrade (an uninstall +
/// reinstall), so a suffix read as "above the pin" would reinstall the package on every
/// Apply and never converge. Twin of `version.js::compareToPin`.
pub fn compare_to_pin(installed: &str, pin: &str) -> i32 {
    let n = pin.split('.').count();
    let cut: Vec<&str> = installed.split('.').take(n).collect();
    compare_versions(&cut.join("."), pin)
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
    /// What the manager announces as the newest version, "" when it announces nothing.
    pub available: &'a str,
    /// The package declares `version: self` AND its own `update:` command.
    pub can_self_update: bool,
    /// ... AND `update-on: apply`: that command runs on every Apply, behind or not.
    pub update_each_apply: bool,
}

/// The action an Apply would take — or None. The rule the server EXECUTES and
/// the front previews (single source). A pin reframes "outdated": the
/// reference becomes the pin, not "latest".
///   present desired & absent            → install
///   present desired & present & <pin    → upgrade (to the pin)
///   present desired & present & >pin    → downgrade (an uninstall + reinstall, batched)
///   present desired & present & stale (no pin) → upgrade (latest)
///   absent desired  & present & removable → uninstall
pub fn action_for(desired: Desired, f: &MachineFacts) -> Option<Action> {
    if desired == Desired::Present && !f.present {
        return Some(Action::Install);
    }
    if desired == Desired::Present && f.present {
        if let Some(pin) = f.pin.filter(|p| !p.is_empty()) {
            // ⭐ THE KEYWORDS FIRST, because they are statements about POLICY, not versions
            // to converge on — and `compare_versions` would happily read them as 0.0.0.
            // MEASURED: `compare_versions("2.50.1", "pending") == 1`, i.e. "installed is above
            // the pin" → Downgrade → an `uninstall && install pkg@pending` that the Apply
            // batches (since 2026-10-10). So the order of these checks is load-bearing, not
            // stylistic.
            //
            // `pending` = nobody has arbitrated this version yet. Present ⇒ do nothing, and
            // `outdated` is deliberately IGNORED: the gap is still emitted to the front
            // (`emit_outdated_if` runs regardless) and the row shows it greyed, so this is a
            // HOLD, not a blindfold. To take the update, the author edits the pin.
            if pin.eq_ignore_ascii_case("pending") {
                return None;
            }
            // `self` = the application updates itself. The manager never upgrades it; the
            // package's own `update:` command does, when the version the tool REPORTS is behind
            // what the manager announces — its receipt is stale by construction, since the
            // tool rewrote itself behind it. Without `update:`, nothing: not a decision left
            // pending, a decision to leave it to the application.
            if pin.eq_ignore_ascii_case("self") {
                // `update-on: apply`: the command itself knows whether there is anything to
                // do, so no announcement is needed — nor a version read.
                if f.can_self_update && f.update_each_apply {
                    return Some(Action::Upgrade);
                }
                // A leading `v` is a spelling, not a segment: winget writes `v1.0.102`.
                fn bare(v: &str) -> &str {
                    v.strip_prefix(['v', 'V']).unwrap_or(v)
                }
                // An unread version is no evidence: no verdict could follow the update, so it
                // would run on every Apply. An outdated row naming no version: the scan decides.
                let behind = !f.installed_version.is_empty()
                    && (f.available.is_empty()
                        || compare_versions(bare(f.installed_version), bare(f.available)) < 0);
                return (f.can_self_update && f.outdated && behind).then_some(Action::Upgrade);
            }
            // `latest` = the author DECIDED to take the newest. Same effect as declaring
            // nothing, and not redundant with it: a catalogue meant to be copied from must be
            // able to say "yes, newest here" rather than merely omit the field.
            if !pin.eq_ignore_ascii_case("latest") {
                return match compare_to_pin(f.installed_version, pin) {
                    c if c < 0 => Some(Action::Upgrade),
                    c if c > 0 => Some(Action::Downgrade),
                    _ => None, // at the pin → satisfied
                };
            }
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
            available: "",
            can_self_update: false,
            update_each_apply: false,
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
            available: "",
            can_self_update: false,
            update_each_apply: false,
        };
        assert_eq!(action_for(Desired::Present, &f), Some(Action::Upgrade));
    }

    #[test]
    fn a_pin_is_matched_to_its_own_precision_so_a_suffix_never_reinstalls() {
        // Found at the final review of the batched downgrade: Git on Windows reports
        // `2.50.1.windows.1`, which compares as [2,50,1,0,1] > [2,50,1] — a pin of "2.50.1" read
        // as "installed ABOVE the pin", so every Apply uninstalled and reinstalled Git, and the
        // probe read the same suffix afterwards: it could never converge. A pin is matched to
        // the precision its author wrote.
        assert_eq!(compare_to_pin("2.50.1.windows.1", "2.50.1"), 0);
        assert_eq!(
            compare_to_pin("1.8.2", "1.8"),
            0,
            "a pin of 1.8 is any 1.8.x"
        );
        assert_eq!(compare_to_pin("2.51.0", "2.50.1"), 1);
        assert_eq!(
            compare_to_pin("2.50", "2.50.1"),
            -1,
            "a shorter install still pads"
        );
        let f = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: true,
            pin: Some("2.50.1"),
            installed_version: "2.50.1.windows.1",
            available: "",
            can_self_update: false,
            update_each_apply: false,
        };
        assert_eq!(
            action_for(Desired::Present, &f),
            None,
            "at the pin, never a reinstall"
        );
    }

    #[test]
    fn action_pin_below_upgrades_above_downgrades() {
        let below = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: false,
            pin: Some("2.0"),
            installed_version: "1.0",
            available: "",
            can_self_update: false,
            update_each_apply: false,
        };
        assert_eq!(action_for(Desired::Present, &below), Some(Action::Upgrade));
        let above = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: false,
            pin: Some("2.0"),
            installed_version: "3.0",
            available: "",
            can_self_update: false,
            update_each_apply: false,
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
            available: "",
            can_self_update: false,
            update_each_apply: false,
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
            available: "",
            can_self_update: false,
            update_each_apply: false,
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
            available: "",
            can_self_update: false,
            update_each_apply: false,
        };
        assert_eq!(action_for(Desired::Absent, &not), None);
    }

    #[test]
    fn the_two_keywords_resolve_as_a_table() {
        // ⭐ ONE fixture, both sides. The same table lives in test/decision.test.mjs, because
        // `action_for` exists TWICE (here and in public/decision.js) — one rule, two callers. The
        // twins have drifted before; feeding them the same rows is the cheapest guard there is.
        //
        // Columns: what, pin, present, outdated, installed → expected action.
        type Row = (
            &'static str,
            Option<&'static str>,
            bool,
            bool,
            &'static str,
            Option<Action>,
        );
        let cases: &[Row] = &[
            // `pending` + present → NOTHING, and `outdated` is ignored. That is the whole point:
            // the gap stays VISIBLE (the row shows it greyed) but the batch does not act.
            (
                "pending, present, newer exists",
                Some("pending"),
                true,
                true,
                "1.0",
                None,
            ),
            (
                "pending, present, current",
                Some("pending"),
                true,
                false,
                "1.0",
                None,
            ),
            // ⭐ `pending` + ABSENT → install. There is no "held version" of a thing that is not
            // there, and a fresh machine must still be equippable.
            (
                "pending, absent",
                Some("pending"),
                false,
                false,
                "",
                Some(Action::Install),
            ),
            // `latest` behaves exactly as today's silence — the difference is that it was DECIDED.
            (
                "latest, present, newer exists",
                Some("latest"),
                true,
                true,
                "1.0",
                Some(Action::Upgrade),
            ),
            (
                "latest, present, current",
                Some("latest"),
                true,
                false,
                "1.0",
                None,
            ),
            (
                "latest, absent",
                Some("latest"),
                false,
                false,
                "",
                Some(Action::Install),
            ),
            // Nothing declared: unchanged.
            (
                "none, present, newer exists",
                None,
                true,
                true,
                "1.0",
                Some(Action::Upgrade),
            ),
            // An exact pin: unchanged, and pinned here so the keywords cannot break it.
            (
                "exact, below",
                Some("2.0"),
                true,
                false,
                "1.0",
                Some(Action::Upgrade),
            ),
            ("exact, equal", Some("2.0"), true, false, "2.0", None),
            (
                "exact, above",
                Some("2.0"),
                true,
                false,
                "3.0",
                Some(Action::Downgrade),
            ),
        ];
        for (what, pin, present, outdated, installed, expected) in cases {
            let f = MachineFacts {
                present: *present,
                outdated: *outdated,
                can_uninstall: true,
                pin: *pin,
                installed_version: installed,
                available: "",
                can_self_update: false,
                update_each_apply: false,
            };
            assert_eq!(action_for(Desired::Present, &f), *expected, "{what}");
        }
    }

    #[test]
    fn a_keyword_is_never_compared_as_a_version() {
        // ⚠️ The defect this closes, pinned as a FACT rather than as prose. `compare_versions`
        // takes the leading digits of each segment, so a word has none and reads as 0.0.0 —
        // "installed is ABOVE the pin" → Downgrade → an `uninstall && install pkg@word` that
        // the Apply batches — a reinstall of every such package on every Apply.
        //
        // Recording the measurement here is what stops someone "simplifying" the keyword branch
        // back into the comparison.
        assert_eq!(
            compare_versions("2.50.1", "current"),
            1,
            "a word reads as 0.0.0"
        );
        assert_eq!(
            compare_versions("2.50.1", "pending"),
            1,
            "…and so would `pending`"
        );
        let f = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: true,
            pin: Some("pending"),
            installed_version: "2.50.1",
            available: "",
            can_self_update: false,
            update_each_apply: false,
        };
        assert_eq!(action_for(Desired::Present, &f), None, "never a downgrade");
    }

    #[test]
    fn a_tool_updated_on_every_apply_runs_its_update_whenever_it_is_present() {
        // `update-on: apply`: no announcement needed, an unread version included — the command
        // itself knows whether there is anything to do.
        let base = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: true,
            pin: Some("self"),
            installed_version: "2.1.308",
            available: "",
            can_self_update: true,
            update_each_apply: true,
        };
        assert_eq!(action_for(Desired::Present, &base), Some(Action::Upgrade));
        let unread = MachineFacts {
            installed_version: "",
            ..base
        };
        assert_eq!(action_for(Desired::Present, &unread), Some(Action::Upgrade));
        let absent = MachineFacts {
            present: false,
            ..base
        };
        assert_eq!(action_for(Desired::Present, &absent), Some(Action::Install));
        // Nothing to run → nothing, whatever the word says.
        let no_update = MachineFacts {
            can_self_update: false,
            update_each_apply: false,
            ..base
        };
        assert_eq!(action_for(Desired::Present, &no_update), None);
        // Wanted absent: still an uninstall, never an update.
        assert_eq!(action_for(Desired::Absent, &base), Some(Action::Uninstall));
    }

    #[test]
    fn self_resolves_as_a_table() {
        // `self` — the application updates itself. The same rows live in
        // test/decision.test.mjs: one rule, two callers.
        //
        // Columns: what, present, outdated, installed, available, can_self_update → action.
        type Row = (
            &'static str,
            bool,
            bool,
            &'static str,
            &'static str,
            bool,
            Option<Action>,
        );
        let cases: &[Row] = &[
            // Absent → the manager installs it, as for every other word: a repair.
            ("absent", false, false, "", "", false, Some(Action::Install)),
            (
                "absent, with update:",
                false,
                false,
                "",
                "",
                true,
                Some(Action::Install),
            ),
            // No `update:` → left to the application's own mechanism, newer or not. This is
            // what stops winget re-running VS Code's upgrade on every Apply.
            (
                "present, newer announced",
                true,
                true,
                "1.0",
                "1.1",
                false,
                None,
            ),
            ("present, current", true, false, "1.0", "", false, None),
            // `update:` → run when the version the tool REPORTS is behind what the manager
            // announces. The manager's own receipt does not count: the tool rewrote itself
            // behind its back, so only `detect:` knows what is there.
            (
                "update:, behind",
                true,
                true,
                "1.0",
                "1.1",
                true,
                Some(Action::Upgrade),
            ),
            (
                "update:, the receipt lags, the tool is current",
                true,
                true,
                "1.1",
                "1.1",
                true,
                None,
            ),
            (
                "update:, the tool is ahead",
                true,
                true,
                "1.2",
                "1.1",
                true,
                None,
            ),
            (
                "update:, nothing announced",
                true,
                false,
                "1.0",
                "",
                true,
                None,
            ),
            // A version the probe could not read is no evidence the tool is behind — and no
            // verdict could follow, so `update:` would run on every Apply. Nothing.
            ("update:, version unread", true, true, "", "1.1", true, None),
            // The scan says outdated but names no version: the scan decides.
            (
                "update:, outdated with no version named",
                true,
                true,
                "1.0",
                "",
                true,
                Some(Action::Upgrade),
            ),
            // MEASURED (Windows VM, 2026-10-22): winget lists Copilot CLI as `v1.0.96` →
            // `v1.0.102`. A leading `v` read as a number is 0: the tool would never be behind.
            (
                "update:, a v-prefixed announcement",
                true,
                true,
                "1.0.96",
                "v1.0.102",
                true,
                Some(Action::Upgrade),
            ),
            (
                "update:, v-prefixed and current",
                true,
                true,
                "1.0.102",
                "v1.0.102",
                true,
                None,
            ),
        ];
        for (what, present, outdated, installed, available, can, expected) in cases {
            let f = MachineFacts {
                present: *present,
                outdated: *outdated,
                can_uninstall: true,
                pin: Some("self"),
                installed_version: installed,
                available,
                can_self_update: *can,
                update_each_apply: false,
            };
            assert_eq!(action_for(Desired::Present, &f), *expected, "{what}");
        }
        // Wanted absent: the word says nothing about whether it should be there.
        let f = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: true,
            pin: Some("Self"),
            installed_version: "1.0",
            available: "",
            can_self_update: true,
            update_each_apply: false,
        };
        assert_eq!(action_for(Desired::Absent, &f), Some(Action::Uninstall));
    }

    #[test]
    fn a_pending_row_wanted_absent_is_still_uninstalled() {
        // A hold on the VERSION says nothing about whether the package should be there. The user's
        // ✕ is a different axis, and it must keep working.
        let f = MachineFacts {
            present: true,
            outdated: true,
            can_uninstall: true,
            pin: Some("pending"),
            installed_version: "1.0",
            available: "",
            can_self_update: false,
            update_each_apply: false,
        };
        assert_eq!(action_for(Desired::Absent, &f), Some(Action::Uninstall));
    }

    #[test]
    fn latest_and_an_undeclared_version_are_indistinguishable() {
        // ⭐ `latest` states an INTENTION where silence states nothing — but they must RESOLVE
        // identically, because `latest` deliberately falls through to the same `outdated` check
        // instead of answering for itself. That is the "exactly one place decides newest ⇒
        // upgrade" shape.
        //
        // ⚠️ WHAT THIS CAN AND CANNOT PROVE. "One code path" is structural: no black-box test
        // can see it, and a test asserting merely that both return Upgrade would pass just as
        // happily against two duplicated branches — a guard in appearance only. What IS
        // observable is EQUIVALENCE across every combination, and a duplicated check that ever
        // drifts fails here on the row where it drifted. The structural half rests on review.
        for present in [true, false] {
            for outdated in [true, false] {
                for installed in ["", "1.0"] {
                    for desired in [Desired::Present, Desired::Absent] {
                        let facts = |pin: Option<&'static str>| MachineFacts {
                            present,
                            outdated,
                            can_uninstall: true,
                            pin,
                            installed_version: installed,
                            available: "",
                            can_self_update: false,
                            update_each_apply: false,
                        };
                        assert_eq!(
                            action_for(desired, &facts(Some("latest"))),
                            action_for(desired, &facts(None)),
                            "latest diverged from silence at present={present} \
                             outdated={outdated} installed={installed:?}"
                        );
                    }
                }
            }
        }
    }
}
