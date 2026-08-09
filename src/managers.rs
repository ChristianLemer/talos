// SystemManager: winget & brew = 1 route, 2 incarnations.
// Commands = String (the pty/shell_probe runs them). PURE, testable parsers.
use std::collections::HashMap;

use crate::platform::Os;

#[derive(Debug, Clone, PartialEq)]
pub struct Outdated {
    pub current: String,
    pub available: String,
    /// True when this entry came from the `casks` bucket of `brew outdated
    /// --json=v2`. Casks self-update behind brew's receipt, so their version
    /// truth comes from `detect:`, not this scan (see decision in server.rs).
    pub is_cask: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IdField {
    Winget,
    Brew,
}

pub struct SystemManager {
    pub route: &'static str,
    pub os: &'static [Os],
    pub id_field: IdField,
}

impl SystemManager {
    pub fn install(&self, id: &str) -> String {
        match self.route {
            "winget" => format!("winget install --id {id} -e --source winget --accept-source-agreements --accept-package-agreements"),
            _ => format!("brew install --yes {id}"),
        }
    }
    pub fn install_pinned(&self, id: &str, version: &str) -> String {
        match self.route {
            "winget" => format!("winget install --id {id} -e --version {version} --source winget --accept-source-agreements --accept-package-agreements"),
            _ => format!("brew install --yes {id}@{version}"),
        }
    }
    pub fn uninstall(&self, id: &str) -> String {
        match self.route {
            // --all-versions: with several versions of the same id installed, a
            // bare uninstall is AMBIGUOUS — winget lists the matches and waits for
            // a narrowing filter, which deadlocks a display-only row (memory
            // display-only-terminal). Field-hit on Windows 2026-07-26. Removing
            // every version is also what "uninstall" MEANS here: the desired state
            // is absent, and detect (not a journal) is the truth afterwards — a
            // partial removal would still probe as present, reading as a failure.
            // --disable-interactivity: any residual ambiguity must fail loudly
            // rather than hang.
            "winget" => format!(
                "winget uninstall --id {id} -e --all-versions --source winget --accept-source-agreements --disable-interactivity"
            ),
            _ => format!("brew uninstall {id}"),
        }
    }
    pub fn upgrade(&self, id: &str, is_cask: bool) -> String {
        match self.route {
            "winget" => format!("winget upgrade --id {id} -e --source winget --accept-source-agreements --accept-package-agreements"),
            // Casks self-update behind brew's receipt → --force overwrites the
            // drift (fixes "already an App" exit 1). --yes stays non-interactive
            // (memory display-only-terminal). Formulae: plain --yes upgrade.
            _ if is_cask => format!("brew upgrade --cask --force --yes {id}"),
            _ => format!("brew upgrade --yes {id}"),
        }
    }
    pub fn presence_command(&self, id: &str) -> String {
        match self.route {
            "winget" => {
                format!("winget list --id {id} --exact --source winget --accept-source-agreements")
            }
            _ => format!("brew list --versions {id} || brew list --cask --versions {id}"),
        }
    }
    pub fn outdated_scan_command(&self) -> String {
        match self.route {
            "winget" => "winget upgrade --accept-source-agreements --source winget".into(),
            // --greedy-auto-updates: without it, brew HIDES `auto_updates: true`
            // casks (VS Code, Chrome…) from `outdated` by design, so Talos never
            // saw them as upgrade candidates and left them silently unmanaged.
            // In a controlled universe the version is guaranteed → we surface them
            // and force the upgrade (the cask command already carries --force).
            _ => "brew outdated --greedy-auto-updates --json=v2".into(),
        }
    }
    /// Extracts the version from presence_command's output.
    pub fn parse_version(&self, id: &str, output: &str) -> String {
        match self.route {
            "winget" => parse_winget_version(id, output),
            _ => parse_brew_version(id, output),
        }
    }
    pub fn parse_outdated(&self, output: &str) -> HashMap<String, Outdated> {
        match self.route {
            "winget" => parse_winget_upgrade(output),
            _ => parse_brew_outdated(output),
        }
    }
    /// Did this output have the SHAPE the parser expects, regardless of how many
    /// entries it yielded?
    ///
    /// Without this, an empty parse is ambiguous: "nothing is outdated" and "this
    /// output is a shape I do not know" look identical, and Talos would claim the
    /// machine is current on the strength of text it never understood. A scan can
    /// legitimately be recognised AND empty — that is the whole point.
    pub fn is_recognised(&self, output: &str) -> bool {
        match self.route {
            // The fixed-width table is identified by its header. winget also has
            // a legitimate no-table answer, which it states in prose.
            "winget" => {
                let clean = strip_ansi(output);
                let has_header = clean
                    .lines()
                    .any(|l| l.contains("Id") && l.contains("Available"));
                let says_nothing_to_do = clean.to_lowercase().contains("no installed package")
                    || clean.contains("No applicable update found")
                    || clean.contains("are up to date");
                has_header || says_nothing_to_do
            }
            // `--json=v2` is recognised when it parses as JSON carrying the two
            // buckets. An empty `{"formulae":[],"casks":[]}` is a real answer.
            _ => serde_json::from_str::<serde_json::Value>(output)
                .ok()
                .is_some_and(|v| v.get("formulae").is_some() || v.get("casks").is_some()),
        }
    }
}

pub const WINGET: SystemManager = SystemManager {
    route: "winget",
    os: &[Os::Windows],
    id_field: IdField::Winget,
};
pub const BREW: SystemManager = SystemManager {
    route: "brew",
    os: &[Os::Darwin, Os::Linux],
    id_field: IdField::Brew,
};

pub fn managers() -> Vec<&'static SystemManager> {
    vec![&WINGET, &BREW]
}

