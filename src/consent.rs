//! consent.rs — the install journal + the "share" consent, local to the machine.
//!
//! Two facts live in the LOCAL per-machine data-dir (%LOCALAPPDATA%\Talos on
//! Windows, ~/Library/Application Support/Talos on Mac — NEVER the shared exe
//! folder): whether the user consented to SHARE their history, and the history
//! itself (one JSON object per line). When (and only when) they consent, each
//! entry is ALSO copied into a shared file next to the exe —
//! logs/<host>/<user>.jsonl — so a team can see who installed what. No
//! consent → the shared copy is never written; the local journal is kept.
//!
//! The same journal also carries what the machine is and how fast it answered
//! (`machine.rs`: one `machine` line per launch, one `scan` line per scan), under
//! the same rule — so the consent covers them too, and the Machine tab shows them.
//!
//! Same discipline as detect.rs / outdated.rs: PARSING is pure (tested), the IO
//! is a thin shell that never panics (a journal that cannot write must not
//! sink an install).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A history entry (one JSONL line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistEntry {
    pub at: String,      // ISO timestamp
    pub package: String, // package name
    #[serde(default)]
    pub version: String, // captured for install/upgrade; "" if unknown
    #[serde(default)]
    pub action: String, // install | uninstall | upgrade
    #[serde(default)]
    pub ok: bool,
    /// How long the step took, in whole seconds. 0 means EITHER pre-feature (every
    /// line written before this field existed) OR sub-second, since `as_secs()`
    /// truncates — most plausibly a step that failed fast, since a successful one pays
    /// for the login-shell wrap and the re-probe on top of its own work. That ambiguity
    /// is harmless: the consumer keeps the WORST duration (a `max`), so a 0 never wins.
    ///
    /// ⚠️ `timings.rs` DISAGREES about 0, deliberately. There it means "never measured
    /// here" and nothing else, because the estimate must be able to tell an
    /// unmeasured row from a fast one — so a sub-second run is floored to 1s before it is
    /// stored. This journal keeps the raw truncation instead: it is the TRACE, and a 0 here
    /// is an honest "under a second or from before the field existed". Two files, two
    /// conventions, on purpose — do not "align" them.
    ///
    /// Absence reading as 0 rather than as an error is what lets the append-only
    /// journal keep parsing its own older lines — and `parse_history`, the reader the
    /// journal is actually read through, is hand-rolled: its `unwrap_or(0)` is the
    /// mechanism. `#[serde(default)]` covers the derived path, which nothing currently
    /// reads; it is here for consistency with the fields above it.
    #[serde(default)]
    pub secs: u64,
}

/// The consent state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Consent {
    pub decided: bool, // has the user answered the "share" question?
    pub share: bool,   // if decided: do they share their history?
}

/// Parse a JSONL file → entries. Ignores empty/malformed lines and any
/// object without a package name, so a single corrupted line does not sink the log.
pub fn parse_history(raw: &str) -> Vec<HistEntry> {
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue; // not JSON → line skipped, we keep the rest
        };
        let pkg = v.get("package").and_then(|p| p.as_str()).unwrap_or("");
        if pkg.is_empty() {
            continue;
        }
        out.push(HistEntry {
            at: v
                .get("at")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            package: pkg.to_string(),
            version: v
                .get("version")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            action: v
                .get("action")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            ok: v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
            secs: v.get("secs").and_then(|x| x.as_u64()).unwrap_or(0),
        });
    }
    out
}

/// Parse the consent file → {decided, share}. Absent or garbage → NOT
/// DECIDED (decided:false), which makes the first-launch dialog show.
pub fn parse_consent(raw: &str) -> Consent {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(share) = v.get("share").and_then(|s| s.as_bool()) {
            return Consent {
                decided: true,
                share,
            };
        }
    }
    Consent::default()
}

/// Where a consented copy lands, next to the exe: logs/<host>/<user>.jsonl.
/// Pure path construction (the separator follows the OS via PathBuf join).
pub fn shared_log_path(exe_dir: &Path, host: &str, user: &str) -> PathBuf {
    exe_dir
        .join("logs")
        .join(host)
        .join(format!("{user}.jsonl"))
}

/// Everything the IO needs, injected — so the shell is testable against
/// temporary folders and the server passes its real paths/identity.
#[derive(Debug, Clone)]
pub struct ConsentStore {
    pub local_dir: PathBuf, // LOCAL per-machine data-dir (%LOCALAPPDATA%\Talos)
    pub exe_dir: PathBuf,   // the shared folder where the exe sits (OneDrive)
    pub host: String,       // machine name — namespaces the shared log
    pub user: String,       // user name — the shared log's file name
}

