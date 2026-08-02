//! behaviour.rs — what a package DOES, learned from what the fleet observed.
//!
//! Three facts per (package, route, os): does it demand elevation, does the firewall
//! block it, how long does it take at worst. They exist so an Apply can be calibrated
//! — "everything that will not interrupt me" is unanswerable without them.
//!
//! ⭐ ALL THREE ARE MONOTONE: `uac` and `forbidden` only ever go false→true, and
//! `slow_secs` only ever grows. That is not a simplification, it is the whole design —
//! but be precise about what it buys, because it is NOT automatic convergence. Writing
//! is a whole-file overwrite, not a merge of concurrent versions: if A reads, B reads, A
//! writes, then B writes, A's facts are gone from the file (and a synchronised share may
//! produce a conflict copy instead of either version).
//!
//! What monotonicity buys is that **no write can ever produce a WRONG value, and a lost
//! fact is recoverable by RE-OBSERVATION**: the next machine that OBSERVES that fact merges
//! it in, and `∨`/`max` put it back. Merging alone recovers nothing — after the overwrite no
//! machine still holds the lost value, so a machine that merely touches the file has nothing
//! to contribute. What makes that acceptable is that these facts are observed rather than
//! authored: the next Apply produces them again. The worst case is a delay, never a wrong
//! answer. A counter ("seen 7 times") would not have that safety — the same interleaving
//! loses an increment permanently and leaves a plausible-looking but simply false number,
//! which no later merge can detect or repair.
//!
//! The price is that nothing is ever forgotten: if the firewall opens, a `forbidden`
//! stays. The escape hatch is the CATALOGUE — punctual, versioned, and it leaves a trace
//! of the decision. (Emptying the shared file would be an invisible gesture nobody would
//! remember making.) It is BUILT and now CONSULTED: `bundles::resolve_facts` applies a
//! package's declared overrides over what was collected here, `RawPkg::overrides` is the one
//! lift from the YAML, and `ladder::resolve_plan_facts` runs the pair over every step at
//! startup — so an override written in catalog/ finally changes what a row says.
//!
//! ⚠️ And it is ACTED on now: `ladder::rung_allows` reads these facts in `visual_plan`'s
//! build loop, so a `uac`/`403`/`slow` flag decides whether a step is in a given rung's
//! Apply at all. Only the BATCH — a per-row button consults no rung.
//!
//! ⚠️ And it must stay on the reading side. What `resolve_facts` returns is a BELIEF, not
//! an observation, and it is the same `Facts` type `merge_into` accepts — so feeding one
//! back here would ratchet a declared `slow`'s sentinel into the share permanently, which
//! is precisely the WRONG value the paragraphs above promise no write can produce. Merge
//! only what a machine actually saw.
//!
//! ⭐ AND THERE IS NO SUBJECT HERE: no host, no user, no fine timestamp, no count. This
//! answers "how does this package behave", a question about software. The install journal
//! answers "who installed what, when" — and its SHARED copy is consent-gated for that
//! reason, while its local copy is written unconditionally (see `consent::append_history`).
//! The gate is about publishing a record of a person. This file is shared by default and
//! needs no gate, because there is no person in it to publish.
//!
//! Pure and total (parse/merge/serialise): no IO here, deliberately, so the properties
//! above can be tested directly. The IO shell is behaviour_io.rs, which reads and writes
//! the shared files and holds every disk decision.

