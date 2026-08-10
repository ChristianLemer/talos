//! ladder.rs — how FAR an Apply goes, and the facts that calibrate it.
//!
//! Apply used to be all-or-nothing. The ladder is one control with six CUMULATIVE
//! rungs, so a user can ask for "the fast harmless part now" or "everything that will
//! not interrupt me" — C's framing, and the reason the behaviour telemetry exists at all.
//!
//! This module is PURE: the rungs, the filter, and the resolution of (collected ×
//! declared) onto a plan. No IO, no state, no socket. The wiring is server.rs's:
//! `resolve_plan_facts` is called there at startup, `rung_from_wire` parses the rung off the
//! `apply` message, and `rung_allows` governs `apply_diff`'s `visual_plan` loop. Every item
//! here has a real caller now — the module carries no `allow(dead_code)`.
//!
//! ⚠️ The filter governs the BATCH only. `row_action` deliberately does not consult a rung:
//! clicking install on one row is an explicit gesture about one thing.
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
/// Two real callers, either of which would keep it and `SLOW_SECS` alive on its own:
/// `server::step_json` puts `slow` on the wire per row, and `rung_allows` classifies with it.
/// It briefly depended on `rung_allows`'s `#[allow(dead_code)]` to stay borrowed at all;
/// nothing in this module carries one any more.
pub fn is_slow(f: &Facts) -> bool {
    f.slow_secs >= SLOW_SECS
}

