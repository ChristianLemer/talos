//! timings.rs — how long each package took ON THIS MACHINE, last time.
//!
//! ⭐ THIS IS THE ESTIMATION. Its twin, the shared `behaviour/<id>.yaml`, holds the
//! CLASSIFICATION. They answer different questions and must not be conflated:
//!
//! | | shared `slow_secs` | local `timings.yaml` |
//! |---|---|---|
//! | holds | the WORST any machine ever saw | this machine's LAST-SEEN duration |
//! | means | "this package is slow" | "expect about this long, here" |
//! | must be | IDENTICAL on every machine, so a rung means the same thing to everyone | personal, free to differ |
//! | merge | monotone `max`, many writers | LAST-SEEN WINS, one writer |
//!
//! ⚠️ THE CONSEQUENCE SOMEBODY WILL FILE AS A BUG: a package can be CLASSIFIED slow — last
//! rung, for everyone — while announcing "~20 s" here, because this machine's cache is
//! warm. Measured: 7-Zip took 1 second on the dev Mac because its brew bottle was already
//! cached; a fresh corporate machine behind the firewall would take far longer, and that
//! far-longer number is exactly what belongs in the shared classification. Two questions,
//! two answers, both right.
//!
//! ⚠️ AND THE MONOTONE DISCIPLINE DOES NOT APPLY HERE. `∨`/`max`/no-counters exist on the
//! shared file ONLY because many machines write it without coordinating. This file has
//! exactly ONE writer, so read-modify-write is fine and ASSIGNMENT is correct. Applying the
//! ratchet by reflex would freeze one unlucky slow run as this machine's estimate forever.
//!
//! ⚠️ AND IT IS A DERIVED CACHE, NOT A RECORD. `consent::HistEntry` already carries `secs`
//! per (package, action), so this is a SECOND source for a datum already written — two
//! truths to keep in agreement, accepted knowingly. What it buys is a bounded read of one
//! small keyed file at startup, instead of reparsing an append-only journal that grows
//! without limit and would have to be walked backwards to find "the last one per package".
//! THE JOURNAL IS THE TRACE; this is the cache. If they ever disagree, the journal is
//! right. Deleting this file is harmless — it refills on the next Apply.
//!
//! ⚠️ AND THE TWO ARE NOT EVEN KEYED THE SAME WAY, so they cannot be reconciled item by
//! item: the journal keys by (package NAME, action) and this file by (catalogue ID,
//! `route/os`). An install and an upgrade of the same package are two journal entries and
//! ONE entry here — the later one wins, deliberately, because the question is "how long
//! will this row take me", not "how long does each verb take".
//!
//! ⚠️ AND `0` IS A SENTINEL, NOT A DURATION. On the wire, `secs: 0` means "this machine has
//! never measured this row", which the widget counts as unknown. So a real sub-second step
//! must not write `0` — `note_duration` floors at one second, for a reason found at a real
//! click rather than reasoned about; see its own note.
//!
//! Same shape as selection.rs: pure parse, thin IO shell, never panics, LOCAL data-dir
//! only (never the shared exe folder).

// There are NO `allow(dead_code)` in this module, and none were added anywhere for it.
// Measured, not assumed: everything landed WITH its consumer in the same task, unlike
// behaviour.rs which spent Part A waiting for a reader. `server::serve` calls `read_timings`,
// `step_json` reads the snapshot, `remember_duration` calls `note_duration`, and
// `flush_behaviour` calls `merge_and_write_timings` — which reaches `read_timings`,
// `note_duration`, `write_timings` and `timings_path` transitively. `parse_timings` comes in
// through `read_timings`. Verified by `cargo clippy --all-targets -- -D warnings` reporting
// zero, with no attribute anywhere to hide behind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// package id → `route/os` → seconds. The inner key is `behaviour::key`'s, deliberately,
/// so the two files agree on what a (route, os) is called and a reader can line them up.
/// BTreeMap for deterministic writes and quiet diffs.
pub type Timings = BTreeMap<String, BTreeMap<String, u64>>;

/// Pure, defensive: anything not well-formed → empty. Never an error — the estimate is a
/// convenience, and a hand-edited or half-synced file must not sink a session.
pub fn parse_timings(raw: &str) -> Timings {
    serde_yaml::from_str::<Timings>(raw).unwrap_or_default()
}

