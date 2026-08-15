//! ladder.rs — how FAR an Apply goes, and the facts that calibrate it.
//!
//! Apply used to be all-or-nothing. The ladder is one control with three CUMULATIVE rungs
//! answering ONE question — where does this land: your files, your profile, or the machine?
//!
//! ⚠️ It had six. The three that sorted upgrades by `uac`/`403`/`slow` were cut: those are
//! per-package FACTS, shown on the row, and a rung that filtered them made the user drag a
//! slider to discover why a line was missing. See `Rung` for what that trade gives up.
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

/// How far an Apply goes. THREE CUMULATIVE rungs — each contains the previous.
///
/// ⭐ There were SIX. C cut it to three, and the reasoning reverses an earlier decision worth
/// recording: the three that went (☕ unattended, 👀 stay nearby, 🏗️ everything) sorted
/// UPGRADES by `uac` / `403` / `slow`. But those are per-package facts, and the front already
/// has words for each of them. Written on the row and always visible, the user reads them
/// BEFORE clicking. Kept as rungs, they filtered on the user's behalf — and the only way to
/// learn WHY a row was missing was to drag the slider until it came back.
///
/// So the axis is now one question, and it is the one people actually ask: **where does this
/// land?** Your files, your profile, or the machine.
///
/// ⚠️ WHAT IS GIVEN UP: nothing in the ladder separates "I can leave the room" from "it will
/// ask for my password". An Apply at 📦 can stop on a prompt long after you walked away. That
/// is the trade — three rungs a user understands with no explanation, against a filter that
/// did the deciding. The row labels carry it now, so they must be visible and true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// ⚡ config-atoms only. Purely local: both shipped atoms run `nu "{dir}/….nu" apply`,
    /// no network at all. A STRONGER promise than "fast" — and what earns this rung its place
    /// at the bottom, since a fresh install DOWNLOADS.
    ConfigOnly,
    /// 🧩 extensions: plugins and skills — things that install INTO a host, not onto the
    /// machine. They touch the network (a clone) and never touch the machine: nothing enters
    /// Program Files or /Applications, nothing elevates, undoing one deletes a folder in your
    /// own profile.
    ///
    /// ✅ The "never elevates" half is MEASURED, not assumed: across the 34 shipped packages
    /// no `claude-plugin` and no `skill` declares `uac` or `403`, while nine binary packages
    /// declare `uac: true`.
    Extensions,
    /// 📦 apps: everything else — the machine itself. Nothing is excluded here, including the
    /// slow ones and the ones that will ask for your hand; the ROW says which is which.
    Apps,
}