/// How far an Apply goes. SIX CUMULATIVE rungs — each contains the previous, which is
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
    /// 🧩 extensions: plugins and skills — things that install INTO a host, not onto the
    /// machine.
    ///
    /// Its promise is its own, which is what earns it a rung rather than a place in one of its
    /// neighbours: it TOUCHES THE NETWORK (a clone, a download) and it NEVER TOUCHES THE
    /// MACHINE — nothing enters Program Files or /Applications, nothing elevates, and undoing
    /// it means deleting a folder inside your own profile.
    ///
    /// So it cannot share rung 0 with a config-atom, whose promise is strictly stronger (no
    /// network at all), and it should not queue behind a binary that may want your hand for
    /// nine minutes. That was C's observation: adding a two-second skill is boring when it
    /// rides with Git.
    ///
    /// ✅ The "never elevates" half is a MEASURED property of the corpus, not an assumption:
    /// across the 34 shipped packages no `claude-plugin` and no `skill` declares `uac` or
    /// `403`, while nine binary packages declare `uac: true`.
    Extensions,
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
    ///
    /// Called by `server::rung_from_wire`, which is what finally made this a real root —
    /// and with it the four variants it constructs. It carried an `#[allow(dead_code)]`
    /// until then, because a `#[cfg(test)]` caller does not keep a non-test build's item
    /// alive.
    pub fn from_wire(n: u64) -> Rung {
        match n {
            0 => Rung::ConfigOnly,
            1 => Rung::Extensions,
            2 => Rung::AddMissing,
            3 => Rung::Unattended,
            4 => Rung::StayNearby,
            // ⚠️ THE CEILING MOVED from 4 to 5 when Extensions was inserted. The catch-all
            // must stay a catch-all: an off-by-one here would make the LAST detent stop
            // working, silently doing less than the user asked for.
            _ => Rung::Everything,
        }
    }
    /// For the log line, so an Apply says how far it was told to go — `apply_diff` prints
    /// it beside `kept of candidates`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Rung::ConfigOnly => "config-only",
            Rung::Extensions => "extensions",
            Rung::AddMissing => "add-missing",
            Rung::Unattended => "unattended",
            Rung::StayNearby => "stay-nearby",
            Rung::Everything => "everything",
        }
    }
    fn level(&self) -> u8 {
        match self {
            Rung::ConfigOnly => 0,
            Rung::Extensions => 1,
            // ⚠️ EVERY LEVEL BELOW SHIFTED UP BY ONE when Extensions was inserted. Safe here
            // because the rung is NOT PERSISTED (app.js says so in as many words), so no
            // stored preference can be re-read under the new numbering — the only exposure is
            // a stale front against a new server, within one launch, and `Default` already
            // errs toward Everything for that case.
            Rung::AddMissing => 2,
            Rung::Unattended => 3,
            Rung::StayNearby => 4,
            Rung::Everything => 5,
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
///
/// ⚠️ AND `row_action` MUST NOT CALL THIS. A per-row button is an explicit gesture about one
/// package — the ladder calibrates the BATCH, and nothing else. The whole module is now
/// `allow`-free: `apply_diff`'s `visual_plan` loop calls this for real, which is what
/// keeps `Rung::level` alive with it.
pub fn rung_allows(
    rung: Rung,
    action: crate::decision::Action,
    is_config: bool,
    is_extension: bool,
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
        // ⭐ An extension: rung 1, above config-atoms and below binaries. Its own promise —
        // touches the network, never touches the machine.
        //
        // ⚠️ The guard order is load-bearing even though the two classes are disjoint by
        // construction (a config-atom is recognised by having a `check:`, which no extension
        // has). Written config-first anyway, and pinned by the table: if the classes ever
        // overlapped, a config-atom must keep its rung-0 admission rather than be demoted.
        //
        // ⚠️ It stays rung 1 EVEN IF a fact says it elevates, unlike the Upgrade arm below.
        // The ROUTE is what earns the rung, not the observation — and today no content route
        // declares `uac` or `403` in the whole catalogue. If one ever does, the promise is
        // broken and THIS is the line to revisit, deliberately.
        Action::Install if is_extension => rung.level() >= 1,
        Action::Install => rung.level() >= 2,
        // ⚠️ EVERY THRESHOLD HERE MOVED UP BY ONE when Extensions was inserted at level 1.
        // The rungs are named by promise, not by number, so what must be preserved is the
        // MEANING: unattended stays "start it and walk away", stay-nearby stays "it will want
        // your hand", everything stays last. The table test asserts the whole grid, which is
        // what catches a threshold left behind.
        //
        // ⬜ OPEN — an extension's UPGRADE takes this path, not the rung-1 arm above, so it
        // currently needs rung 3+. Unreachable today (`scan_outdated` asks only winget/brew,
        // so no plugin can ever be marked outdated — the plugin-drift plan fixes that), and
        // arguably wrong once it IS reachable: an extension upgrade engages exactly what its
        // install engages, so rung 1 is the consistent answer. Left as-is deliberately rather
        // than guessed, because the day it becomes reachable is the day it can be measured.
        Action::Upgrade => {
            let needed = if is_slow(f) {
                5 // slow DOMINATES: the spec's last rung excludes "the slow ones", full stop
            } else if f.uac || f.forbidden {
                4 // it will want your hand, or a page unblocked
            } else {
                3 // start it and walk away
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
            is_extension: false,
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
        // Columns: Config only · Extensions · Add missing · Unattended · Stay nearby · Everything
        let quiet = f(false, false, 0);
        let elevates = f(true, false, 0);
        let blocked = f(false, true, 0);
        let slow = f(false, false, 900);
        // Slow AND elevating: `slow` DOMINATES. The spec's rung 3 excludes "the slow
        // ones" full stop, so a package that is both lands on the last rung.
        let slow_and_elevates = f(true, false, 900);

        // One row of the grid: what it is, the action, its two CLASS flags (config-atom /
        // extension — derived from the route, never declared), the facts, and whether each of
        // the six rungs admits it. Named because clippy is right that the bare tuple had
        // grown unreadable — and a named row is what lets the table be read as a table.
        type Case = (&'static str, Action, bool, bool, Facts, [bool; 6]);
        let cases: &[Case] = &[
            // a config-atom: admitted from the very first rung, at every rung after
            (
                "config atom",
                Action::Install,
                true,
                false,
                quiet,
                [true, true, true, true, true, true],
            ),
            // ⭐ AN EXTENSION: rung 1 onward — above config-atoms, below binaries. Its promise
            // is its own: it TOUCHES THE NETWORK (a clone) and NEVER TOUCHES THE MACHINE
            // (nothing in Program Files, nothing elevated). That is why it cannot share rung 0
            // with a config-atom, whose promise is stronger still (no network at all), and why
            // it should not wait behind a Git that may want your hand for nine minutes.
            (
                "extension install",
                Action::Install,
                false,
                true,
                quiet,
                [false, true, true, true, true, true],
            ),
            // ⚠️ And it stays rung 1 even if some fact says it elevates. Measured across the
            // 34 shipped packages: no plugin and no skill declares `uac` or `403`, so this row
            // is about a fact that does not exist today — answered deliberately, because the
            // ROUTE is what earns the rung. If a content route ever did elevate, the promise
            // would be broken and THIS LINE is where that decision must be revisited rather
            // than discovered in the field.
            (
                "extension install, elevates",
                Action::Install,
                false,
                true,
                elevates,
                [false, true, true, true, true, true],
            ),
            // a plain install: rung 1 onward. NOT rung 0 — a fresh install DOWNLOADS,
            // so it can be slow, elevate, or be blocked; "installs" and "fast" contradict
            // each other, which is exactly why rung 0 exists below it.
            (
                "install",
                Action::Install,
                false,
                false,
                quiet,
                [false, false, true, true, true, true],
            ),
            // an install of something known to elevate is STILL rung 1: the rung 2/3/4
            // distinctions govern UPGRADES. An install was explicitly asked for.
            (
                "install, elevates",
                Action::Install,
                false,
                false,
                elevates,
                [false, false, true, true, true, true],
            ),
            // a quiet upgrade: rung 2, "start it and walk away"
            (
                "upgrade, quiet",
                Action::Upgrade,
                false,
                false,
                quiet,
                [false, false, false, true, true, true],
            ),
            // an upgrade that will ask for your hand: rung 3
            (
                "upgrade, uac",
                Action::Upgrade,
                false,
                false,
                elevates,
                [false, false, false, false, true, true],
            ),
            // an upgrade the firewall blocks: rung 3 too — same promise ("you must stay"),
            // because a 403 needs a human to unblock a page
            (
                "upgrade, 403",
                Action::Upgrade,
                false,
                false,
                blocked,
                [false, false, false, false, true, true],
            ),
            // an upgrade that drags: rung 4 only
            (
                "upgrade, slow",
                Action::Upgrade,
                false,
                false,
                slow,
                [false, false, false, false, false, true],
            ),
            (
                "upgrade, slow+uac",
                Action::Upgrade,
                false,
                false,
                slow_and_elevates,
                [false, false, false, false, false, true],
            ),
            // UNINSTALL IS NOT ON THE LADDER. It honours the user's ✕ at every rung,
            // including the first — the ladder governs how far to GO, not whether to
            // honour a veto. Even a slow, elevating, blocked removal goes through.
            (
                "uninstall",
                Action::Uninstall,
                false,
                false,
                quiet,
                [true, true, true, true, true, true],
            ),
            (
                "uninstall, slow+uac+403",
                Action::Uninstall,
                false,
                false,
                f(true, true, 900),
                [true, true, true, true, true, true],
            ),
            // Downgrade is filtered out UPSTREAM (apply_diff matches only
            // Install|Uninstall|Upgrade), so this is unreachable — answered anyway so the
            // function is total, and pinned so "unreachable" never quietly becomes "true".
            (
                "downgrade",
                Action::Downgrade,
                false,
                false,
                quiet,
                [false, false, false, false, false, false],
            ),
        ];

        for (what, action, is_config, is_extension, facts, expected) in cases {
            for (n, &want) in expected.iter().enumerate() {
                let rung = Rung::from_wire(n as u64);
                assert_eq!(
                    rung_allows(rung, *action, *is_config, *is_extension, facts),
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
        // ⚠️ THE CEILING MOVED, 4 → 5, when Extensions was inserted at 1. This assertion is
        // the one that catches an off-by-one in the clamp — which would make the LAST detent
        // stop working while everything else looked fine.
        assert_eq!(Rung::from_wire(5), Rung::Everything, "the last real rung");
        assert_eq!(
            Rung::from_wire(4),
            Rung::StayNearby,
            "and 4 is no longer the top — it is stay-nearby now"
        );
        assert_eq!(
            Rung::from_wire(6),
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