/// The native manager for this OS, or None.
pub fn native_manager(os: Os) -> Option<&'static SystemManager> {
    managers().into_iter().find(|m| m.os.contains(&os))
}

fn strip_ansi(s: &str) -> String {
    regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
        .unwrap()
        .replace_all(s, "")
        .into_owned()
}

// winget: "name  Id  Version" — the token after the id (must start with a digit).
fn parse_winget_version(id: &str, output: &str) -> String {
    let clean = strip_ansi(output);
    let lc = id.to_lowercase();
    for line in clean.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if let Some(at) = cols.iter().position(|c| c.to_lowercase() == lc) {
            if let Some(next) = cols.get(at + 1) {
                if next.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return next.to_string();
                }
            }
        }
    }
    String::new()
}

// brew: "git 2.50.1" — token after the id, must start with a digit (rejects "version").
fn parse_brew_version(id: &str, output: &str) -> String {
    let lc = id.to_lowercase();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first().is_some_and(|c| c.to_lowercase() == lc)
            && cols
                .get(1)
                .is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        {
            return cols[1].to_string();
        }
    }
    String::new()
}

// `winget upgrade`: FIXED-WIDTH table — slice by HEADER offsets (never split on
// spaces: names/versions contain spaces). Any hiccup → empty map.
pub fn parse_winget_upgrade(raw: &str) -> HashMap<String, Outdated> {
    let mut map = HashMap::new();
    let cleaned = strip_ansi(raw);
    let lines: Vec<String> = cleaned
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let Some(h) = lines
        .iter()
        .position(|l| l.contains("Id") && l.contains("Available"))
    else {
        return map;
    };
    let header = &lines[h];
    let (Some(id_pos), Some(ver_pos), Some(av_pos)) = (
        header.find("Id"),
        header.find("Version"),
        header.find("Available"),
    ) else {
        return map;
    };
    let src_pos = header.find("Source");
    for line in &lines[h + 1..] {
        if line.trim().is_empty() {
            break;
        }
        // separator line (dashes/spaces only)
        if line.trim().chars().all(|c| c == '-' || c.is_whitespace()) {
            continue;
        }
        if line.len() < av_pos {
            continue;
        }
        let slice = |a: usize, b: Option<usize>| -> String {
            let end = b.filter(|&e| e > a).unwrap_or(line.len()).min(line.len());
            line.get(a..end).unwrap_or("").trim().to_string()
        };
        let id = slice(id_pos, Some(ver_pos));
        let current = slice(ver_pos, Some(av_pos));
        let available = slice(av_pos, src_pos.filter(|&s| s > av_pos));
        if id.is_empty() || available.is_empty() {
            continue;
        }
        map.insert(
            id.to_lowercase(),
            Outdated {
                current,
                available,
                is_cask: false,
            },
        );
    }
    map
}

