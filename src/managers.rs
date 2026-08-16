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
    /// ONE listing for the whole machine, the presence counterpart of
    /// `outdated_scan_command`. brew needs a second call for casks — see
    /// `presence_scan_command_cask`.
    pub fn presence_scan_command(&self) -> String {
        match self.route {
            "winget" => "winget list --accept-source-agreements".into(),
            _ => "brew list --versions".into(),
        }
    }
    /// brew reports formulae and casks separately, so a full picture needs both.
    /// `None` for a manager that lists everything at once.
    pub fn presence_scan_command_cask(&self) -> Option<String> {
        match self.route {
            "winget" => None,
            _ => Some("brew list --cask --versions".into()),
        }
    }
    /// Installed id (lowercased) → its version, for the WHOLE machine.
    ///
    /// `Err` when the output was not recognised. That distinction is the point: an
    /// empty map read as "nothing installed" would make Apply offer to install
    /// software that is already there — the mirror of the false-green defect fixed
    /// on the upgrade side.
    #[allow(clippy::result_unit_err)] // recognised-or-not; there is no second cause to name
    pub fn parse_presence(&self, output: &str) -> Result<HashMap<String, String>, ()> {
        match self.route {
            "winget" => parse_winget_list(output),
            _ => parse_brew_list(output),
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
// ⚠️ When MULTIPLE VERSIONS are installed for the same id (field-hit: two Nushell
// versions on Windows, pin between them → wrong action direction), returns the
// MAXIMUM by `compare_versions`, because the newest is what PATH resolves and what
// a pin must be compared against. `winget list` prints one line per version.
fn parse_winget_version(id: &str, output: &str) -> String {
    let clean = strip_ansi(output);
    let lc = id.to_lowercase();
    let mut versions: Vec<String> = Vec::new();
    for line in clean.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if let Some(at) = cols.iter().position(|c| c.to_lowercase() == lc) {
            if let Some(next) = cols.get(at + 1) {
                // ⚠️ winget marks a held/pinned entry with `> ` in its own column before
                // the version. Strip it if present (the parse_winget_list test exercises
                // that case; this parser shares the same shape).
                let v = next.trim_start_matches('>').trim();
                if v.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    versions.push(v.to_string());
                }
            }
        }
    }
    // Return the MAXIMUM by `compare_versions`. This relies on numeric comparison
    // (2.10 > 2.9) segment by segment, which works for dot-version schemes and brew
    // dates (2026-07-16), but a brew revision suffix (`_1`) or a tap suffix are
    // compared only by accident. That limit is KNOWN and named here rather than
    // hidden: a proper version-comparison overhaul is separate work.
    versions.sort_by(|a, b| crate::decision::compare_versions(b, a).cmp(&0));
    versions.first().cloned().unwrap_or_default()
}

