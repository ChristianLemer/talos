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
    use std::io::Write;
    let Ok(mut line) = serde_json::to_string(entry) else {
        return;
    };
    line.push('\n');
    // Local: always.
    let _ = std::fs::create_dir_all(&s.local_dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(local_hist_path(s))
    {
        let _ = f.write_all(line.as_bytes());
    }
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
    }
}

/// Clears the LOCAL history only. The shared team log is a record
/// others depend on — clearing one's own view must not clear what
/// the team has already seen.
pub fn clear_history(s: &ConsentStore) {
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
        };
        append_history(&s, &e);
        let shared = shared_log_path(&s.exe_dir, &s.host, &s.user);
        assert!(shared.exists());
        clear_history(&s);
        assert_eq!(read_history(&s).len(), 0, "local emptied");
        assert!(shared.exists(), "the shared team log stays intact");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
