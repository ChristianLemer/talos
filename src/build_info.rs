//! build_info.rs — lit le STAMP baké par build.rs (variables d'env de compilation)
//! et l'expose au front. Pendant Rust de build-info.ts : {change, sha, builtAt}.
//! Le front (app.js showBuild) en fait le titre `Talos · <change>` et le footer
//! `build <change> · <sha> — <builtAt>`. Répond à "quel binaire tourne vraiment ?".

use serde_json::{json, Value};

/// change id (stable across amends) — mène, comme `jj log`. "git"/"unknown" en fallback.
pub const CHANGE: &str = env!("TALOS_BUILD_CHANGE");
/// git-sha court (bouge à chaque snapshot d'édition).
pub const SHA: &str = env!("TALOS_BUILD_SHA");
/// timestamp de build ISO 8601 UTC.
pub const BUILT_AT: &str = env!("TALOS_BUILD_AT");

/// Le stamp en JSON, aux clés EXACTES que le front attend (change/sha/builtAt).
pub fn build_json() -> Value {
    json!({ "change": CHANGE, "sha": SHA, "builtAt": BUILT_AT })
}

/// Ligne de log de démarrage — ordre miroir de `jj log` (change mène, sha suit).
pub fn start_line() -> String {
    format!("build={CHANGE} ({SHA}) built={BUILT_AT}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_bake_non_vide() {
        // build.rs a dû baker les 3 (jj/git/unknown — jamais vide).
        assert!(!CHANGE.is_empty(), "change baké");
        assert!(!SHA.is_empty(), "sha baké");
        assert!(!BUILT_AT.is_empty(), "builtAt baké");
    }

    #[test]
    fn json_a_les_cles_du_front() {
        let j = build_json();
        assert!(j.get("change").is_some());
        assert!(j.get("sha").is_some());
        assert!(j.get("builtAt").is_some(), "front lit builtAt (camelCase)");
    }

    #[test]
    fn built_at_ressemble_iso() {
        // Sauf "unknown" impossible ici (build.rs met toujours un timestamp),
        // le stamp doit ressembler à une date ISO UTC (…T…Z).
        assert!(BUILT_AT.contains('T') && BUILT_AT.ends_with('Z'), "got {BUILT_AT}");
    }
}
