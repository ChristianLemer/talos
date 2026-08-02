//! ladder.rs — how FAR an Apply goes, and the facts that calibrate it.
//!
//! Apply used to be all-or-nothing. The ladder is one control with five CUMULATIVE
//! rungs, so a user can ask for "the fast harmless part now" or "everything that will
//! not interrupt me" — C's framing, and the reason the behaviour telemetry exists at all.
//!
//! This module is PURE: the rungs, the filter, and the resolution of (collected ×
//! declared) onto a plan. No IO, no state, no socket. The wiring is server.rs's —
//! `resolve_plan_facts` is called there at startup, while the FILTER is still unwired; see
//! the per-item `allow(dead_code)` notes below for which task removes each one.
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

/// The measured duration at or above which a package is CLASSIFIED slow — "allow time",
/// the last rung.
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
/// WORST duration any machine ever saw, which is what makes the rung mean the SAME thing
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
/// No `allow` of its own, and no longer needs one to be borrowed: `server::handle_socket`
/// puts `slow` on the wire per row through it, so this is a real root now and `SLOW_SECS`
/// lives through it. (It used to be kept alive only by `rung_allows`'s allow — measured
/// again after that wiring: stripping that allow reports two warnings, not four.)
pub fn is_slow(f: &Facts) -> bool {
    f.slow_secs >= SLOW_SECS
}

/// How far an Apply goes. FIVE CUMULATIVE rungs — each contains the previous, which is
/// why this is one control and not four checkboxes (C's design; the aspects are not
/// independent).
///
/// The emojis answer **"do I have time right now?"**, deliberately NOT "is this better?".
/// A satisfaction ramp (🙁→😀) would assert that Everything is the good end, and it is
/// not: rung 0 is the only one that CANNOT fail on the network, and on a Tuesday morning
/// the right answer is usually rung 2. ☕ vs 👀 carries the real difference between rungs
/// 2 and 3 — "you may leave" vs "you must stay" — better than any word would. The emojis
/// themselves will live in `public/ladder.js`; only the names are needed here, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// ⚡ config-atoms only. Purely local: both shipped atoms run `nu "{dir}/….nu" apply`,
    /// no network at all. That is a STRONGER promise than "fast", and it is what earns
    /// this rung its place below "add missing" — a fresh install DOWNLOADS, so "installs"
    /// and "fast" contradict each other.
    ConfigOnly,
    /// 📦 things arrive: every `install`, no `upgrade` at all.
    AddMissing,
    /// ☕ start it and walk away: upgrades with no `uac`, no `403`, not slow.
    Unattended,
    /// 👀 Windows will ask for your hand: also the ones flagged `uac` or `403`.
    StayNearby,
    /// 🏗️ the big one: also the slow ones. Nothing excluded.
    Everything,
}

impl Default for Rung {
    /// ABSENT MEANS EVERYTHING. An older front, or any client that omits the field, must
    /// get today's behaviour — doing silently LESS than the user asked for is the worse
    /// direction of error, and a rung is a preference rather than a safety property.
    ///
    /// Deliberately the OPPOSITE stance from `unmanaged`, whose omission the server
    /// refuses to trust: scope is a safety property, a rung is a preference.
    fn default() -> Self {
        Rung::Everything
    }
}

impl Rung {
    /// From the wire. Out of range → Everything, for the reason on `Default`.
    // Task 4 calls this from server.rs's `apply` arm; until then only the test does, and a
    // #[cfg(test)] caller does not keep a non-test build's item alive. MEASURED: this allow
    // also keeps four of the five variants CONSTRUCTED — stripping it alone reports two
    // warnings. (`Everything` needs no help: `Default` constructs it.)
    #[allow(dead_code)]
    pub fn from_wire(n: u64) -> Rung {
        match n {
            0 => Rung::ConfigOnly,
            1 => Rung::AddMissing,
            2 => Rung::Unattended,
            3 => Rung::StayNearby,
            _ => Rung::Everything,
        }
    }
    /// For the log line, so an Apply says how far it was told to go.
    // Task 4 puts this in `apply_diff`'s log line. Same test-only situation as `from_wire`.
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Rung::ConfigOnly => "config-only",
            Rung::AddMissing => "add-missing",
            Rung::Unattended => "unattended",
            Rung::StayNearby => "stay-nearby",
            Rung::Everything => "everything",
        }
    }
    fn level(&self) -> u8 {
        match self {
            Rung::ConfigOnly => 0,
            Rung::AddMissing => 1,
            Rung::Unattended => 2,
            Rung::StayNearby => 3,
            Rung::Everything => 4,
        }
    }
}