fn consent_path(s: &ConsentStore) -> PathBuf {
    s.local_dir.join("consent.json")
}
fn local_hist_path(s: &ConsentStore) -> PathBuf {
    s.local_dir.join("history.jsonl")
}

/// Reads the consent state. Absent/unreadable → not decided (1st-boot dialog).
pub fn read_consent(s: &ConsentStore) -> Consent {
    match std::fs::read_to_string(consent_path(s)) {
        Ok(raw) => parse_consent(&raw),
        Err(_) => Consent::default(),
    }
}

/// Records the share choice (ALSO marks the consent as DECIDED).
pub fn write_consent(s: &ConsentStore, share: bool) {
    let _ = std::fs::create_dir_all(&s.local_dir);
    if let Ok(json) = serde_json::to_string(&serde_json::json!({ "share": share })) {
        let _ = std::fs::write(consent_path(s), json);
    }
}

/// Reads the local install history (display order is the UI's job).
pub fn read_history(s: &ConsentStore) -> Vec<HistEntry> {
    match std::fs::read_to_string(local_hist_path(s)) {
        Ok(raw) => parse_history(&raw),
        Err(_) => Vec::new(),
    }
}

/// Journals ONE outcome: ALWAYS to the local file; and — only if
/// the user consented to share — ALSO appended to the shared file
/// next to the exe. Both best-effort: a journal that cannot write must
/// never sink an install.
pub fn append_history(s: &ConsentStore, entry: &HistEntry) {
    append_record(s, entry);
}

