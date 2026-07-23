//! selection.rs — the user's INTENT (the packages they toggled in/out
//! by hand), persisted locally to the machine. Port of src/selection.ts.
//!
//! Distinct from machine presence, always re-detected live ("detect, don't
//! remember" governs PRESENCE, NOT intent: a choice cannot be re-observed,
//! so it must be memorized). Same discipline as consent.rs: the PARSING is pure
//! and defensive (a corrupt file never sinks a session), the IO shell
//! never panics. Stored in the LOCAL data-dir, NEVER the shared exe folder.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Persisted intent: per-package toggle (name → "in"/"out") PLUS the personal
/// bundle's member list (`personal`: package names the user added from the
/// Catalog — spec §14). BTreeMap for deterministic writes / clean diffs.
/// Pure UI preferences (display only, not intent): kept in the SAME file so all
/// config lives in one place (spec — C: "toute la configuration au même endroit").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiPrefs {
    #[serde(default)]
    pub advanced: bool,
    #[serde(default, rename = "showAll")]
    pub show_all: bool,
    /// Gallery theme name ("" = the default Tokyo Night). Round-tripped verbatim;
    /// the front-end owns the set of valid names (see index.html [data-theme]).
    #[serde(default)]
    pub theme: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub pkgs: BTreeMap<String, String>,
    #[serde(default)]
    pub personal: Vec<String>,
    /// Names of the bundles the user has activated (the top cards). Restored so
    /// the cascade + card state survive a restart. `active_bundles` on the wire.
    #[serde(default, rename = "activeBundles")]
    pub active_bundles: Vec<String>,
    /// Display preferences (advanced mode, show-all) — one config file for all.
    #[serde(default)]
    pub ui: UiPrefs,
}

/// Read a JSON field as a Vec<String>, dropping non-string entries. Absent → empty.
fn string_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Pure, defensive: anything not well-formed → dropped. `pkgs` keeps only "in"/
/// "out" values; `personal` + `activeBundles` keep only string entries.
pub fn parse_selection(raw: &str) -> Selection {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Selection::default();
    };
    let mut pkgs = BTreeMap::new();
    if let Some(src) = v.get("pkgs").and_then(|p| p.as_object()) {
        for (k, val) in src {
            if let Some(s) = val.as_str() {
                if s == "in" || s == "out" {
                    pkgs.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    // UI prefs: parse the `ui` object with serde (missing/garbage → defaults).
    let ui = v
        .get("ui")
        .and_then(|u| serde_json::from_value::<UiPrefs>(u.clone()).ok())
        .unwrap_or_default();
    Selection {
        pkgs,
        personal: string_array(&v, "personal"),
        active_bundles: string_array(&v, "activeBundles"),
        ui,
    }
}

/// Path of the intent file, in the local data-dir.
fn selection_path(local_dir: &Path) -> PathBuf {
    local_dir.join("selection.json")
}

/// Reads the persisted intent. Absent/unreadable → empty (author's defaults).
pub fn read_selection(local_dir: &Path) -> Selection {
    match std::fs::read_to_string(selection_path(local_dir)) {
        Ok(raw) => parse_selection(&raw),
        Err(_) => Selection::default(),
    }
}

/// Persists the intent. Best-effort: a failed write never sinks a
/// session (at worst we restart from the author's defaults on the next launch).
pub fn write_selection(local_dir: &Path, sel: &Selection) {
    let _ = std::fs::create_dir_all(local_dir);
    if let Ok(json) = serde_json::to_string(sel) {
        let _ = std::fs::write(selection_path(local_dir), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_only_in_out() {
        let sel = parse_selection(r#"{"pkgs":{"a::x":"in","b::y":"out","c::z":"maybe"}}"#);
        assert_eq!(sel.pkgs.get("a::x"), Some(&"in".to_string()));
        assert_eq!(sel.pkgs.get("b::y"), Some(&"out".to_string()));
        assert_eq!(sel.pkgs.get("c::z"), None, "value outside in/out rejected");
    }

    #[test]
    fn parse_defensive_on_garbage() {
        assert_eq!(parse_selection("pas du json").pkgs.len(), 0);
        assert_eq!(parse_selection("{}").pkgs.len(), 0);
        assert_eq!(parse_selection(r#"{"pkgs":42}"#).pkgs.len(), 0);
        assert_eq!(parse_selection(r#"{"pkgs":{"k":true}}"#).pkgs.len(), 0);
    }

    #[test]
    fn parses_personal_list_dropping_non_strings() {
        let sel = parse_selection(r#"{"pkgs":{},"personal":["Git","rg",42]}"#);
        assert_eq!(sel.personal, vec!["Git".to_string(), "rg".to_string()]);
        // absent personal → empty (serde default), not an error
        assert!(parse_selection(r#"{"pkgs":{}}"#).personal.is_empty());
    }

    #[test]
    fn parses_active_bundles() {
        let sel = parse_selection(r#"{"pkgs":{},"activeBundles":["Base","Data",7]}"#);
        assert_eq!(
            sel.active_bundles,
            vec!["Base".to_string(), "Data".to_string()]
        );
        assert!(parse_selection(r#"{"pkgs":{}}"#).active_bundles.is_empty());
    }

    #[test]
    fn parses_ui_prefs() {
        let sel = parse_selection(r#"{"pkgs":{},"ui":{"advanced":true,"showAll":true}}"#);
        assert!(sel.ui.advanced && sel.ui.show_all);
        // theme round-trips verbatim; absent → "" (default, = Tokyo Night)
        assert_eq!(
            parse_selection(r#"{"pkgs":{},"ui":{"theme":"gruvbox"}}"#)
                .ui
                .theme,
            "gruvbox"
        );
        assert_eq!(sel.ui.theme, "");
        // absent ui → default false; garbage ui → default (never sinks)
        assert_eq!(parse_selection(r#"{"pkgs":{}}"#).ui, UiPrefs::default());
        assert_eq!(
            parse_selection(r#"{"pkgs":{},"ui":42}"#).ui,
            UiPrefs::default()
        );
    }

    #[test]
    fn read_absent_est_vide() {
        let dir = std::env::temp_dir().join("talos-test-sel-absent");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_selection(&dir).pkgs.len(), 0);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = std::env::temp_dir().join("talos-test-sel-rt");
        let _ = std::fs::remove_dir_all(&dir);
        let mut sel = Selection::default();
        sel.pkgs.insert("core::node".into(), "in".into());
        sel.pkgs.insert("extra::obsidian".into(), "out".into());
        write_selection(&dir, &sel);
        let back = read_selection(&dir);
        assert_eq!(back, sel);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
