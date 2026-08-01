//! behaviour_io.rs — the thin IO shell for the behaviour files.
//!
//! Same split as consent.rs / selection.rs: the RULE is pure and tested elsewhere
//! (behaviour.rs), this file only touches the disk and never panics. It sits on a
//! SHARED folder written by other machines, so every read tolerates anything and every
//! write is best-effort — a share that is offline must not sink an Apply.
//!
//! One file per package, mirroring catalog/: same key (the file stem), so the
//! correspondence needs no mapping. That layout also narrows the window in which two
//! machines can collide at all — they must touch the SAME package at the same moment.
//! It does not eliminate it: a write is a whole-file overwrite, so an interleaved
//! read/read/write/write DOES drop the earlier machine's facts. What the ratchet buys is
//! that the dropped facts were never WRONG, and that whoever observes them next merges them
//! back — recovery is by RE-OBSERVATION, not by merging alone, since after the overwrite no
//! machine still holds the lost value. A delay, not a corruption. See behaviour.rs's module
//! doc for the full argument.

use crate::behaviour::{merge_into, parse_behaviour, parse_failed, to_yaml, Record};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ONE `allow(dead_code)` is left below, on `read_all`. The other one went when the Apply
// path started flushing its observations through `merge_and_write` — which, being reached,
// now keeps `read_one`, `write_one`, `behaviour_path`, `behaviour_dir` and
// `note_if_whole_file_loss` alive without any allow of their own.
//
// `read_all` outlives this plan on purpose: nothing in it reads the facts back. Its first
// caller is the ladder's own plan, which loads them at startup beside the catalogue.
// Measured by stripping it under `-D warnings`: without it, one warning returns.

/// The folder holding every package's facts. Single-sourced here so `behaviour_path` and
/// `read_all` cannot drift apart about where the layout lives.
fn behaviour_dir(exe_dir: &Path) -> PathBuf {
    exe_dir.join("behaviour")
}

/// Where one package's facts live: `<shared>/behaviour/<id>.yaml`.
/// Pure path construction, like consent::shared_log_path.
pub fn behaviour_path(exe_dir: &Path, id: &str) -> PathBuf {
    behaviour_dir(exe_dir).join(format!("{id}.yaml"))
}

/// Report a file whose whole content was thrown away, on stdout, in the style of
/// `[scan]`/`[plan]`. `parse_behaviour` is all-or-nothing, so ONE malformed entry discards
/// every valid fact in the file — and it returns the same empty record as a file that never
/// existed. `parse_failed` is what tells the two apart; this shell must not try to guess it
/// from the bytes, because "non-empty yet empty record" is false for six shapes that parse
/// cleanly (see `behaviour::parse_failed`).
///
/// Not an error: the facts return once someone re-observes them. But a file silently
/// emptying itself on a share is invisible otherwise, and invisible is how a bad file
/// survives for months.
///
/// ⚠️ TWO LIMITS, both deliberate. This is a CONSOLE diagnostic only: `main.rs` sets
/// `windows_subsystem = "windows"` in release, so on the Windows fleet — the very case this
/// notice was written for — a release build has no console and this line goes nowhere. It
/// helps a dev run, a Mac run, and a debug build. Making it reach a real operator needs a
/// log sink threaded in from the server, which would ripple into the Apply wiring and is
/// deferred rather than forgotten. And it is printed once per READ, with no per-process
/// dedup: a run that calls `read_all` at startup and `merge_and_write` at flush prints the
/// same path twice.
///
/// ⚠️ AND THE EVIDENCE DOES NOT SURVIVE: `merge_and_write` overwrites the file it just
/// failed to read, so the offending bytes are gone by the time anyone reads this line. That
/// is self-healing — the file becomes valid again — but it means the notice is the only
/// trace left of what was in there.
fn note_if_whole_file_loss(path: &Path, raw: &str) {
    if parse_failed(raw) {
        #[cfg(test)]
        NOTICES.with(|n| n.set(n.get() + 1));
        println!(
            "[behaviour] unreadable, every fact in it ignored: {}",
            path.display()
        );
    }
}

// A `println!` cannot be asserted, so without this the CALL SITES above were unpinned:
// deleting both left the whole suite green (found in review, not by the suite). Counting the
// notices is the cheapest thing that pins them — thread-local because the test harness gives
// each test its own thread, so two tests reading files concurrently cannot see each other's
// count. Test-only, and deliberately NOT a `log: &dyn Fn(&str)` parameter: a real sink is
// wanted eventually (see the release-Windows limit above) but threading one through now would
// ripple into the Apply wiring, which belongs to a later task.
#[cfg(test)]
thread_local! {
    static NOTICES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One package's facts from the share. Absent or malformed → empty (never an error):
/// "we know nothing about this package" is a perfectly good answer.
pub fn read_one(exe_dir: &Path, id: &str) -> Record {
    let path = behaviour_path(exe_dir, id);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            note_if_whole_file_loss(&path, &raw);
            parse_behaviour(&raw)
        }
        Err(_) => Record::new(),
    }
}