/// Journals any record — the `machine` and `scan` lines of `machine.rs` — under the SAME
/// rule as an install: local always, shared only with the consent. One journal, one rule;
/// the install reader (`parse_history`) skips them because they name no package.
pub fn append_record<T: Serialize>(s: &ConsentStore, record: &T) {
    use std::io::Write;
    let Ok(mut line) = serde_json::to_string(record) else {
        return;
    };
    line.push('\n');
    let _held = journal_lock();
    // Local: always.
    let _ = std::fs::create_dir_all(&s.local_dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(local_hist_path(s))
    {
        let _ = f.write_all(line.as_bytes());
    }
    compact_if_large(&local_hist_path(s));
    append_shared(s, &line);
}

/// ONE writer at a time, in this process. A compaction reads the journal, writes it beside
/// and renames it over: an install line appended by another task in that window would land
/// in the replaced file and vanish — breaking the one promise compaction makes, that every
/// install is kept. The profile task, the scan and an Apply's steps all write. Held across
/// the append AND the compaction. (Two Talos instances for one user stay unserialised; the
/// port self-heal kills a stale one at launch.)
static JOURNAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn journal_lock() -> std::sync::MutexGuard<'static, ()> {
    JOURNAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Above this size a journal is compacted (see `compact_journal`).
const COMPACT_ABOVE: u64 = 2 * 1024 * 1024;
/// What a compaction keeps of the two records written without a user's gesture.
const KEEP_MACHINES: usize = 100;
const KEEP_SCANS: usize = 200;

/// Shrinks a journal that grew past `COMPACT_ABOVE`: keeps EVERY install line (the trace
/// `timings.rs` defers to, and what the History tab lists) and every line it does not
/// recognise, but only the last `KEEP_MACHINES` machine profiles and `KEEP_SCANS` scans.
/// Those two are written on every launch and every scan — a scan line is 1–3 KB — so
/// without this the journal grew forever, on the local disk and on the share alike.
/// Order is preserved. Pure, so it is tested; `compact_if_large` is the IO.
pub fn compact_journal(raw: &str) -> String {
    let kind = |line: &str| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(String::from))
    };
    let kinds: Vec<Option<String>> = raw.lines().map(kind).collect();
    let count = |k: &str| kinds.iter().filter(|x| x.as_deref() == Some(k)).count();
    let (mut drop_machines, mut drop_scans) = (
        count("machine").saturating_sub(KEEP_MACHINES),
        count("scan").saturating_sub(KEEP_SCANS),
    );
    let mut out = String::with_capacity(raw.len());
    for (line, k) in raw.lines().zip(&kinds) {
        let drop = match k.as_deref() {
            Some("machine") if drop_machines > 0 => {
                drop_machines -= 1;
                true
            }
            Some("scan") if drop_scans > 0 => {
                drop_scans -= 1;
                true
            }
            _ => false,
        };
        if !drop {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Best-effort, like every journal write: a compaction that fails leaves the file as it
/// was. Written beside and renamed over, so a crash never leaves half a journal.
fn compact_if_large(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= COMPACT_ABOVE {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let tmp = path.with_extension("jsonl.tmp");
    let renamed =
        std::fs::write(&tmp, compact_journal(&raw)).is_ok() && std::fs::rename(&tmp, path).is_ok();
    if !renamed {
        // A sync client or an antivirus holding the journal: try again on the next write,
        // and leave no `.tmp` behind to be synced to the share meanwhile.
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Copies a record to the SHARED log only, when the consent allows it — for the machine
/// profile of a launch whose user turns sharing on mid-session: it was journalled locally
/// at collection time, and the Machine tab says the share receives it. Without this, the share
/// would see no profile until the next launch while the card claimed otherwise.
pub fn share_record<T: Serialize>(s: &ConsentStore, record: &T) {
    if let Ok(mut line) = serde_json::to_string(record) {
        line.push('\n');
        let _held = journal_lock();
        append_shared(s, &line);
    }
}

fn append_shared(s: &ConsentStore, line: &str) {
    use std::io::Write;
    // Shared: only if consented (the shared folder may be offline →
    // the local copy stays kept no matter what).
    if read_consent(s).share {
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        if let Some(parent) = shared.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&shared)
        {
            let _ = f.write_all(line.as_bytes());
        }
        compact_if_large(&shared);
    }
}

/// Clears the LOCAL history only. The shared team log is a record
/// others depend on — clearing one's own view must not clear what
/// the team has already seen.
pub fn clear_history(s: &ConsentStore) {
    let _held = journal_lock();
    let _ = std::fs::remove_file(local_hist_path(s));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> ConsentStore {
        ConsentStore {
            local_dir: dir.to_path_buf(),
            exe_dir: dir.join("shared"),
            host: "hostA".into(),
            user: "userB".into(),
        }
    }

    #[test]
    fn parse_history_ignores_corrupt_lines() {
        let raw = "\n{bad json}\n{\"package\":\"node\",\"at\":\"t\",\"action\":\"install\",\"ok\":true}\n{\"no\":\"pkg\"}\n";
        let h = parse_history(raw);
        assert_eq!(h.len(), 1, "only the valid-with-package line survives");
        assert_eq!(h[0].package, "node");
        assert!(h[0].ok);
    }

    #[test]
    fn parse_consent_defensive() {
        assert_eq!(
            parse_consent("garbage"),
            Consent {
                decided: false,
                share: false
            }
        );
        assert_eq!(
            parse_consent("{}"),
            Consent {
                decided: false,
                share: false
            }
        );
        assert_eq!(
            parse_consent(r#"{"share":true}"#),
            Consent {
                decided: true,
                share: true
            }
        );
        assert_eq!(
            parse_consent(r#"{"share":false}"#),
            Consent {
                decided: true,
                share: false
            }
        );
    }

    #[test]
    fn shared_path_namespace_host_user() {
        let p = shared_log_path(Path::new("/exe"), "H", "U");
        assert_eq!(p, PathBuf::from("/exe/logs/H/U.jsonl"));
    }

    #[test]
    fn consent_roundtrip_marks_decided() {
        let dir = std::env::temp_dir().join("talos-test-consent-rt");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        assert_eq!(read_consent(&s), Consent::default(), "absent → not decided");
        write_consent(&s, true);
        assert_eq!(
            read_consent(&s),
            Consent {
                decided: true,
                share: true
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_local_always_shared_only_if_consented() {
        let dir = std::env::temp_dir().join("talos-test-consent-append");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        let e = HistEntry {
            at: "2026-07-19T12:00".into(),
            package: "bat".into(),
            version: "0.24".into(),
            action: "install".into(),
            ok: true,
            secs: 0,
        };
        // No consent yet → local written, shared ABSENT.
        append_history(&s, &e);
        assert_eq!(read_history(&s).len(), 1);
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        assert!(!shared.exists(), "without consent, no shared copy");
        // Consent share → the next entry is ALSO shared.
        write_consent(&s, true);
        append_history(&s, &e);
        assert_eq!(read_history(&s).len(), 2);
        assert!(shared.exists(), "with consent, shared copy written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_machine_record_follows_the_install_rule_and_the_install_reader_skips_it() {
        let dir = std::env::temp_dir().join("talos-test-consent-record");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        let record = serde_json::json!({ "kind": "machine", "at": "t", "cores": 8 });
        append_record(&s, &record);
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        assert!(!shared.exists(), "without consent, no shared copy");
        let local = std::fs::read_to_string(local_hist_path(&s)).unwrap();
        assert!(local.contains("\"kind\":\"machine\""), "{local}");
        assert!(
            read_history(&s).is_empty(),
            "a machine line is not an install"
        );
        write_consent(&s, true);
        append_record(&s, &record);
        assert!(shared.exists(), "with consent, shared copy written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sharing_a_record_writes_the_shared_log_only_and_only_with_consent() {
        let dir = std::env::temp_dir().join("talos-test-consent-share-record");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        let record = serde_json::json!({ "kind": "machine", "at": "t" });
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        share_record(&s, &record);
        assert!(!shared.exists(), "without consent, nothing is shared");
        write_consent(&s, true);
        share_record(&s, &record);
        let text = std::fs::read_to_string(&shared).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            !local_hist_path(&s).exists(),
            "the local journal is not written twice"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compaction_keeps_every_install_and_only_the_latest_profiles_and_scans() {
        let mut raw = String::new();
        raw.push_str("{\"package\":\"jq\",\"at\":\"first\"}\n");
        for i in 0..(KEEP_SCANS + 5) {
            raw.push_str(&format!("{{\"kind\":\"scan\",\"at\":\"s{i}\"}}\n"));
        }
        for i in 0..(KEEP_MACHINES + 3) {
            raw.push_str(&format!("{{\"kind\":\"machine\",\"at\":\"m{i}\"}}\n"));
        }
        raw.push_str("not json, kept\n{\"package\":\"git\",\"at\":\"last\"}\n");
        let out = compact_journal(&raw);
        let installs = parse_history(&out);
        assert_eq!(installs.len(), 2, "every install survives");
        assert_eq!(out.matches("\"kind\":\"scan\"").count(), KEEP_SCANS);
        assert_eq!(out.matches("\"kind\":\"machine\"").count(), KEEP_MACHINES);
        assert!(!out.contains("\"s4\""), "the OLDEST scans go");
        assert!(out.contains("\"s5\"") && out.contains(&format!("\"s{}\"", KEEP_SCANS + 4)));
        assert!(out.contains("not json, kept"));
        assert!(out.starts_with("{\"package\":\"jq\""), "order is kept");
        // Under the limits, nothing changes.
        let small = "{\"kind\":\"scan\"}\n{\"package\":\"jq\"}\n";
        assert_eq!(compact_journal(small), small);
    }

    #[test]
    fn a_journal_past_the_limit_is_compacted_on_the_next_write() {
        let dir = std::env::temp_dir().join("talos-test-consent-compact");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pad = "x".repeat(3000);
        let mut raw = String::new();
        while (raw.len() as u64) <= COMPACT_ABOVE {
            raw.push_str(&format!("{{\"kind\":\"scan\",\"pad\":\"{pad}\"}}\n"));
        }
        std::fs::write(local_hist_path(&s), &raw).unwrap();
        append_record(&s, &serde_json::json!({ "kind": "scan", "at": "new" }));
        let after = std::fs::read_to_string(local_hist_path(&s)).unwrap();
        assert_eq!(
            after.lines().count(),
            KEEP_SCANS,
            "compacted to the scans kept"
        );
        assert!(
            after.lines().last().unwrap().contains("\"new\""),
            "the newest survives"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_wipes_local_not_shared() {
        let dir = std::env::temp_dir().join("talos-test-consent-clear");
        let _ = std::fs::remove_dir_all(&dir);
        let s = store(&dir);
        write_consent(&s, true);
        let e = HistEntry {
            at: "t".into(),
            package: "bat".into(),
            version: "".into(),
            action: "install".into(),
            ok: true,
            secs: 0,
        };
        append_history(&s, &e);
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        assert!(shared.exists());
        clear_history(&s);
        assert_eq!(read_history(&s).len(), 0, "local emptied");
        assert!(shared.exists(), "the shared team log stays intact");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_roundtrips_the_duration() {
        let e = HistEntry {
            at: "2026-08-01T10:00:00Z".into(),
            package: "AWS CLI".into(),
            version: "2.36.13".into(),
            action: "install".into(),
            ok: true,
            secs: 242,
        };
        let line = serde_json::to_string(&e).unwrap();
        let back = parse_history(&format!("{line}\n"));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].secs, 242);
    }

    #[test]
    fn history_without_a_duration_reads_zero() {
        // Every line written before this feature has no `secs`. They must still
        // parse — a journal that rejects its own history is worse than one with a
        // gap, and 0 reads honestly as "not measured".
        let old = r#"{"at":"2026-07-01T10:00:00Z","package":"jq","version":"1.8.2","action":"install","ok":true}"#;
        let back = parse_history(&format!("{old}\n"));
        assert_eq!(back.len(), 1, "an old line must still parse");
        assert_eq!(back[0].secs, 0, "absent duration → 0, not an error");
    }
}
