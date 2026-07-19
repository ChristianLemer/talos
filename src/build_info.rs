//! build_info.rs — reads the STAMP baked by build.rs (compile-time env variables)
//! and exposes it to the front. Rust counterpart of build-info.ts: {change, sha, builtAt}.
//! The front (app.js showBuild) turns it into the title `Talos · <change>` and the footer
//! `build <change> · <sha> — <builtAt>`. Answers "which binary is really running?".

use serde_json::{json, Value};

/// change id (stable across amends) — leads, like `jj log`. "git"/"unknown" as fallback.
pub const CHANGE: &str = env!("TALOS_BUILD_CHANGE");
/// short git-sha (moves on every edit snapshot).
pub const SHA: &str = env!("TALOS_BUILD_SHA");
/// build timestamp ISO 8601 UTC.
pub const BUILT_AT: &str = env!("TALOS_BUILD_AT");

/// The stamp as JSON, with the EXACT keys the front expects (change/sha/builtAt).
pub fn build_json() -> Value {
    json!({ "change": CHANGE, "sha": SHA, "builtAt": BUILT_AT })
}

/// Startup log line — mirrors the order of `jj log` (change leads, sha follows).
pub fn start_line() -> String {
    format!("build={CHANGE} ({SHA}) built={BUILT_AT}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_bake_non_vide() {
        // build.rs must have baked all 3 (jj/git/unknown — never empty).
        assert!(!CHANGE.is_empty(), "change baked");
        assert!(!SHA.is_empty(), "sha baked");
        assert!(!BUILT_AT.is_empty(), "builtAt baked");
    }

    #[test]
    fn json_a_les_cles_du_front() {
        let j = build_json();
        assert!(j.get("change").is_some());
        assert!(j.get("sha").is_some());
        assert!(j.get("builtAt").is_some(), "front reads builtAt (camelCase)");
    }

    #[test]
    fn built_at_ressemble_iso() {
        // Except "unknown" impossible here (build.rs always sets a timestamp),
        // the stamp must look like an ISO UTC date (…T…Z).
        assert!(BUILT_AT.contains('T') && BUILT_AT.ends_with('Z'), "got {BUILT_AT}");
    }
}