use crate::platform::Os;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// There are NO `allow(dead_code)` left in this module. There were four when it was the only
// thing landed, one after the IO shell reached `parse_behaviour`/`parse_failed`/`to_yaml`/
// `merge_into`, and now none: the Apply path calls `key` and `merge_into` when a step
// records what it observed, which — with the flush going through the IO shell — reaches
// everything else transitively (`key`/`merge_into` reach `merge`; the parse/serialise trio
// is reached via `merge_and_write`). Nothing here is waiting for a consumer any more.
//
// And nowhere else is, either: the three that survived Part A — `read_all` in
// behaviour_io.rs, `resolve_facts` and `RawPkg::overrides` in bundles.rs, all on the READING
// side — left together when the ladder gave them their caller. `server::serve` reads every
// package's facts at startup and `ladder::resolve_plan_facts` lays the catalogue's
// declarations over them. The behaviour mechanism now carries no `allow(dead_code)` at all,
// across all three of its files.
//
// Measured by stripping one at a time under `-D warnings`, not guessed. Two things that
// measurement showed, worth recording because they are counter-intuitive:
//
// - Of the SEVEN `#[allow(dead_code)]` left in the tree, only TWO are load-bearing:
//   `Posture::parse` (which transitively keeps the enum's variants constructed) and
//   `StepOutcome::ok`. Stripping all seven yields exactly three errors, all from those two
//   roots. The other five — on `Posture` itself and on `Step`'s
//   `install`/`upgrade`/`downgrade`/`requires` — are REDUNDANT: every one of those items is
//   read somewhere. That predates this work and is left alone rather than swept in, verified
//   by stripping the same five against an earlier commit where clippy is equally silent.
//   (The ladder's three came out when the filter was wired into `visual_plan`; the count was
//   ten while they stood.)
// - An `allow` on the reading side did NOT go away when the Apply path started COLLECTING
//   facts. That wiring only writes observations, and feeding a resolved fact back into it is
//   the one thing this module forbids. It took a READER.

/// What was observed for ONE (route, os) of one package.
/// `Default` = nothing observed, which is distinct from "observed as false" only in
/// intent — the merge treats them identically, and that is deliberate: a quiet run is
/// not evidence that a package never elevates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// Demanded elevation (a foreign window appeared during the step, Windows only).
    #[serde(default, skip_serializing_if = "is_false")]
    pub uac: bool,
    /// The corporate firewall answered 403 on this package.
    /// Named `forbidden`, not `403`, because a Rust field cannot start with a digit —
    /// the YAML key is what the spec calls `403`, via the `rename` on this field.
    #[serde(default, rename = "403", skip_serializing_if = "is_false")]
    pub forbidden: bool,
    /// The WORST duration ever seen, in whole seconds. 0 = never measured.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slow_secs: u64,
}

// Skip-serializing keeps a behaviour file readable: only what is KNOWN appears, so a
// human opening it sees facts rather than a wall of `false`.
fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One package's facts, keyed by `route/os`. BTreeMap for deterministic output — the
/// file lives on a share and a stable byte order keeps its diffs and syncs quiet.
pub type Record = BTreeMap<String, Facts>;

/// The key under which a (route, os) pair's facts live. A behaviour belongs to the
/// COUPLE, not the package: "AWS CLI elevates" is false as stated — `Amazon.AWSCLI`
/// via winget on Windows elevates, `brew install awscli` does not.
///
/// ⚠️ These os strings are the FILE FORMAT, not Rust's: macOS is `darwin` here, whereas
/// `std::env::consts::OS` says `macos`. It comes from the spec's `brew/darwin`. Do not
/// "correct" it to match Rust — that would orphan the facts in every file already written.
pub fn key(route: &str, os: Os) -> String {
    let os = match os {
        Os::Windows => "windows",
        Os::Darwin => "darwin",
        Os::Linux => "linux",
    };
    let route = if route.is_empty() { "none" } else { route };
    format!("{route}/{os}")
}

/// Ratchet two observations together. Commutative and idempotent by construction
/// (`merge(a,b) == merge(b,a)` and `merge(a,a) == a`); both are tested.
///
/// Nothing outside this module calls it directly — every caller goes through `merge_into`,
/// which is why it needs no `allow(dead_code)` of its own.
pub fn merge(a: &Facts, b: &Facts) -> Facts {
    Facts {
        uac: a.uac || b.uac,
        forbidden: a.forbidden || b.forbidden,
        slow_secs: a.slow_secs.max(b.slow_secs),
    }
}