// brew: "git 2.50.1" or "git 2.50.1 2.51.0" — multiple versions in later columns on
// the SAME line. Returns the MAXIMUM by `compare_versions` for the same reason as
// the winget parser: the newest is what PATH resolves and what a pin compares against.
fn parse_brew_version(id: &str, output: &str) -> String {
    let lc = id.to_lowercase();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first().is_some_and(|c| c.to_lowercase() == lc) {
            // ⚠️ The line must match the expected shape: "name version..." where the
            // SECOND token starts with a digit. Reject "git version 2.50.1" (the word
            // "version" is not a version token). This preserves the original behavior.
            if !cols
                .get(1)
                .is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
            {
                continue;
            }
            // Collect ALL version tokens (cols[1..]), keeping only those that start with a digit.
            let mut versions: Vec<String> = cols
                .iter()
                .skip(1)
                .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .map(|v| v.to_string())
                .collect();
            if versions.is_empty() {
                continue;
            }
            // Return the MAXIMUM. Sorting relies on `compare_versions`, which handles
            // numeric segments and brew dates, but brew revisions (`_1`) and tap suffixes
            // are compared only by accident — a known limit, named here.
            versions.sort_by(|a, b| crate::decision::compare_versions(b, a).cmp(&0));
            return versions.first().cloned().unwrap_or_default();
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

// `winget list`: the same FIXED-WIDTH table as `winget upgrade`, minus the meaning
// of the rows — EVERY row here is an installed package, upgradable or not. Sliced
// by header offsets, never by spaces (names and versions contain them).
fn parse_winget_list(raw: &str) -> Result<HashMap<String, String>, ()> {
    let cleaned = strip_ansi(raw);
    let lines: Vec<String> = cleaned
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    // "Available" is optional here: `winget list` prints the column, but a machine
    // with nothing to upgrade leaves every cell empty, so recognition keys on
    // Id + Version — the two columns that always carry meaning.
    let Some(h) = lines
        .iter()
        .position(|l| l.contains("Id") && l.contains("Version"))
    else {
        return Err(());
    };
    let header = &lines[h];
    let (Some(id_pos), Some(ver_pos)) = (header.find("Id"), header.find("Version")) else {
        return Err(());
    };
    // The column after Version bounds the version cell: Available if present, else
    // Source, else the end of the line.
    let ver_end = header
        .find("Available")
        .or_else(|| header.find("Source"))
        .filter(|&e| e > ver_pos);

    let mut map = HashMap::new();
    for line in &lines[h + 1..] {
        // ⚠️ `continue`, not `break` as the upgrade parser does: there every row
        // after a blank is noise, here a short MSIX row must not end the table.
        if line.trim().is_empty() {
            continue;
        }
        // separator line (dashes/spaces only)
        if line.trim().chars().all(|c| c == '-' || c.is_whitespace()) {
            continue;
        }
        if line.len() < ver_pos {
            continue;
        }
        let slice = |a: usize, b: Option<usize>| -> String {
            let end = b.filter(|&e| e > a).unwrap_or(line.len()).min(line.len());
            line.get(a..end).unwrap_or("").trim().to_string()
        };
        let id = slice(id_pos, Some(ver_pos));
        // ⚠️ winget marks a held/pinned entry with a leading "> ". The marker is not
        // part of the version and must not reach the UI.
        let version = slice(ver_pos, ver_end)
            .trim_start_matches('>')
            .trim()
            .to_string();
        if id.is_empty() {
            continue;
        }
        map.insert(id.to_lowercase(), version);
    }
    Ok(map)
}

// `brew list --versions`: one package per line, "name v1 v2…". Multiple versions in
// later columns. Returns the MAXIMUM for each package. Recognition is structural —
// at least one line shaped that way — because brew prints no header to key on.
fn parse_brew_list(raw: &str) -> Result<HashMap<String, String>, ()> {
    let mut map = HashMap::new();
    for line in raw.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = cols.first() else {
            continue;
        };
        // The line must match the expected shape: the SECOND token must start with a
        // digit. This rejects warnings and prompts that aren't "name version..." lines.
        if !cols
            .get(1)
            .is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        {
            continue;
        }
        // Collect ALL version tokens (cols[1..]), keeping only those that start with a digit.
        let mut versions: Vec<String> = cols
            .iter()
            .skip(1)
            .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(|v| v.to_string())
            .collect();
        if versions.is_empty() {
            continue; // not a "name version" line (a warning, a prompt…)
        }
        // Return the MAXIMUM. Same sorting as `parse_brew_version`, same limit on
        // revisions and tap suffixes.
        versions.sort_by(|a, b| crate::decision::compare_versions(b, a).cmp(&0));
        if let Some(max) = versions.first() {
            map.insert(name.to_lowercase(), max.clone());
        }
    }
    if map.is_empty() {
        Err(()) // nothing recognised: cannot claim the machine is empty
    } else {
        Ok(map)
    }
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
    use crate::decision::{action_for, Action, Desired, MachineFacts};

    #[test]
    fn presence_scan_reads_the_captured_windows_listing() {
        // The SAME capture the upgrade parser is tested against: a real machine's
        // output, not a hand-aligned fixture. It carries the two shapes no synthetic
        // one had — a `> `-prefixed version and an MSIX id with a backslash.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/managers/winget-list-real.txt");
        let raw = std::fs::read_to_string(&path).unwrap();
        let m = WINGET
            .parse_presence(&raw)
            .expect("the header is recognised");

        // Every id in the table is a PRESENT package, upgradable or not — that is the
        // difference from parse_winget_upgrade, which keeps only upgradable rows.
        assert_eq!(m.get("git.git").map(String::as_str), Some("2.51.0"));
        assert_eq!(m.get("openjs.nodejs").map(String::as_str), Some("20.1.0"));
        assert_eq!(
            m.get("microsoft.visualstudiocode").map(String::as_str),
            Some("1.130.0"),
            "a current package must still be PRESENT"
        );
        // ⚠️ The `> ` prefix marks a held entry. The version must be usable, not
        // carry the marker into the UI.
        assert_eq!(
            m.get("agilebits.1password").map(String::as_str),
            Some("8.12.30.21"),
            "the `> ` prefix must be stripped"
        );
        // An MSIX id has no Source column; it must still be found, not crash.
        assert!(
            m.keys().any(|k| k.contains("aim-tams")),
            "an MSIX row is still an installed package: {:?}",
            m.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn presence_scan_reads_brew_listings() {
        // Two listings merge into one map: `brew list --versions` for formulae and
        // the `--cask` call, because brew reports them separately.
        let formulae = "git 2.51.0\nnode 26.5.1 26.4.0\njq 1.8.2\n";
        let casks = "visual-studio-code 1.130.0\n";
        let m = BREW
            .parse_presence(&format!("{formulae}{casks}"))
            .expect("plain lines are recognised");
        assert_eq!(m.get("git").map(String::as_str), Some("2.51.0"));
        assert_eq!(m.get("jq").map(String::as_str), Some("1.8.2"));
        assert_eq!(
            m.get("visual-studio-code").map(String::as_str),
            Some("1.130.0")
        );
        // Several versions installed → the FIRST is what brew reports as current.
        assert_eq!(m.get("node").map(String::as_str), Some("26.5.1"));
    }

    #[test]
    fn an_unreadable_listing_is_an_error_not_an_empty_machine() {
        // ⭐ The trap. An empty map from a failed command is indistinguishable from a
        // machine with nothing installed — and reading it as "nothing installed"
        // would make Apply offer to install everything that is already there.
        assert!(WINGET
            .parse_presence("Failed in attempting to update the source: winget")
            .is_err());
        assert!(WINGET.parse_presence("").is_err());
        assert!(BREW
            .parse_presence("Error: Another active Homebrew process")
            .is_err());
        // A recognised-but-empty listing is a REAL answer and must be Ok.
        assert_eq!(
            WINGET
                .parse_presence("Name  Id  Version  Available  Source\n----\n")
                .map(|m| m.len()),
            Ok(0)
        );
        // ⚠️ brew's empty output is genuinely ambiguous — no header to recognise —
        // so unlike winget it CANNOT report a recognised-but-empty machine.
        assert!(BREW.parse_presence("").is_err());
    }

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

    #[test]
    fn winget_version_returns_maximum_when_multiple_versions_installed() {
        // ⚠️ Field-reported defect (Windows, two Nushell versions: 0.111.x and
        // 0.114.x, catalogue pins 0.113.1): the parser returned whichever version
        // came FIRST in `winget list` output, so when 0.111 was first, Talos
        // compared the pin against the WRONG number and reported an upgrade when
        // a downgrade was needed. The fix: return the MAXIMUM of all versions
        // found for the id, because the newest is what PATH resolves and what
        // the pin must be compared against.
        let output = "\
Name      Id               Version   Available  Source
-------------------------------------------------------
Nushell   Nushell.Nushell  0.111.1              winget
Nushell   Nushell.Nushell  0.114.0              winget
";
        // The higher version is SECOND — must still return the higher one.
        assert_eq!(parse_winget_version("Nushell.Nushell", output), "0.114.0");

        // Reversed: higher version FIRST.
        let output_rev = "\
Name      Id               Version   Available  Source
-------------------------------------------------------
Nushell   Nushell.Nushell  0.114.0              winget
Nushell   Nushell.Nushell  0.111.1              winget
";
        assert_eq!(
            parse_winget_version("Nushell.Nushell", output_rev),
            "0.114.0",
            "must return MAX regardless of order"
        );
    }

    #[test]
    fn brew_version_returns_maximum_when_multiple_versions_installed() {
        // `brew list --versions` prints extra versions in later columns on the
        // SAME line: "nushell 0.111.1 0.114.0". The parser must return the MAX.
        assert_eq!(
            parse_brew_version("nushell", "nushell 0.111.1 0.114.0"),
            "0.114.0"
        );
        // Reversed order.
        assert_eq!(
            parse_brew_version("nushell", "nushell 0.114.0 0.111.1"),
            "0.114.0",
            "must return MAX regardless of order"
        );
    }

    #[test]
    fn brew_list_returns_maximum_when_multiple_versions_installed() {
        // Machine-wide scan: same as the per-package probe, must return MAX.
        let output = "git 2.50.1\nnushell 0.111.1 0.114.0\njq 1.8.2\n";
        let m = parse_brew_list(output).expect("recognised");
        assert_eq!(m.get("nushell").map(String::as_str), Some("0.114.0"));

        // Reversed.
        let output_rev = "git 2.50.1\nnushell 0.114.0 0.111.1\njq 1.8.2\n";
        let m = parse_brew_list(output_rev).expect("recognised");
        assert_eq!(
            m.get("nushell").map(String::as_str),
            Some("0.114.0"),
            "must return MAX regardless of order"
        );
    }

    #[test]
    fn the_operators_case_two_versions_pin_between_them() {
        // ⚠️ THE DEFECT AS REPORTED: two versions installed, an exact pin
        // BETWEEN them. The parser returned whichever version came first,
        // so the action was wrong. Field-hit: Windows, Nushell 0.111.x and
        // 0.114.x, pin 0.113.1 → should be a DOWNGRADE, was reported as upgrade.
        let f = MachineFacts {
            present: true,
            outdated: false,
            can_uninstall: false,
            pin: Some("0.113.1"),
            // ⭐ Installed version is the MAXIMUM of what the parser found.
            // Before the fix, this would have been 0.111.x (the first line),
            // producing Action::Upgrade. After: 0.114.0 → Downgrade.
            installed_version: "0.114.0",
        };
        assert_eq!(
            action_for(Desired::Present, &f),
            Some(Action::Downgrade),
            "pin 0.113.1 against installed 0.114.0 is a downgrade"
        );
    }
}