/// Every package's facts, keyed by package id. Called ONCE at startup, beside the
/// catalogue — not per Apply, so the plan cannot shift under a running one.
#[allow(dead_code)] // no caller in THIS plan — the ladder's first task reads the facts
pub fn read_all(exe_dir: &Path) -> BTreeMap<String, Record> {
    let mut out = BTreeMap::new();
    let dir = behaviour_dir(exe_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out; // no behaviour folder yet → nothing known, which is fine
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        // A corrupt file yields an EMPTY record rather than being skipped. The asymmetry is
        // deliberate, not a feature anyone consumes: no caller needs "we have a file and
        // learned nothing" told apart from "no file at all", and `read_one` does not preserve
        // the distinction anyway. Inserting is simply the choice that discards less.
        note_if_whole_file_loss(&path, &raw);
        out.insert(stem.to_string(), parse_behaviour(&raw));
    }
    out
}

/// Overwrite one package's file. Best-effort: a missing folder is created, and any
/// failure is silently dropped (the share may be offline or read-only).
pub fn write_one(exe_dir: &Path, id: &str, rec: &Record) {
    let raw = to_yaml(rec);
    if raw.is_empty() {
        // Serialisation failed — better no update than a truncated file. This is NOT the
        // "nothing to write" case: an empty record serialises to `{}\n`, not to "".
        return;
    }
    let path = behaviour_path(exe_dir, id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, raw);
}

