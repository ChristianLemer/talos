//! plan_facts.rs — what the app BELIEVES about every step's behaviour, and the slow threshold.
//!
//! ⭐ This module was `ladder.rs` until 2026-10-10. The ladder (three cumulative rungs —
//! ⚡ config only, 🧩 extensions, 📦 apps — filtering which actions one Apply runs) was
//! removed: applying PART of a desire built states the engine never reasons about, such as a
//! fresh config over a stale app, and nobody ever moved the slider. What survived is what
//! the ladder was calibrated WITH: the resolution of (collected × declared) facts onto the
//! plan, which the row labels and the estimate still read.
//!
//! ⚠️ The facts this reads are a BELIEF, not an observation: `bundles::resolve_facts`
//! lays the catalogue's declarations over what the fleet observed, and what comes out is
//! the very same `behaviour::Facts` type that `behaviour::merge_into` and
//! `behaviour_io::merge_and_write` accept. So feeding one of these values back toward the
//! share COMPILES, and would ratchet a declared `slow`'s 600s sentinel into a monotone
//! file permanently: a WRONG value, which is precisely what `behaviour.rs` promises no
//! write can produce. Merge only what a machine actually observed.

use crate::behaviour::{Facts, Record};
use crate::bundles::{resolve_facts, Step};
use crate::platform::Os;
use std::collections::BTreeMap;

/// What the app should BELIEVE about every step, index for index with the plan — the same
/// parallel-vector shape `AppState::last_seen` already uses.
///
/// Called ONCE at startup, beside the catalogue. Not per Apply: a behaviour file that
/// appears mid-session is not seen until the next launch, which is correct — the plan must
/// not shift under a running Apply. That is the exact mirror of the write side, which
/// accumulates in memory and flushes once at the end.
///
/// The lookup is by (catalogue id, `route/os`), because a behaviour belongs to the COUPLE:
/// `Amazon.AWSCLI` via winget on Windows elevates, `brew install awscli` does not. A step
/// with no route on this platform keys to `none/<os>`, which no real file writes — so it
/// gets `Facts::default()` rather than another route's facts. It also has no commands, so
/// it can never be an action anyway.
///
/// ⚠️ WHAT COMES OUT MUST NOT GO BACK TOWARD THE SHARE — see the module doc. It is a
/// belief, of the same type an observation has.
pub fn resolve_plan_facts(
    steps: &[Step],
    collected: &BTreeMap<String, Record>,
    os: Os,
) -> Vec<Facts> {
    steps
        .iter()
        .map(|s| {
            let at = crate::behaviour::key(s.route.as_deref().unwrap_or(""), os);
            let observed = collected
                .get(&s.id)
                .and_then(|rec| rec.get(&at))
                .copied()
                .unwrap_or_default();
            resolve_facts(&observed, &s.overrides)
        })
        .collect()
}

/// The measured duration at or above which a package is CLASSIFIED slow — the `slow` label
/// on its row.
///
/// ONE minute, and the reasoning matters more than the number: this is the boundary
/// between ☕ ("start it and walk away") and 🏗️ ("allow time"), which is a question about
/// a PERSON's patience, not about bytes. Past a minute at the screen you have already left
/// the room, so a minute is where waiting STOPS — not where it becomes painful. (C's
/// number, corrected down from a first draft of 120s for exactly that reason.)
///
/// ⚠️ It must stay strictly BELOW `bundles::DECLARED_SLOW_SECS` (600), the sentinel a
/// declared `slow: true` maps to when nothing has been measured. At or above it, every
/// hand-declared `slow` in the catalogue would go silently inert. A `const` block in the
/// tests below checks the inequality when the test target COMPILES — that check is the
/// whole reason the sentinel is `pub`.
///
/// ⚠️ AND THE COMPARISON IS AGAINST THE SHARED FILE, NOT A LOCAL ONE. `slow_secs` is the
/// WORST duration any machine ever saw, which is what makes the label mean the SAME thing
/// on every machine. The minutes shown to this user will come from a local last-seen file
/// instead (a later task) — so a package can legitimately be classified slow while
/// announcing "~20 s" here, because this machine's cache is warm. Measured example, from
/// Part A's real-click session: 7-Zip took 1 second on that Mac, its brew bottle already
/// cached, while a fresh machine behind a corporate firewall would take far longer — and that
/// far-longer number is the one that belongs in the shared classification. Two questions
/// with two answers, not a bug.
pub const SLOW_SECS: u64 = 60;