// `brew outdated --json=v2`: formulae + casks.
fn parse_brew_outdated(output: &str) -> HashMap<String, Outdated> {
    let mut map = HashMap::new();
    let Ok(json) = serde_json::from_str::<serde_json::Value>(output) else {
        return map;
    };
    for key in ["formulae", "casks"] {
        let is_cask = key == "casks";
        if let Some(arr) = json.get(key).and_then(|v| v.as_array()) {
            for item in arr {
                let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let current = item
                    .get("installed_versions")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let available = item
                    .get("current_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !name.is_empty() && !available.is_empty() {
                    map.insert(
                        name.to_lowercase(),
                        Outdated {
                            current: current.into(),
                            available: available.into(),
                            is_cask,
                        },
                    );
                }
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_manager_by_os() {
        assert_eq!(native_manager(Os::Windows).unwrap().route, "winget");
        assert_eq!(native_manager(Os::Darwin).unwrap().route, "brew");
    }

    #[test]
    fn brew_install_yes() {
        assert_eq!(BREW.install("jq"), "brew install --yes jq");
    }

    #[test]
    fn winget_uninstall_removes_every_version_non_interactively() {
        // Regression (Windows, 2026-07-26): a bare `winget uninstall --id X -e`
        // is ambiguous when several versions are installed — winget lists the
        // matches and waits, deadlocking a display-only row.
        let cmd = WINGET.uninstall("OpenJS.NodeJS");
        assert!(
            cmd.contains("--all-versions"),
            "must remove every version: {cmd}"
        );
        assert!(
            cmd.contains("--disable-interactivity"),
            "residual ambiguity must fail, not hang: {cmd}"
        );
        assert!(
            cmd.contains("--accept-source-agreements"),
            "no agreement prompt: {cmd}"
        );
    }

    #[test]
    fn brew_upgrade_cask_is_forced() {
        // Formula: plain non-interactive upgrade.
        assert_eq!(BREW.upgrade("jq", false), "brew upgrade --yes jq");
        // Cask: --cask --force absorbs the receipt drift that self-updating
        // apps create (else brew's anti-clobber guard → exit 1).
        assert_eq!(
            BREW.upgrade("visual-studio-code", true),
            "brew upgrade --cask --force --yes visual-studio-code"
        );
    }

    #[test]
    fn brew_version_parse() {
        assert_eq!(parse_brew_version("git", "git 2.50.1"), "2.50.1");
        assert_eq!(parse_brew_version("git", "git version 2.50.1"), ""); // rejects "version"
    }

    #[test]
    fn brew_outdated_json() {
        let j = r#"{"formulae":[{"name":"jq","installed_versions":["1.7"],"current_version":"1.8"}],"casks":[{"name":"visual-studio-code","installed_versions":["1.127.0"],"current_version":"1.130.0"}]}"#;
        let m = parse_brew_outdated(j);
        assert_eq!(
            m.get("jq"),
            Some(&Outdated {
                current: "1.7".into(),
                available: "1.8".into(),
                is_cask: false,
            })
        );
        assert_eq!(
            m.get("visual-studio-code"),
            Some(&Outdated {
                current: "1.127.0".into(),
                available: "1.130.0".into(),
                is_cask: true,
            })
        );
    }

    #[test]
    fn winget_upgrade_fixed_width() {
        let raw = "Name    Id            Version  Available Source\n\
                   -----------------------------------------------\n\
                   Node.js OpenJS.NodeJS 20.1.0   21.0.0    winget\n";
        let m = parse_winget_upgrade(raw);
        assert_eq!(
            m.get("openjs.nodejs").map(|o| o.available.clone()),
            Some("21.0.0".into())
        );
    }

    /// The hand-aligned fixture above proves the offsets arithmetic; it does NOT
    /// prove the parser survives what a real machine prints. This one reads
    /// output CAPTURED on a Windows box (winget v1.29.280, 2026-08-09) and kept
    /// under tests/fixtures/managers/ so it can be replaced by a maintainer's
    /// own capture when a row comes out wrong.
    ///
    /// Two shapes in it that no synthetic fixture had: a Version cell prefixed
    /// with `> ` (winget marks a pinned/held entry that way) and an MSIX id
    /// carrying a backslash and no Source column at all.
    /// ⭐ The distinction the whole ScanFailure type exists for: an output that
    /// is UNDERSTOOD but lists nothing must not read the same as an output the
    /// parser never recognised. Without this, Talos claims a machine is current
    /// on the strength of text it could not read.
    #[test]
    fn recognised_separates_a_real_empty_answer_from_an_unreadable_one() {
        let w = &WINGET;
        // A header with no rows: read, and genuinely empty.
        assert!(w.is_recognised("Name  Id  Version  Available  Source\n----\n"));
        // winget's prose answer for "nothing to do".
        assert!(w.is_recognised("No installed package found matching input criteria."));
        // ⚠️ The field-hit case: the source failed, no table was printed. This is
        // NOT "up to date" and must not be reported as such.
        assert!(!w.is_recognised("Failed in attempting to update the source: winget"));
        assert!(!w.is_recognised(""));

        let b = &BREW;
        // Empty buckets are a real answer from `--json=v2`.
        assert!(b.is_recognised(r#"{"formulae":[],"casks":[]}"#));
        // Anything that is not that JSON shape was not understood.
        assert!(!b.is_recognised("Error: Another active Homebrew process"));
        assert!(!b.is_recognised("[]"));
    }

    #[test]
    fn winget_upgrade_parses_real_captured_output() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/managers/winget-list-real.txt");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {} unreadable: {e}", path.display()));
        let m = parse_winget_upgrade(&raw);

        // Only rows that actually offer an upgrade are entries: three here.
        assert_eq!(m.len(), 3, "upgradable rows in the capture: {m:?}");
        assert_eq!(
            m.get("openjs.nodejs")
                .map(|o| (o.current.as_str(), o.available.as_str())),
            Some(("20.1.0", "21.0.0"))
        );
        assert_eq!(
            m.get("git.git").map(|o| o.available.clone()),
            Some("2.52.0".into())
        );
        assert_eq!(
            m.get("nushell.nushell").map(|o| o.available.clone()),
            Some("0.114.0".into())
        );

        // A row with no Available cell must NOT become an upgrade candidate —
        // that would make Talos offer an upgrade to the version already there.
        assert!(
            !m.contains_key("agilebits.1password"),
            "a `> `-prefixed row with no Available must not be an upgrade"
        );
        assert!(
            !m.contains_key("microsoft.visualstudiocode"),
            "a current row must not be an upgrade"
        );
        // The MSIX id has no Source column; whatever the parser does with it, it
        // must not panic and must not invent an upgrade.
        assert!(
            !m.keys().any(|k| k.contains("aim-tams")),
            "an MSIX row without Available must not be an upgrade"
        );
    }
}