/// Record what a step just took. ASSIGNMENT, not a max — see the module doc.
///
/// ⚠️ CLAMPED TO AT LEAST ONE SECOND, because `0` is the wire's sentinel for "never measured
/// here" and a sub-second step is a MEASUREMENT. Found at a real click, not reasoned about: a
/// no-op `brew upgrade jq` on an already-current formula elapsed under a second and wrote
/// `jq: brew/darwin: 0`, which the estimate would then count as an unknown forever — the row
/// that is most certainly fast would be the one the widget refuses to promise anything about.
/// One second is a lie of at most one second, in a display whose finest word is "<1 min".
///
/// The alternative — sub-second precision on the wire, or a separate "measured" flag — buys
/// nothing a user can see and costs a second field to keep in agreement. Rejected knowingly.
pub fn note_duration(t: &mut Timings, id: &str, at: &str, secs: u64) {
    t.entry(id.to_string())
        .or_default()
        .insert(at.to_string(), secs.max(1));
}

/// The file, in the LOCAL data-dir. Never the shared folder: the exe is launched by N
/// machines and this number is about THIS one.
pub fn timings_path(local_dir: &Path) -> PathBuf {
    local_dir.join("timings.yaml")
}

/// Reads the cache. Absent/unreadable/corrupt → empty, which the widget already renders
/// (a fresh machine has no timings at all).
pub fn read_timings(local_dir: &Path) -> Timings {
    match std::fs::read_to_string(timings_path(local_dir)) {
        Ok(raw) => parse_timings(&raw),
        Err(_) => Timings::new(),
    }
}

/// Persists the cache. Best-effort, like write_selection: a failed write costs one warm
/// estimate, never a session.
pub fn write_timings(local_dir: &Path, t: &Timings) {
    let _ = std::fs::create_dir_all(local_dir);
    if let Ok(raw) = serde_yaml::to_string(t) {
        let _ = std::fs::write(timings_path(local_dir), raw);
    }
}

