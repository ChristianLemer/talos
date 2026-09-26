//! build_info.rs — reads the STAMP baked by build.rs (compile-time env variables)
//! and exposes it to the front. Shape sent to the front: {tag, sha, builtAt}.
//! The front (app.js showBuild) turns it into the title `Talos · <tag>` and the footer
//! `<tag> · <sha> — <builtAt>`. Answers "which binary is really running?".

use serde_json::{json, Value};

/// release tag (v0.0.1-beta.4) — the human-facing version. Locally it is the describe
/// string: `-<n>-g<sha>` when ahead of the tag, `-dirty` when built on uncommitted
/// edits. "untagged" when no tag is reachable.
pub const TAG: &str = env!("TALOS_BUILD_TAG");
/// short git-sha of HEAD at compile time; "unknown" outside a checkout.
pub const SHA: &str = env!("TALOS_BUILD_SHA");
/// build timestamp ISO 8601 UTC.
pub const BUILT_AT: &str = env!("TALOS_BUILD_AT");

/// The stamp as JSON, with the EXACT keys the front expects (tag/sha/builtAt).
pub fn build_json() -> Value {
    json!({ "tag": TAG, "sha": SHA, "builtAt": BUILT_AT })
}

/// Startup log line — tag leads (the release version), then the exact snapshot.
pub fn start_line() -> String {
    format!("tag={TAG} sha={SHA} built={BUILT_AT}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_bake_non_vide() {
        // build.rs must have baked all 3 (tag/sha/builtAt — never empty).
        assert!(!TAG.is_empty(), "tag baked");
        assert!(!SHA.is_empty(), "sha baked");
        assert!(!BUILT_AT.is_empty(), "builtAt baked");
    }

    #[test]
    fn json_a_les_cles_du_front() {
        let j = build_json();
        assert!(j.get("tag").is_some());
        assert!(j.get("sha").is_some());
        assert!(
            j.get("builtAt").is_some(),
            "front reads builtAt (camelCase)"
        );
        // The jj change id left with jj (2026-09-05); a front that still reads it
        // would print "undefined" in the title.
        assert!(
            j.get("change").is_none(),
            "no change id in a git-only stamp"
        );
    }

    #[test]
    fn built_at_ressemble_iso() {
        // Except "unknown" impossible here (build.rs always sets a timestamp),
        // the stamp must look like an ISO UTC date (…T…Z).
        assert!(
            BUILT_AT.contains('T') && BUILT_AT.ends_with('Z'),
            "got {BUILT_AT}"
        );
    }
}