/// Merge a fresh observation into a whole record, under one key. (`at`, not `key`, so it
/// does not shadow the `key()` function.)
pub fn merge_into(rec: &mut Record, at: &str, obs: &Facts) {
    let entry = rec.entry(at.to_string()).or_default();
    *entry = merge(entry, obs);
}

/// Pure, defensive: anything not well-formed → an empty record. This file is on a
/// share and written by other machines, so malformed is a normal event.
///
/// ⚠️ THE COST: this is all-or-nothing, NOT per-entry. `serde_yaml` rejects the whole
/// document, so ONE malformed entry silently discards EVERY valid fact in the file, and
/// a corrupt file is indistinguishable from a fresh one — the caller sees an empty record
/// either way. Tolerable only because facts are re-observed rather than authored: the
/// next Apply puts them back. Do not read this as the line-by-line resilience of
/// `consent::parse_history`; a mapping has no lines to salvage.
///
/// ⚠️ AND UNKNOWN KEYS ARE ERASED, not preserved: a key this build does not know is
/// accepted here, dropped from `Facts`, and therefore absent from `to_yaml`. So the first
/// time an older build merges and writes back, a newer build's fourth fact is gone from
/// the shared file. Adding a fact is safe; removing or renaming one is not.
pub fn parse_behaviour(raw: &str) -> Record {
    serde_yaml::from_str::<Record>(raw).unwrap_or_default()
}

/// Did the document fail to parse AT ALL? The companion signal to `parse_behaviour`, which
/// is total by design and therefore cannot distinguish "nothing was there" from "everything
/// was thrown away". Same bytes, same parser; this keeps only whether it errored.
///
/// It lives HERE rather than in the IO shell because knowing what YAML accepts is this
/// module's business, and the shell must stay YAML-ignorant. That is not a purity
/// preference: the obvious shell-side heuristic — "bytes were non-empty but the record is
/// empty" — is WRONG for six shapes that parse cleanly and lose nothing. `{}`, `{ }`,
/// `--- {}`, `---`, a comment-only file, and `{}` followed by a comment (which is not
/// hypothetical: an otherwise-empty file is allowed to carry a date comment). Only a real
/// error answers true: a malformed entry, unbalanced brackets, or a `null` document.
pub fn parse_failed(raw: &str) -> bool {
    serde_yaml::from_str::<Record>(raw).is_err()
}