/// Read what is cached, overwrite the entries this Apply measured, write it back.
///
/// Read-modify-write, which the SHARED file's design rejects for counters. Fine here for a
/// reason that does not transfer: there is exactly ONE writer, so there is no interleaving
/// to lose, and plain assignment is both correct and the honest semantics ("last time, it
/// took this long").
///
/// ⚠️ "One writer" means one Talos process per local data-dir, which is what the local dir
/// is FOR — it is per-machine, not per-share. Two Talos running at once on the same machine
/// would interleave here exactly as two machines do on the share, and the ratchet's
/// consolation ("no write is ever wrong") would NOT apply: assignment can lose the newer
/// number. That is accepted, not overlooked — the loss costs one warm estimate and the next
/// Apply overwrites it.
pub fn merge_and_write_timings(local_dir: &Path, fresh: &Timings) {
    let mut cached = read_timings(local_dir);
    for (id, per_route) in fresh {
        for (at, secs) in per_route {
            note_duration(&mut cached, id, at, *secs);
        }
    }
    write_timings(local_dir, &cached);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn parse_reads_a_package_then_route_os_map() {
        let raw = "\
aws-cli:
  brew/darwin: 38
7-zip:
  brew/darwin: 1
  winget/windows: 242
";
        let t = parse_timings(raw);
        assert_eq!(t["aws-cli"]["brew/darwin"], 38);
        assert_eq!(t["7-zip"]["winget/windows"], 242);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn parse_is_defensive_and_never_panics() {
        // Same discipline as parse_selection / parse_behaviour: a corrupt file must never
        // sink a session. And this one is EXPENDABLE in a way those are not — it is a
        // derived cache, so losing it costs one warm estimate.
        assert!(parse_timings("not yaml at all: [").is_empty());
        assert!(parse_timings("").is_empty());
        assert!(parse_timings("aws-cli: 42").is_empty());
    }

    /// THE property that separates this file from the shared one, and the reason it is
    /// pinned by a test: the monotone discipline (`∨`/`max`, no counters) existed ONLY
    /// because MANY machines write the shared file. This file has exactly ONE writer, so the
    /// carcan does not apply and assignment is correct — a package that got faster must be
    /// ABLE to say so, or a one-off slow run would freeze a wrong estimate forever.
    #[test]
    fn last_seen_wins_it_does_not_ratchet() {
        let mut t = Timings::new();
        note_duration(&mut t, "aws-cli", "brew/darwin", 240);
        note_duration(&mut t, "aws-cli", "brew/darwin", 12);
        assert_eq!(
            t["aws-cli"]["brew/darwin"], 12,
            "the LAST run wins, not the worst"
        );
        // And a sibling route is untouched.
        note_duration(&mut t, "aws-cli", "winget/windows", 500);
        assert_eq!(t["aws-cli"]["brew/darwin"], 12);
        assert_eq!(t["aws-cli"]["winget/windows"], 500);
    }

    /// A sub-second step is a MEASUREMENT, and `0` is the wire's sentinel for "never measured
    /// here" — so the two must not collide. Found at a real click: a no-op `brew upgrade jq`
    /// elapsed under a second and wrote `0`, which the estimate would have counted as unknown
    /// forever for the very row that is most certainly fast.
    #[test]
    fn a_sub_second_step_is_measured_not_unknown() {
        let mut t = Timings::new();
        note_duration(&mut t, "jq", "brew/darwin", 0);
        assert_eq!(
            t["jq"]["brew/darwin"], 1,
            "0 elapsed means fast, not unmeasured"
        );
        // And the clamp is a FLOOR, not a rounding: real durations pass through untouched,
        // including the 1 it clamps to.
        note_duration(&mut t, "jq", "brew/darwin", 1);
        assert_eq!(t["jq"]["brew/darwin"], 1);
        note_duration(&mut t, "jq", "brew/darwin", 38);
        assert_eq!(t["jq"]["brew/darwin"], 38);
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tmp("talos-test-timings-rt");
        let mut t = Timings::new();
        note_duration(&mut t, "aws-cli", "brew/darwin", 38);
        write_timings(&dir, &t);
        assert_eq!(read_timings(&dir), t);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Stated as a test because it is the PROMISE: this is a derived cache, not a record. An
    /// absent file reads as "no estimate yet", which the widget already has to render (a
    /// fresh machine has none), and the next Apply refills it.
    #[test]
    fn deleting_the_file_is_harmless() {
        let dir = tmp("talos-test-timings-absent");
        assert!(read_timings(&dir).is_empty());
        let mut t = Timings::new();
        note_duration(&mut t, "jq", "brew/darwin", 3);
        write_timings(&dir, &t);
        assert!(!read_timings(&dir).is_empty());
        let _ = std::fs::remove_file(timings_path(&dir));
        assert!(
            read_timings(&dir).is_empty(),
            "and it is empty again, not an error"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_and_write_keeps_untouched_packages_and_replaces_touched_ones() {
        // Read-modify-write, which the SHARED file's design rejects for counters. Fine
        // here for a reason that does not apply there: ONE writer. No interleaving to
        // lose, so plain assignment is both correct and the honest semantics.
        let dir = tmp("talos-test-timings-merge");
        let mut before = Timings::new();
        note_duration(&mut before, "jq", "brew/darwin", 3);
        note_duration(&mut before, "aws-cli", "brew/darwin", 240);
        write_timings(&dir, &before);

        let mut fresh = Timings::new();
        note_duration(&mut fresh, "aws-cli", "brew/darwin", 12);
        merge_and_write_timings(&dir, &fresh);

        let back = read_timings(&dir);
        assert_eq!(
            back["jq"]["brew/darwin"], 3,
            "a package this Apply never touched survives"
        );
        assert_eq!(
            back["aws-cli"]["brew/darwin"], 12,
            "a touched one is REPLACED, not maxed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_dir_is_survivable() {
        // Best-effort, same stance as write_selection: a failed write costs one warm
        // estimate, never a session. The blocker is a FILE where the folder must go, so
        // create_dir_all fails with ENOTDIR for any uid (a root container would make a
        // privileged path prove nothing).
        let dir = tmp("talos-test-timings-unwritable");
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, "I am a file where the folder should be").unwrap();
        let mut t = Timings::new();
        note_duration(&mut t, "jq", "brew/darwin", 3);
        write_timings(&dir, &t); // must not panic
        merge_and_write_timings(&dir, &t); // must not panic
        assert!(read_timings(&dir).is_empty());
        let _ = std::fs::remove_file(&dir);
    }
}