impl Default for Rung {
    /// ABSENT MEANS EVERYTHING. An older front, or any client that omits the field, must
    /// get today's behaviour — doing silently LESS than the user asked for is the worse
    /// direction of error, and a rung is a preference rather than a safety property.
    ///
    /// Deliberately the OPPOSITE stance from `unmanaged`, whose omission the server
    /// refuses to trust: scope is a safety property, a rung is a preference.
    fn default() -> Self {
        Rung::Apps
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
            // ⚠️ The catch-all must STAY a catch-all: an off-by-one here would make the last
            // detent stop working, silently doing less than the user asked for.
            _ => Rung::Apps,
        }
    }
    /// For the log line, so an Apply says how far it was told to go — `apply_diff` prints
    /// it beside `kept of candidates`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Rung::ConfigOnly => "config-only",
            Rung::Extensions => "extensions",
            Rung::Apps => "apps",
        }
    }
    fn level(&self) -> u8 {
        match self {
            Rung::ConfigOnly => 0,
            Rung::Extensions => 1,
            Rung::Apps => 2,
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
/// ⭐ It takes NO `Facts`, and that absence is the reshape's signature. It used to, because
/// three rungs sorted upgrades by `uac` / `403` / `slow`. Those rungs are gone and the facts
/// are shown on the ROW, so the rule now depends only on WHAT KIND of thing this is. Keeping a
/// `&Facts` parameter that nothing reads would state a dependency that no longer exists — and
/// the next reader would look for where the facts are weighed.
///
/// `Facts` is still very much alive next door: `is_slow` classifies for the row labels and for
/// the estimate. It simply has no say in whether a rung admits an action.
///
/// ⭐ TWO DECISIONS WERE REVERSED to get here, and both are recorded because each was RIGHT
/// about the axis it was written for — a reader finding only the current form would re-derive
/// them:
///
/// 1. **The facts left.** Three more rungs (☕ unattended, 👀 stay nearby, 🏗️ everything) sorted
///    upgrades by `uac` / `403` / `slow`. A rung SORTS; a label INFORMS. Those are per-package
///    truths, and the front already had words for them — shown ONLY when the slider hid the row.
///    So the app knew something about a package and made the user drag a slider until the row
///    came back to find out what. On the row, always visible, it is read before clicking.
///
/// 2. **Uninstall joined.** It used to bypass every rung: *"the ladder governs how far to GO,
///    never whether to honour a veto"*. True of an axis made of TIME — delaying a removal for
///    being slow is nonsense. The axis is now WHERE IT LANDS, and removing an app touches the
///    machine, so ⚡ Config only would have quietly uninstalled software while promising to touch
///    nothing but your own files.
pub fn rung_allows(
    rung: Rung,
    action: crate::decision::Action,
    is_config: bool,
    is_extension: bool,
) -> bool {
    use crate::decision::Action;
    match action {
        // ⭐ UNINSTALL RIDES THE RUNG OF ITS KIND — the same rule as every other action, which
        // is why there is no arm of its own for it any more (see the shared arm below).
        //
        // ⚠️ THIS REVERSES A DOCUMENTED DECISION, and the reasoning is worth keeping because
        // the old one was not wrong — it was right about a DIFFERENT axis. It read: "removing a
        // package is driven by the user's ✕, not by how much time they have; the ladder governs
        // how far to GO, never whether to honour a veto." True while the axis was TIME. The axis
        // is now WHERE IT LANDS, and removing an app touches the machine — so ⚡ Config only
        // would have quietly uninstalled software while promising to touch nothing but your own
        // files. C's call, and the visual reshape makes it unavoidable: the smallest frame on
        // screen cannot be the one that removes the most.
        // Never batched by Apply at all (apply_diff matches Install|Uninstall|Upgrade), so
        // this arm is unreachable today. Answered so the function is total, and pinned by a
        // test so "unreachable" cannot quietly become "allowed".
        Action::Downgrade => false,
        // ⭐ ONE RULE FOR EVERY ACTION: it rides the rung of the KIND it acts on. Install,
        // upgrade and uninstall are now indistinguishable here, which is why they share an arm
        // — three arms saying the same thing would invite one of them to drift.
        //
        // ⚠️ The guard order is load-bearing even though the classes are disjoint by
        // construction (a config-atom is recognised by having a `check:`, which no extension
        // has). Written config-first anyway, and pinned by the table: if the classes ever
        // overlapped, a config-atom must keep its rung-0 admission rather than be demoted.
        _ if is_config => true,
        _ if is_extension => rung.level() >= 1,
        _ => rung.level() >= 2,
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
    fn the_three_rungs_are_cumulative_and_the_table_shows_it() {
        // ONE row per candidate, ONE column per rung. The cumulative property is the
        // SHAPE of this fixture — every row is a prefix of `false`s followed by `true`s —
        // and reading it is the proof. An assertion in prose ("rung 2 contains rung 1")
        // would be a claim; this is a fixture that fails if it stops being true.
        //
        // Columns: ⚡ Config only · 🧩 Extensions · 📦 Apps
        //
        // ⚠️ NO FACTS FIXTURES HERE ANY MORE — there were five (`quiet`, `elevates`, `blocked`,
        // `slow`, `slow_and_elevates`), and their disappearance is the reshape made visible:
        // `rung_allows` no longer takes `Facts` at all, so a fact cannot change a verdict and a
        // fixture carrying one would suggest otherwise. They still matter next door, where the
        // facts are still weighed: the row labels, and `is_slow` for the estimate.

        // One row of the grid: what it is, the action, its two CLASS flags (config-atom /
        // extension — derived from the route, never declared), and whether each of the three
        // rungs admits it. Named because clippy is right that the bare tuple had grown
        // unreadable — and a named row is what lets the table be read as a table.
        type Case = (&'static str, Action, bool, bool, [bool; 3]);
        let cases: &[Case] = &[
            // ⚡ rung 0 — your own files.
            (
                "config atom install",
                Action::Install,
                true,
                false,
                [true, true, true],
            ),
            // ⭐ A config-atom's UPGRADE is admitted at 0 too, which the six-rung version got
            // wrong: it sent every upgrade to the facts ladder regardless of kind.
            (
                "config atom upgrade",
                Action::Upgrade,
                true,
                false,
                [true, true, true],
            ),
            // 🧩 rung 1 — your profile. An extension, install or upgrade, and the facts do
            // NOT move it: the ROUTE earns the rung. That also settles the incoherence the
            // previous version left open, where a `version:` pin on a plugin dropped a
            // two-second install onto 👀 "it will want your hand".
            (
                "extension install",
                Action::Install,
                false,
                true,
                [false, true, true],
            ),
            (
                "extension upgrade",
                Action::Upgrade,
                false,
                true,
                [false, true, true],
            ),
            (
                "extension, elevates",
                Action::Install,
                false,
                true,
                [false, true, true],
            ),
            // A VS Code extension: same class, same rung, and the table is where that is
            // PROVEN rather than asserted in prose.
            (
                "vscode extension install",
                Action::Install,
                false,
                true,
                [false, true, true],
            ),
            (
                "vscode extension removal",
                Action::Uninstall,
                false,
                true,
                [false, true, true],
            ),
            // 📦 rung 2 — the machine. NOTHING is excluded here, and that is the whole point of
            // the reshape: `uac`, `403` and `slow` are written on the ROW instead of filtering
            // on the user's behalf.
            //
            // ⚠️ There used to be five rows here — quiet / uac / 403 / slow / slow+uac — one
            // per fact, because each landed on a different rung. They collapsed into two, and
            // the collapse IS the change: with the facts out of the rule, a fact-carrying
            // upgrade is indistinguishable from a quiet one. Keeping five identical rows would
            // dress up as coverage what is now a single behaviour. The facts' own coverage
            // moved to where they are still weighed — the row labels (`rungReason`) and
            // `is_slow` for the estimate.
            (
                "install",
                Action::Install,
                false,
                false,
                [false, false, true],
            ),
            (
                "upgrade",
                Action::Upgrade,
                false,
                false,
                [false, false, true],
            ),
            // ⭐ UNINSTALL RIDES THE RUNG OF ITS KIND, like every other action. C's decision,
            // and it closes a hole the reshape opened rather than adding a rule.
            //
            // `Uninstall => true` was RIGHT while the axis was TIME: delaying a removal because
            // it is slow makes no sense, so "the ladder governs how far to GO, never whether to
            // honour a veto" followed. The axis is now WHERE IT LANDS — and removing an app
            // touches the machine. So ⚡ Config only would have quietly uninstalled software
            // while claiming to touch nothing but your own files, which is the one promise the
            // bottom rung makes.
            (
                "uninstall config atom",
                Action::Uninstall,
                true,
                false,
                [true, true, true],
            ),
            (
                "uninstall extension",
                Action::Uninstall,
                false,
                true,
                [false, true, true],
            ),
            (
                "uninstall app",
                Action::Uninstall,
                false,
                false,
                [false, false, true],
            ),
            // Downgrade is filtered UPSTREAM (apply_diff matches Install|Uninstall|Upgrade),
            // so this is unreachable — answered anyway so the function is total, and pinned so
            // "unreachable" never quietly becomes "true".
            (
                "downgrade",
                Action::Downgrade,
                false,
                false,
                [false, false, false],
            ),
        ];

        for (what, action, is_config, is_extension, expected) in cases {
            for (n, &want) in expected.iter().enumerate() {
                let rung = Rung::from_wire(n as u64);
                assert_eq!(
                    rung_allows(rung, *action, *is_config, *is_extension),
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
        assert_eq!(Rung::from_wire(1), Rung::Extensions);
        // ⚠️ 2 is now the TOP, and everything above it clamps there. The ceiling has moved
        // twice (4 → 5 → 2), which is why it is asserted rather than reasoned about: an
        // off-by-one makes the last detent stop working and the failure is silent.
        assert_eq!(Rung::from_wire(2), Rung::Apps, "the last real rung");
        assert_eq!(Rung::from_wire(3), Rung::Apps, "out of range → the top");
        assert_eq!(Rung::from_wire(9999), Rung::Apps);
        assert_eq!(Rung::default(), Rung::Apps, "absent → the top");
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