/// Read what is on the share, ratchet our observations into it, write it back.
///
/// This is read-modify-write, which the design explicitly avoids for COUNTERS. It is
/// tolerable here for the reason that made monotone facts the choice — but be exact about
/// what that reason is, because the tempting version of it is false.
///
/// ⚠️ AN INTERLEAVED WRITE IS LOST. We read `O`, another machine writes `O ∨ b`, then we
/// write `O ∨ ours`: the file no longer contains `b`, and no later merge brings it back,
/// because after the overwrite no machine still holds `b` to merge. Recovery is by
/// RE-OBSERVATION — whoever next sees that behaviour merges it in again — which works only
/// because these facts are observed rather than authored.
///
/// What monotonicity actually buys is therefore narrower than convergence, and enough: no
/// write can ever produce a WRONG value, and every loss is temporary. A counter would fail
/// both — a dropped increment is permanently and plausibly false, and nothing can detect it.
pub fn merge_and_write(exe_dir: &Path, id: &str, observed: &Record) {
    let mut on_disk = read_one(exe_dir, id);
    for (k, obs) in observed {
        merge_into(&mut on_disk, k, obs);
    }
    write_one(exe_dir, id, &on_disk);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behaviour::Facts;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// How many whole-file-loss notices this test's thread has emitted.
    fn notices() -> usize {
        NOTICES.with(|n| n.get())
    }

    #[test]
    fn path_is_one_file_per_package_under_behaviour() {
        let p = behaviour_path(std::path::Path::new("/Apps/OneDrive"), "aws-cli");
        assert!(p.ends_with("behaviour/aws-cli.yaml"), "got {p:?}");
    }

    #[test]
    fn write_then_read_roundtrips_one_package() {
        let dir = tmp("talos-test-behaviour-rt");
        let mut rec = crate::behaviour::Record::new();
        rec.insert(
            "winget/windows".into(),
            Facts {
                uac: true,
                forbidden: false,
                slow_secs: 242,
            },
        );
        write_one(&dir, "aws-cli", &rec);
        let back = read_one(&dir, "aws-cli");
        assert_eq!(back, rec);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_an_absent_file_is_empty_not_an_error() {
        let dir = tmp("talos-test-behaviour-absent");
        assert!(read_one(&dir, "nothing-here").is_empty());
    }

    #[test]
    fn read_all_collects_every_package_and_ignores_junk() {
        let dir = tmp("talos-test-behaviour-all");
        let bdir = dir.join("behaviour");
        std::fs::create_dir_all(&bdir).unwrap();
        std::fs::write(bdir.join("aws-cli.yaml"), "winget/windows:\n  uac: true\n").unwrap();
        std::fs::write(
            bdir.join("rclone.yaml"),
            "winget/windows:\n  \"403\": true\n",
        )
        .unwrap();
        // A non-yaml file and a corrupt one must both be survivable: the share is written
        // by other machines and may hold anything. Neither may error or panic — the corrupt
        // one is logged, because losing a whole file's facts is a real loss, but it is
        // still not fatal.
        std::fs::write(bdir.join("README.txt"), "not a package").unwrap();
        std::fs::write(bdir.join("broken.yaml"), "this: [is: not").unwrap();
        let all = read_all(&dir);
        assert!(all.get("aws-cli").unwrap()["winget/windows"].uac);
        assert!(all.get("rclone").unwrap()["winget/windows"].forbidden);
        // (`!contains_key`, not `get(..).is_none()`: same assertion, and clippy's
        // `unnecessary_get_then_check` rejects the latter under `-D warnings`.)
        assert!(!all.contains_key("README"), "non-yaml skipped");
        assert_eq!(
            all.get("broken").map(|r| r.is_empty()),
            Some(true),
            "a corrupt file yields an EMPTY record, not a missing key or a panic"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_merges_into_what_is_already_on_disk() {
        // The point of the ratchet: another machine's facts must survive ours.
        //
        // This is the SEQUENTIAL case — their write completes before ours begins — and it is
        // the only one a test can reach. It does NOT cover interleaving (read/read/write/
        // write), which loses the earlier facts by construction and is why
        // merge_and_write's doc says recovery is by re-observation.
        let dir = tmp("talos-test-behaviour-merge");
        let mut theirs = crate::behaviour::Record::new();
        theirs.insert(
            "winget/windows".into(),
            Facts {
                uac: true,
                forbidden: false,
                slow_secs: 500,
            },
        );
        write_one(&dir, "vscode", &theirs);
        // Now WE observed a shorter run and no elevation. Nothing may be lost.
        let mut ours = crate::behaviour::Record::new();
        ours.insert(
            "winget/windows".into(),
            Facts {
                uac: false,
                forbidden: true,
                slow_secs: 12,
            },
        );
        merge_and_write(&dir, "vscode", &ours);
        let back = read_one(&dir, "vscode");
        let f = back["winget/windows"];
        assert!(f.uac, "their uac survived our quiet run");
        assert!(f.forbidden, "our 403 was added");
        assert_eq!(f.slow_secs, 500, "the worst duration survived");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn both_read_paths_classify_a_corrupt_file_as_a_loss_and_an_empty_one_as_fine() {
        // Pins the WIRING, not the rule. `parse_failed` is tested in behaviour.rs; what can
        // still silently rot is this shell forgetting to consult it, which no assertion
        // caught until the classification was reachable from a test. It is asserted on the
        // same bytes each read path holds, which is the closest a println can be pinned
        // without a log sink to capture (see note_if_whole_file_loss's limits).
        let dir = tmp("talos-test-behaviour-classify");
        let bdir = dir.join("behaviour");
        std::fs::create_dir_all(&bdir).unwrap();
        std::fs::write(
            bdir.join("broken.yaml"),
            "good/darwin:\n  uac: true\nbad: 42\n",
        )
        .unwrap();
        std::fs::write(bdir.join("blank.yaml"), "{}\n# written 2026-08-01\n").unwrap();
        std::fs::write(bdir.join("fine.yaml"), "brew/darwin:\n  slow_secs: 38\n").unwrap();

        // Corrupt: valid siblings WERE discarded, so read_one must report exactly one loss.
        let before = notices();
        assert!(read_one(&dir, "broken").is_empty(), "the record is empty");
        assert_eq!(
            notices() - before,
            1,
            "read_one must CONSULT the classifier, not merely have one available"
        );

        // Empty-but-valid: nothing was lost, so it must NOT be reported. This exact shape —
        // `{}` plus a date comment — is what the format allows an empty file to carry, and it
        // is what defeated the byte-level heuristic this replaced. It is indistinguishable
        // from the corrupt case by the returned record alone, which is the whole point.
        let before = notices();
        assert!(read_one(&dir, "blank").is_empty(), "empty either way");
        assert!(read_one(&dir, "absent").is_empty(), "and so is no file");
        assert_eq!(notices() - before, 0, "neither lost anything to report");
        // Including what WE write for an empty record.
        assert!(!parse_failed(&to_yaml(&Record::new())));

        // read_all sees the same three files, reports the ONE real loss, keeps all three
        // keys, and panics on none of them. Asserted separately because the two call sites
        // can rot independently.
        let before = notices();
        let all = read_all(&dir);
        assert_eq!(notices() - before, 1, "read_all must consult it too");
        assert_eq!(all.len(), 3, "all three kept: {all:?}");
        assert!(all["broken"].is_empty());
        assert!(all["blank"].is_empty());
        assert_eq!(all["fine"]["brew/darwin"].slow_secs, 38);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_share_is_survivable() {
        // The share may be offline. A failed write must be a no-op, never a panic — same
        // stance as append_history's shared copy.
        //
        // The unwritable parent is a FILE, not a privileged path: create_dir_all then fails
        // with ENOTDIR for any uid, so this stays a real test inside a root container (where
        // "/" is writable and an absolute path would have proved nothing). And it asserts the
        // outcome rather than mere survival — a test whose only claim is "did not panic"
        // passes just as happily when the write succeeds.
        let dir = tmp("talos-test-behaviour-unwritable");
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("behaviour");
        std::fs::write(&blocker, "I am a file where the folder should be").unwrap();

        let mut rec = crate::behaviour::Record::new();
        rec.insert("winget/windows".into(), Facts::default());
        write_one(&dir, "whatever", &rec); // must not panic
        merge_and_write(&dir, "whatever", &rec); // must not panic

        assert!(
            !behaviour_path(&dir, "whatever").exists(),
            "no file may be conjured through an unwritable parent"
        );
        assert!(
            std::fs::read_to_string(&blocker)
                .unwrap()
                .starts_with("I am"),
            "and the thing in the way was not clobbered"
        );
        assert!(
            read_one(&dir, "whatever").is_empty(),
            "reading back through the same broken parent is empty, not a panic"
        );
        assert!(read_all(&dir).is_empty(), "nor does read_all trip over it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