/// Serialise for writing. An empty record is `"{}\n"`, so the empty string means only one
/// thing: serialisation failed. It is not reachable for `BTreeMap<String, Facts>` (no
/// non-string keys, no failing Serialize impl) — it exists so this stays total, and so the
/// IO shell has an unambiguous "do not write" signal if the type ever grows a fallible field.
pub fn to_yaml(rec: &Record) -> String {
    serde_yaml::to_string(rec).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(uac: bool, blocked: bool, secs: u64) -> Facts {
        Facts {
            uac,
            forbidden: blocked,
            slow_secs: secs,
        }
    }

    #[test]
    fn merge_ratchets_each_fact_upward() {
        // uac and forbidden are ORs, slow_secs is a MAX. Nothing ever goes back down:
        // that is what lets two machines write without coordinating.
        let a = facts(true, false, 100);
        let b = facts(false, true, 250);
        let m = merge(&a, &b);
        assert!(m.uac, "true ∨ false = true");
        assert!(m.forbidden, "false ∨ true = true");
        assert_eq!(m.slow_secs, 250, "the WORST duration survives");
    }

    #[test]
    fn merge_is_order_independent() {
        // THE property the design rests on: it is what makes a lost fact merely DELAYED
        // rather than wrong, since whoever merges next reaches the same answer regardless
        // of who wrote first. Proved, not sampled — the booleans are only 4 states each,
        // so every pair is cheap to enumerate.
        let durations = [0u64, 30, 900];
        for &ua in &[false, true] {
            for &fa in &[false, true] {
                for &ub in &[false, true] {
                    for &fb in &[false, true] {
                        for &da in &durations {
                            for &db in &durations {
                                let a = facts(ua, fa, da);
                                let b = facts(ub, fb, db);
                                assert_eq!(
                                    merge(&a, &b),
                                    merge(&b, &a),
                                    "merge is not commutative for {a:?} / {b:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn merge_is_idempotent() {
        // Claimed in merge's doc, so it is pinned here. It matters in practice: the same
        // observation can be merged twice (a retried write, a file synced back to us) and
        // must not drift.
        for &uac in &[false, true] {
            for &blocked in &[false, true] {
                for &secs in &[0u64, 242] {
                    let a = facts(uac, blocked, secs);
                    assert_eq!(merge(&a, &a), a, "merging {a:?} with itself changed it");
                }
            }
        }
    }

    #[test]
    fn merge_with_nothing_observed_changes_nothing() {
        let a = facts(true, true, 500);
        assert_eq!(merge(&a, &Facts::default()), a);
        assert_eq!(merge(&Facts::default(), &a), a);
    }

    #[test]
    fn merge_never_unsets_a_fact() {
        // An observation saying "I saw no elevation this time" must NOT clear a
        // previous `uac: true`. Rigid on purpose: the escape hatch is the catalogue
        // override, not a later quieter run.
        let known = facts(true, true, 400);
        let quiet_run = facts(false, false, 10);
        let m = merge(&known, &quiet_run);
        assert!(m.uac && m.forbidden);
        assert_eq!(m.slow_secs, 400);
    }

    #[test]
    fn merge_into_ratchets_one_key_and_leaves_the_others_alone() {
        // The record-level entry point: it must create a key that was absent, ratchet a
        // key that was present, and never touch a sibling route.
        let mut rec = Record::new();
        rec.insert("winget/windows".to_string(), facts(true, false, 400));
        rec.insert("brew/darwin".to_string(), facts(false, false, 38));

        merge_into(&mut rec, "winget/windows", &facts(false, true, 10));
        merge_into(&mut rec, "cargo/linux", &facts(false, false, 7));

        let w = rec["winget/windows"];
        assert!(w.uac, "a quiet observation did not clear the known uac");
        assert!(w.forbidden, "the new 403 was recorded");
        assert_eq!(w.slow_secs, 400, "the faster run did not lower the worst");
        assert_eq!(
            rec["brew/darwin"],
            facts(false, false, 38),
            "sibling untouched"
        );
        assert_eq!(
            rec["cargo/linux"],
            facts(false, false, 7),
            "absent key created"
        );
    }

    #[test]
    fn parse_reads_a_route_os_keyed_file() {
        let raw = "\
winget/windows:
  uac: true
  slow_secs: 242
brew/darwin:
  slow_secs: 38
";
        let rec = parse_behaviour(raw);
        let w = rec.get("winget/windows").expect("winget/windows present");
        assert!(w.uac);
        assert_eq!(w.slow_secs, 242);
        assert!(!w.forbidden, "absent means false, not unknown");
        let b = rec.get("brew/darwin").expect("brew/darwin present");
        assert_eq!(b.slow_secs, 38);
        assert!(!b.uac);
    }

    #[test]
    fn parse_is_defensive_and_never_panics() {
        // The shared discipline with parse_selection / parse_history is only "never panic,
        // never sink the session" — NOT their per-line salvage. parse_history skips the one
        // bad line and keeps the rest; a YAML mapping has no lines to skip, so this parser
        // returns nothing at all. That difference is asserted just below.
        assert!(parse_behaviour("not yaml at all: [").is_empty());
        assert!(parse_behaviour("").is_empty());
        assert!(parse_behaviour("winget/windows: 42").is_empty());
    }

    #[test]
    fn one_bad_entry_discards_the_whole_file() {
        // Pins the all-or-nothing cost documented on parse_behaviour, which the
        // wholly-malformed inputs above cannot detect. If a serde_yaml upgrade (or a switch
        // to a lenient per-entry parse) ever changed this, the doc comment would silently
        // become a lie — and callers would start seeing partial records where they were
        // promised none.
        let mixed = "good/darwin:\n  uac: true\nbad/darwin: 42\n";
        assert!(
            parse_behaviour(mixed).is_empty(),
            "one malformed entry must discard the file entirely, valid siblings included \
             — a partial record here would mean the documented cost no longer holds"
        );
    }

    #[test]
    fn parse_failed_separates_a_real_error_from_a_legitimately_empty_document() {
        // Exists so the IO shell can log a whole-file loss WITHOUT knowing any YAML. Every
        // input below was measured against serde_yaml 0.9, not assumed — the five "empty but
        // valid" shapes are exactly the false alarms that sank the shell-side heuristic
        // ("non-empty bytes, empty record") this replaced.
        for empty_but_valid in [
            "",             // absent/truncated
            "   \n",        // whitespace
            "{}",           // what to_yaml writes for an empty record
            "{}\n",         // ditto, as written to disk
            "{ }",          // a human's spacing
            "--- {}",       // an explicit document marker
            "---\n",        // a document with no content
            "# comment\n",  // comment-only
            "{}\n# note\n", // empty PLUS a date comment, which the format allows
        ] {
            assert!(
                !parse_failed(empty_but_valid),
                "{empty_but_valid:?} parses fine and loses nothing — reporting it would cry wolf"
            );
            assert!(parse_behaviour(empty_but_valid).is_empty());
        }
        for really_broken in [
            "null",                                      // a document that is not a map
            "this: [is: not",                            // unbalanced
            "winget/windows: 42",                        // a scalar where Facts belongs
            "good/darwin:\n  uac: true\nbad/darwin: 42", // ONE bad entry discards the rest
        ] {
            assert!(
                parse_failed(really_broken),
                "{really_broken:?} is a real failure and must be reportable"
            );
            assert!(parse_behaviour(really_broken).is_empty());
        }
        // And a document that yields facts never counts as a failure.
        assert!(!parse_failed("winget/windows:\n  uac: true\n"));
    }

    #[test]
    fn serialise_then_parse_roundtrips() {
        let mut rec = Record::new();
        rec.insert("winget/windows".to_string(), facts(true, false, 242));
        let raw = to_yaml(&rec);
        assert_eq!(parse_behaviour(&raw), rec);
    }

    #[test]
    fn the_yaml_key_for_forbidden_is_403() {
        // The field is `forbidden` only because Rust forbids an identifier starting with
        // a digit; the file the fleet shares says `403`. Asserted on the bytes, because a
        // roundtrip alone would still pass if the rename were dropped — and then a file
        // written by an older build would silently read back as "not blocked".
        let mut rec = Record::new();
        rec.insert("winget/windows".to_string(), facts(false, true, 0));
        let raw = to_yaml(&rec);
        assert!(raw.contains("403"), "the YAML key is 403, got: {raw}");
        assert!(
            !raw.contains("forbidden"),
            "the Rust name must not leak: {raw}"
        );
        assert_eq!(parse_behaviour(&raw), rec);

        // And a hand-written `403:` is read back — this is the shape the spec documents.
        let hand = parse_behaviour("winget/windows:\n  \"403\": true\n");
        assert!(hand["winget/windows"].forbidden);
    }

    #[test]
    fn the_key_is_route_and_os() {
        assert_eq!(
            key("winget", crate::platform::Os::Windows),
            "winget/windows"
        );
        assert_eq!(key("brew", crate::platform::Os::Darwin), "brew/darwin");
        // A package with no route on this platform still needs a stable key rather
        // than a panic — "none" is honest and sorts harmlessly.
        assert_eq!(key("", crate::platform::Os::Linux), "none/linux");
    }
}