/// May this action run at this rung? THE rule, pure and total.
///
/// Kept a free function of four plain values — no `Step`, no `AppState`, no socket — so the
/// cumulative property can be asserted as a TABLE (one row per candidate, one column per
/// rung) rather than described. The spec asked for exactly that: the property should be
/// visible in the fixture, not claimed in prose.
///
/// ⚠️ `public/ladder.js` will hold a TWIN of this function, and the two must then stay in
/// lockstep — the same relation `model.js`'s `AUTO_ACTS` already has with the server's
/// `Install|Uninstall|Upgrade` filter. The front will need it ONLY to show per-rung counts;
/// the filter that actually governs the plan is this one. A front-only guard would be
/// cosmetic (`row_action` never sees the wire lists — the Git-hazard lesson), so the
/// duplication runs in the safe direction: if the twin drifts, a count is wrong, never an
/// action.
// Task 4 calls this in `visual_plan`'s build loop, which is what makes the ladder govern
// anything at all. MEASURED, after the facts reached the wire: this allow additionally keeps
// `Rung::level` alive — stripping it alone now reports TWO warnings (`level` and this), where
// it reported four before. `is_slow` and `SLOW_SECS` no longer hang off it: `steps_json`
// calls `is_slow` per row, so they stand on their own. It does NOT keep the enum's variants
// alive; `from_wire` is what constructs those.
#[allow(dead_code)]
pub fn rung_allows(
    rung: Rung,
    action: crate::decision::Action,
    is_config: bool,
    f: &Facts,
) -> bool {
    use crate::decision::Action;
    match action {
        // NOT ON THE LADDER, at any rung. Removing a package is driven by the user's ✕,
        // not by how much time they have. The ladder governs how far to GO; a veto is
        // honoured or it is not.
        Action::Uninstall => true,
        // Never batched by Apply at all (apply_diff matches Install|Uninstall|Upgrade), so
        // this arm is unreachable today. Answered so the function is total, and pinned by a
        // test so "unreachable" cannot quietly become "allowed".
        Action::Downgrade => false,
        // A config-atom is admitted from the first rung. Its action is always Install (a
        // `run:` route has no upgrade path), so this covers it whole.
        Action::Install if is_config => true,
        Action::Install => rung.level() >= 1,
        Action::Upgrade => {
            let needed = if is_slow(f) {
                4 // slow DOMINATES: the spec's rung 3 excludes "the slow ones", full stop
            } else if f.uac || f.forbidden {
                3 // it will want your hand, or a page unblocked
            } else {
                2 // start it and walk away
            };
            rung.level() >= needed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behaviour::Facts;
    use crate::bundles::{Overrides, Posture};
    use crate::decision::Action;

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
            version_regex: None,
            pin: None,
            requires: Vec::new(),
            posture: Posture::OptIn,
            categories: Vec::new(),
            overrides: ov,
        }
    }

    #[test]
    fn the_five_rungs_are_cumulative_and_the_table_shows_it() {
        // ONE row per candidate, ONE column per rung. The cumulative property is the
        // SHAPE of this fixture — every row is a prefix of `false`s followed by `true`s —
        // and reading it is the proof. An assertion in prose ("rung 3 contains rung 2")
        // would be a claim; this is a fixture that fails if it stops being true.
        //
        // Columns: Config only · Add missing · Unattended · Stay nearby · Everything
        let quiet = f(false, false, 0);
        let elevates = f(true, false, 0);
        let blocked = f(false, true, 0);
        let slow = f(false, false, 900);
        // Slow AND elevating: `slow` DOMINATES. The spec's rung 3 excludes "the slow
        // ones" full stop, so a package that is both lands on the last rung.
        let slow_and_elevates = f(true, false, 900);

        let cases: &[(&str, Action, bool, Facts, [bool; 5])] = &[
            // a config-atom: admitted from the very first rung, at every rung after
            (
                "config atom",
                Action::Install,
                true,
                quiet,
                [true, true, true, true, true],
            ),
            // a plain install: rung 1 onward. NOT rung 0 — a fresh install DOWNLOADS,
            // so it can be slow, elevate, or be blocked; "installs" and "fast" contradict
            // each other, which is exactly why rung 0 exists below it.
            (
                "install",
                Action::Install,
                false,
                quiet,
                [false, true, true, true, true],
            ),
            // an install of something known to elevate is STILL rung 1: the rung 2/3/4
            // distinctions govern UPGRADES. An install was explicitly asked for.
            (
                "install, elevates",
                Action::Install,
                false,
                elevates,
                [false, true, true, true, true],
            ),
            // a quiet upgrade: rung 2, "start it and walk away"
            (
                "upgrade, quiet",
                Action::Upgrade,
                false,
                quiet,
                [false, false, true, true, true],
            ),
            // an upgrade that will ask for your hand: rung 3
            (
                "upgrade, uac",
                Action::Upgrade,
                false,
                elevates,
                [false, false, false, true, true],
            ),
            // an upgrade the firewall blocks: rung 3 too — same promise ("you must stay"),
            // because a 403 needs a human to unblock a page
            (
                "upgrade, 403",
                Action::Upgrade,
                false,
                blocked,
                [false, false, false, true, true],
            ),
            // an upgrade that drags: rung 4 only
            (
                "upgrade, slow",
                Action::Upgrade,
                false,
                slow,
                [false, false, false, false, true],
            ),
            (
                "upgrade, slow+uac",
                Action::Upgrade,
                false,
                slow_and_elevates,
                [false, false, false, false, true],
            ),
            // UNINSTALL IS NOT ON THE LADDER. It honours the user's ✕ at every rung,
            // including the first — the ladder governs how far to GO, not whether to
            // honour a veto. Even a slow, elevating, blocked removal goes through.
            (
                "uninstall",
                Action::Uninstall,
                false,
                quiet,
                [true, true, true, true, true],
            ),
            (
                "uninstall, slow+uac+403",
                Action::Uninstall,
                false,
                f(true, true, 900),
                [true, true, true, true, true],
            ),
            // Downgrade is filtered out UPSTREAM (apply_diff matches only
            // Install|Uninstall|Upgrade), so this is unreachable — answered anyway so the
            // function is total, and pinned so "unreachable" never quietly becomes "true".
            (
                "downgrade",
                Action::Downgrade,
                false,
                quiet,
                [false, false, false, false, false],
            ),
        ];

        for (what, action, is_config, facts, expected) in cases {
            for (n, &want) in expected.iter().enumerate() {
                let rung = Rung::from_wire(n as u64);
                assert_eq!(
                    rung_allows(rung, *action, *is_config, facts),
                    want,
                    "{what} at rung {n} ({})",
                    rung.as_str()
                );
            }
            // And the shape itself: once admitted, never withdrawn.
            let mut seen_true = false;
            for &v in expected.iter() {
                if seen_true {
                    assert!(v, "{what}: a rung withdrew what a lower one admitted");
                }
                seen_true |= v;
            }
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
    fn a_rung_off_the_wire_is_clamped_not_trusted() {
        // The rung arrives as a number from a client. An out-of-range or absent one must
        // land on EVERYTHING, not on nothing: an older front, or a probe that omits the
        // field, must get today's behaviour. Doing silently LESS than the user asked is
        // the worse direction of error.
        assert_eq!(Rung::from_wire(0), Rung::ConfigOnly);
        assert_eq!(Rung::from_wire(4), Rung::Everything);
        assert_eq!(
            Rung::from_wire(5),
            Rung::Everything,
            "out of range → everything"
        );
        assert_eq!(Rung::from_wire(9999), Rung::Everything);
        assert_eq!(Rung::default(), Rung::Everything, "absent → everything");
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
        // — otherwise a Mac would inherit Windows' elevation and drop a rung for no
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