/// Does this package DRAG? The one place the threshold is applied, so nothing can drift.
///
/// `server::step_json` puts `slow` on the wire per row.
pub fn is_slow(f: &Facts) -> bool {
    f.slow_secs >= SLOW_SECS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behaviour::Facts;
    use crate::bundles::{Overrides, Posture};

    fn f(uac: bool, blocked: bool, secs: u64) -> Facts {
        Facts {
            uac,
            forbidden: blocked,
            slow_secs: secs,
        }
    }

    /// A minimal Step for the resolution tests: only `id`, `route` and `overrides`
    /// matter here, the rest are inert defaults.
    fn step(id: &str, route: Option<&str>, ov: Overrides) -> Step {
        Step {
            doctor: None,
            id: id.into(),
            bundle: String::new(),
            name: id.into(),
            description: String::new(),
            install: None,
            uninstall: None,
            upgrade: None,
            downgrade: None,
            route: route.map(|r| r.into()),
            system_id: None,
            detect: None,
            check: None,
            is_config: false,
            is_extension: false,
            version_regex: None,
            pin: None,
            scope: None,
            requires: Vec::new(),
            replaces: Vec::new(),
            posture: Posture::OptIn,
            categories: Vec::new(),
            overrides: ov,
            update_each_apply: false,
        }
    }

    #[test]
    fn the_slow_threshold_stays_strictly_below_the_declared_sentinel() {
        // `slow: true` in the catalogue maps to a SENTINEL duration (600s). If the
        // threshold ever climbed to or above it, every declared `slow: true` would go
        // silently inert. This is the comparison the Part A plan could only describe;
        // DECLARED_SLOW_SECS is `pub` so it can be made.
        //
        // A `const` block, not a plain `assert!`: both constants are compile-time, so
        // clippy's `assertions_on_constants` rejects the runtime form the plan wrote — and
        // it is right to. Being precise about what that buys, because it is easy to overstate:
        // this block lives inside `#[cfg(test)]`, so it is evaluated when the test target is
        // actually BUILT, and not in a release build. Check-mode is not enough — measured:
        // with SLOW_SECS mutated to 600, `cargo check --all-targets` and `cargo clippy
        // --all-targets -- -D warnings` both pass, while `cargo test --bins` fails to
        // compile. So what it buys is that the check no longer needs the test to RUN, not
        // that a lint pass would catch it.
        //
        // The plan's neighbouring claim that "no test would notice" needs the same care: this
        // test does notice, and a test asserting the sentinel against itself would not. The
        // real hazard was that the two constants lived in different modules with no comparison
        // between them at all, because this one was function-local.
        const {
            assert!(SLOW_SECS < crate::bundles::DECLARED_SLOW_SECS);
        }
        // The second half is a real runtime check and stays one: a declared `slow: true`
        // with nothing measured must actually READ as slow, which goes through
        // `resolve_facts` rather than through the two constants.
        let declared = crate::bundles::resolve_facts(
            &Facts::default(),
            &Overrides {
                uac: None,
                forbidden: None,
                slow: Some(true),
            },
        );
        assert!(is_slow(&declared), "a declared slow must classify as slow");
    }

    #[test]
    fn is_slow_reads_the_measured_seconds_not_a_declaration() {
        assert!(!is_slow(&f(false, false, 0)), "never measured is not slow");
        assert!(!is_slow(&f(false, false, SLOW_SECS - 1)));
        assert!(
            is_slow(&f(false, false, SLOW_SECS)),
            "the threshold is inclusive"
        );
        assert!(is_slow(&f(false, false, 9999)));
    }

    #[test]
    fn facts_are_read_for_this_route_and_os_only() {
        // A behaviour belongs to the (route, os) COUPLE. A record that only knows
        // about winget/windows must say NOTHING about the same package on brew/darwin
        // — otherwise a Mac would inherit Windows' elevation and show an "elevates" label for no
        // reason at all.
        let mut collected = std::collections::BTreeMap::new();
        let mut rec = crate::behaviour::Record::new();
        rec.insert("winget/windows".into(), f(true, false, 242));
        rec.insert("brew/darwin".into(), f(false, false, 38));
        collected.insert("aws-cli".to_string(), rec);

        let steps = vec![step("aws-cli", Some("brew"), Overrides::default())];
        let facts = resolve_plan_facts(&steps, &collected, Os::Darwin);
        assert_eq!(facts.len(), 1);
        assert!(
            !facts[0].uac,
            "the Windows elevation must not leak onto a Mac"
        );
        assert_eq!(
            facts[0].slow_secs, 38,
            "the brew/darwin duration, not winget's"
        );

        // Same steps, same record, a different os: `brew/windows` is not a key anything
        // writes, so nothing is known — and in particular the winget record is NOT
        // consulted just because the os now matches its half of the couple.
        let facts = resolve_plan_facts(&steps, &collected, Os::Windows);
        assert_eq!(
            facts[0].slow_secs, 0,
            "brew on Windows is not a key that exists"
        );
        assert!(!facts[0].uac);
    }

    #[test]
    fn a_catalogue_override_wins_over_what_the_fleet_observed() {
        // The escape hatch, reaching the plan for the first time. Until this task,
        // `resolve_facts` had no caller at all, so an override written in catalog/
        // changed nothing observable.
        let mut collected = std::collections::BTreeMap::new();
        let mut rec = crate::behaviour::Record::new();
        rec.insert("brew/darwin".into(), f(true, true, 900));
        collected.insert("rclone".to_string(), rec);

        let declared = Overrides {
            uac: Some(false),
            forbidden: Some(false),
            slow: None,
        };
        let steps = vec![step("rclone", Some("brew"), declared)];
        let facts = resolve_plan_facts(&steps, &collected, Os::Darwin);
        assert!(!facts[0].uac, "an explicit false in the catalogue wins");
        assert!(!facts[0].forbidden);
        assert_eq!(
            facts[0].slow_secs, 900,
            "not declared → the collected value stands"
        );
    }

    #[test]
    fn a_package_nobody_has_observed_gets_the_declaration_alone() {
        // A fresh machine. The seeds ARE the calibration: without this, its first
        // Apply would promise "nothing will interrupt you" and then hit a UAC prompt.
        let collected = std::collections::BTreeMap::new();
        let steps = vec![step(
            "7-zip",
            Some("winget"),
            Overrides {
                uac: Some(true),
                forbidden: None,
                slow: None,
            },
        )];
        let facts = resolve_plan_facts(&steps, &collected, Os::Windows);
        assert!(
            facts[0].uac,
            "the catalogue speaks when nothing was collected"
        );
        assert_eq!(facts[0].slow_secs, 0, "and invents no duration");
    }

    #[test]
    fn a_step_with_no_route_here_gets_nothing_rather_than_a_wrong_key() {
        // Notepad++ on a Mac: no route, so no commands, so no action — but it must
        // still produce a Facts, at the right index, without matching some other
        // route's record. `behaviour::key` maps "" to "none", which no real file uses.
        let mut collected = std::collections::BTreeMap::new();
        let mut rec = crate::behaviour::Record::new();
        rec.insert("winget/windows".into(), f(true, false, 5));
        collected.insert("notepad-plus-plus".to_string(), rec);

        let steps = vec![step("notepad-plus-plus", None, Overrides::default())];
        let facts = resolve_plan_facts(&steps, &collected, Os::Darwin);
        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0],
            Facts::default(),
            "no route → nothing known, not somebody else's facts"
        );
    }

    #[test]
    fn the_result_is_parallel_to_the_plan_index_for_index() {
        // The vector is indexed by step position, exactly like `last_seen`. A shorter
        // or reordered result would silently attribute one package's facts to another.
        // The three steps share a route and differ only in id and declaration, so a
        // lookup keyed on anything but the id would collapse them.
        let mut collected = std::collections::BTreeMap::new();
        let mut rec = crate::behaviour::Record::new();
        rec.insert("brew/darwin".into(), f(false, true, 0));
        collected.insert("c".to_string(), rec);
        let steps = vec![
            step("a", Some("brew"), Overrides::default()),
            step(
                "b",
                Some("brew"),
                Overrides {
                    uac: Some(true),
                    forbidden: None,
                    slow: None,
                },
            ),
            step("c", Some("brew"), Overrides::default()),
        ];
        let facts = resolve_plan_facts(&steps, &collected, Os::Darwin);
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[0], Facts::default(), "index 0 knows nothing");
        assert!(facts[1].uac, "index 1 is the one that declared it");
        assert!(!facts[1].forbidden, "and it did not inherit index 2's 403");
        assert!(!facts[2].uac);
        assert!(facts[2].forbidden, "index 2 is the one the fleet observed");
    }
}
