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

/// Where a winget package is installed: for every user (`--scope machine`) or for one
/// (`--scope user`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InstallScope {
    User,
    Machine,
}

impl InstallScope {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallScope::User => "user",
            InstallScope::Machine => "machine",
        }
    }
}

/// The name winget shows for an installed id: the cells before the id on its line of
/// `winget list`. It is the uninstall entry's DisplayName — which is what the scope probe
/// matches on. A cell winget truncated (`…`) is kept as the prefix it certainly is.
pub fn winget_installed_name(id: &str, list_output: &str) -> Option<String> {
    let clean = strip_ansi(list_output);
    let lc = id.to_lowercase();
    clean.lines().find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let at = cols.iter().position(|c| c.to_lowercase() == lc)?;
        let name = cols[..at].join(" ");
        let name = name.trim_end_matches('…').trim().to_string();
        (!name.is_empty()).then_some(name)
    })
}

/// The PowerShell probe behind `scope_from_arp`: the user's profile, then every uninstall
/// entry (64-bit, 32-bit, per-user) whose DisplayName starts with `name`, one per line.
pub fn arp_probe_command(name: &str) -> String {
    let quoted = name.replace('\'', "''");
    format!(
        "$n = '{quoted}'; \"PROFILE`t$env:USERPROFILE\"; \
         foreach ($p in 'HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall', \
         'HKLM:\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall', \
         'HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall') {{ \
         Get-ChildItem $p -ErrorAction SilentlyContinue | ForEach-Object {{ \
         $v = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue; \
         if ($v.DisplayName -and $v.DisplayName.StartsWith($n)) {{ \
         \"ARP`t$($v.DisplayName)`t$($v.DisplayVersion)`t$($v.InstallLocation)\" }} }} }}"
    )
}

/// Which scope the installed copy of `name` at `version` lives in, read from the probe
/// above. MEASURED on Windows 11 with Nushell's MSI: the hive does NOT tell — a per-user
/// copy's entry sits under HKLM too, which is why winget calls both "machine" — but the
/// InstallLocation does: under the user's profile → user, anywhere else → machine.
///
/// Entries of that version are preferred; when none carries it (winget and the entry spell
/// a version differently), every entry of the name is used. They must AGREE: no location,
/// no entry, or two copies in two places → None, and the caller keeps winget's default.
pub fn scope_from_arp(output: &str, name: &str, version: &str) -> Option<InstallScope> {
    let norm = |p: &str| p.trim().trim_end_matches('\\').to_lowercase();
    let mut profile = None;
    let mut named: Vec<(&str, &str, &str)> = Vec::new(); // (name, version, location)
    for line in output.lines() {
        let cols: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
        match cols.as_slice() {
            ["PROFILE", p] => profile = Some(norm(p)),
            ["ARP", n, v, loc] if n.starts_with(name) => named.push((n, v, loc)),
            _ => {}
        }
    }
    let profile = profile.filter(|p| !p.is_empty())?;
    // The exact name first: a longer one that merely starts the same is another product.
    // Only when none is exact — winget truncated the cell — does the prefix stand in.
    let exact = named.iter().any(|(n, _, _)| *n == name);
    let entries: Vec<(&str, &str)> = named
        .iter()
        .filter(|(n, _, _)| !exact || *n == name)
        .map(|(_, v, loc)| (*v, *loc))
        .collect();
    let same: Vec<&(&str, &str)> = entries.iter().filter(|(v, _)| *v == version).collect();
    let chosen: Vec<&(&str, &str)> = if same.is_empty() {
        entries.iter().collect()
    } else {
        same
    };
    let mut scopes = chosen
        .iter()
        .filter(|(_, loc)| !loc.trim().is_empty())
        .map(|(_, loc)| {
            if norm(loc).starts_with(&profile) {
                InstallScope::User
            } else {
                InstallScope::Machine
            }
        });
    let first = scopes.next()?;
    scopes.all(|s| s == first).then_some(first)
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
    /// Down to a pin: an uninstall, then the pinned install. ⚠️ For brew the pin is the
    /// versioned formula `id@version`, which exists only if the tap ships it — so it is
    /// CHECKED first: without that, a missing formula failed AFTER the uninstall and left
    /// the package removed. winget pins with a flag on the same id, so its chain is plain.
    pub fn downgrade(&self, id: &str, version: &str) -> String {
        self.downgrade_in(id, version, None)
    }
    /// The same chain, reinstalling in a known scope. Only winget has one to give: the
    /// uninstall removes every copy (`--all-versions`), and a pinned install with no
    /// `--scope` lands in winget's default, which is not where a machine-wide copy was —
    /// see `scope_from_arp`. brew has no scope, so it ignores it.
    pub fn downgrade_in(&self, id: &str, version: &str, scope: Option<InstallScope>) -> String {
        let install = match (self.route, scope) {
            ("winget", Some(sc)) => {
                format!(
                    "{} --scope {}",
                    self.install_pinned(id, version),
                    sc.as_str()
                )
            }
            _ => self.install_pinned(id, version),
        };
        let chain = format!("{} && {install}", self.uninstall(id));
        match self.route {
            "winget" => chain,
            _ => format!("brew info --formula {id}@{version} >/dev/null && {chain}"),
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

/// A fixed-width table's lines as a terminal would SHOW them.
///
/// ⚠️ Run without a console, winget writes its progress spinner onto the header's own
/// line — `\r   - \r<blanks>\rName  Id  Version…` — so the line holds every frame the
/// screen overwrote. Only what follows the last `\r` was ever visible, and it is what the
/// columns are aligned to. Counted from the raw start, every header offset lands too far
/// right and each row slices into nothing: the scan read ZERO rows while `is_recognised`
/// still saw a header, which reported the machine current and its packages absent.
fn screen_lines(raw: &str) -> Vec<String> {
    strip_ansi(raw)
        .lines()
        .map(|l| {
            let l = l.trim_end_matches('\r');
            l.rsplit('\r').next().unwrap_or(l).to_string()
        })
        .collect()
}

/// Where `needle` starts on a screen line, in COLUMNS — characters, not bytes.
///
/// ⚠️ winget ends a truncated cell with `…`: one column on screen, three bytes in UTF-8.
/// A byte offset taken from the header drifts by two on every such row and slices the
/// next cell mid-word (or off a char boundary, which `str::get` answers with nothing).
fn column(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|b| line[..b].chars().count())
}

/// The trimmed cell between columns `a` and `b` (`None`, or a `b` not after `a`, means to
/// the end of the line).
fn cell(line: &str, a: usize, b: Option<usize>) -> String {
    let end = b.filter(|&e| e > a);
    line.chars()
        .skip(a)
        .take(end.map_or(usize::MAX, |e| e - a))
        .collect::<String>()
        .trim()
        .to_string()
}

// winget: "name  Id  Version" — the token after the id (must start with a digit).
// ⚠️ When MULTIPLE VERSIONS are installed for the same id (field-hit: two Nushell
// versions on Windows, pin between them → wrong action direction), returns the
// MAXIMUM by `compare_versions`, because the newest is what PATH resolves and what
// a pin must be compared against. `winget list` prints one line per version.
pub fn parse_winget_version(id: &str, output: &str) -> String {
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
    let lines = screen_lines(raw);
    let Some(h) = lines
        .iter()
        .position(|l| l.contains("Id") && l.contains("Available"))
    else {
        return map;
    };
    let header = &lines[h];
    let (Some(id_pos), Some(ver_pos), Some(av_pos)) = (
        column(header, "Id"),
        column(header, "Version"),
        column(header, "Available"),
    ) else {
        return map;
    };
    let src_pos = column(header, "Source");
    for line in &lines[h + 1..] {
        if line.trim().is_empty() {
            break;
        }
        // separator line (dashes/spaces only)
        if line.trim().chars().all(|c| c == '-' || c.is_whitespace()) {
            continue;
        }
        if line.chars().count() < av_pos {
            continue;
        }
        let id = cell(line, id_pos, Some(ver_pos));
        let current = cell(line, ver_pos, Some(av_pos));
        let available = cell(line, av_pos, src_pos.filter(|&s| s > av_pos));
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
    let lines = screen_lines(raw);
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
    let (Some(id_pos), Some(ver_pos)) = (column(header, "Id"), column(header, "Version")) else {
        return Err(());
    };
    // The column after Version bounds the version cell: Available if present, else
    // Source, else the end of the line.
    let ver_end = column(header, "Available")
        .or_else(|| column(header, "Source"))
        .filter(|&e| e > ver_pos);

    // Field-reported defect (beta.32): two installed versions of the same id
    // appeared on separate rows → the last one silently overwrote the first.
    // The fix: collect ALL versions per id, then keep the MAXIMUM by
    // `compare_versions` (same reasoning as parse_brew_list and the parsers
    // fixed in beta.31). ⚠️ This is the symmetric twin of parse_brew_list that
    // was missed in beta.31: bulk scan calls THIS parser, not the per-package one.
    let mut versions_by_id: HashMap<String, Vec<String>> = HashMap::new();
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
        if line.chars().count() < ver_pos {
            continue;
        }
        let id = cell(line, id_pos, Some(ver_pos));
        // ⚠️ winget marks a held/pinned entry with a leading "> ". The marker is not
        // part of the version and must not reach the UI.
        let version = cell(line, ver_pos, ver_end)
            .trim_start_matches('>')
            .trim()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let key = id.to_lowercase();
        versions_by_id.entry(key).or_default().push(version);
    }
    // Now pick the MAXIMUM version for each id.
    let mut map = HashMap::new();
    for (id, mut versions) in versions_by_id {
        // Filter out empty versions, sort by compare_versions, take the first (max).
        versions.retain(|v| !v.is_empty());
        if versions.is_empty() {
            continue;
        }
        versions.sort_by(|a, b| crate::decision::compare_versions(b, a).cmp(&0));
        if let Some(max) = versions.first() {
            map.insert(id, max.clone());
        }
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
    fn a_brew_downgrade_checks_the_versioned_formula_before_removing_anything() {
        // A downgrade is `uninstall && install id@ver`. brew has no version flag, so the pin is
        // the versioned formula `id@ver`, which exists only if the tap ships it — and when it
        // does not, the uninstall had already run: the package was GONE. The check comes first,
        // so a missing formula fails the step with the package still in place.
        let brew = native_manager(Os::Darwin).unwrap();
        let cmd = brew.downgrade("nushell", "0.113.1");
        let check = cmd
            .find("brew info --formula nushell@0.113.1")
            .expect("the formula is checked");
        let removes = cmd
            .find("brew uninstall nushell")
            .expect("then it uninstalls");
        assert!(check < removes, "{cmd}");
        assert!(cmd.ends_with("brew install --yes nushell@0.113.1"), "{cmd}");
        // winget pins with a flag on the same id; its chain is the plain one.
        let winget = native_manager(Os::Windows).unwrap();
        assert!(winget
            .downgrade("Git.Git", "2.50.1")
            .starts_with("winget uninstall"));
    }

    // ---- a winget downgrade reinstalls where the package WAS ---------------------------
    //
    // MEASURED in the Windows VM (2026-10-21, Nushell's MSI): with no `--scope`, the pinned
    // install after the uninstall landed PER-USER (AppData\Local\Programs) although the
    // copy it replaced was machine-wide (Program Files); the next upgrade then installed a
    // second, machine-wide copy beside it. winget cannot say which scope a package is in —
    // `winget list --scope machine` listed BOTH copies, `--scope user` neither — and both
    // uninstall entries sit under HKLM. What separates them is the entry's InstallLocation.

    const ARP: &str = "PROFILE\tC:\\Users\\chris\n\
        ARP\tNushell\t0.116.1\tC:\\Program Files\\nu\n\
        ARP\tNushell\t0.113.1\tC:\\Users\\chris\\AppData\\Local\\Programs\\nu\n\
        ARP\tNushell Extras\t9.9\t\n";

    #[test]
    fn a_downgrade_reinstalls_in_the_scope_of_the_copy_it_replaces() {
        assert_eq!(
            scope_from_arp(ARP, "Nushell", "0.116.1"),
            Some(InstallScope::Machine)
        );
        assert_eq!(
            scope_from_arp(ARP, "Nushell", "0.113.1"),
            Some(InstallScope::User)
        );
        // The profile is compared without case: Windows paths are case-insensitive.
        let lower = ARP.replace("C:\\Users\\chris\\AppData", "c:\\users\\CHRIS\\appdata");
        assert_eq!(
            scope_from_arp(&lower, "Nushell", "0.113.1"),
            Some(InstallScope::User)
        );
    }

    #[test]
    fn a_scope_nobody_recorded_is_not_guessed() {
        // No location, no matching entry, or two copies of one version in two places: None,
        // and the chain stays the plain one — winget's own default, as before.
        assert_eq!(scope_from_arp(ARP, "Nushell Extras", "9.9"), None);
        assert_eq!(scope_from_arp(ARP, "Git", "2.50.1"), None);
        let both = format!("{ARP}ARP\tNushell\t0.116.1\tC:\\Users\\chris\\nu\n");
        assert_eq!(scope_from_arp(&both, "Nushell", "0.116.1"), None);
        // An unknown version falls back to every entry of the name, when they agree.
        let one = "PROFILE\tC:\\Users\\chris\nARP\tGit\t2.51.0\tC:\\Program Files\\Git\n";
        assert_eq!(
            scope_from_arp(one, "Git", "2.51.0.windows.1"),
            Some(InstallScope::Machine)
        );
        // A longer name that merely STARTS like this one is another product, never a copy.
        let other = "PROFILE\tC:\\Users\\chris\n\
            ARP\tGit\t2.51.0\tC:\\Program Files\\Git\n\
            ARP\tGit Extensions\t5.0\tC:\\Users\\chris\\GitExt\n";
        assert_eq!(
            scope_from_arp(other, "Git", "9"),
            Some(InstallScope::Machine)
        );
        // …unless winget truncated the name, and the prefix is all there is to match.
        assert_eq!(
            scope_from_arp(other, "Git Ext", "5.0"),
            Some(InstallScope::User)
        );
    }

    #[test]
    fn the_installed_name_is_what_winget_list_prints_before_the_id() {
        let out = "Name                Id               Version  Source\n\
                   --------------------------------------------------\n\
                   Nushell             Nushell.Nushell  0.116.1  winget\n\
                   Microsoft Visual Studio Code  Microsoft.VisualStudioCode  1.105.0  winget\n\
                   Very Long Product…  Vendor.Long      1.0      winget\n";
        assert_eq!(
            winget_installed_name("Nushell.Nushell", out).as_deref(),
            Some("Nushell")
        );
        assert_eq!(
            winget_installed_name("Microsoft.VisualStudioCode", out).as_deref(),
            Some("Microsoft Visual Studio Code")
        );
        // A truncated cell keeps only what is certain: a prefix.
        assert_eq!(
            winget_installed_name("Vendor.Long", out).as_deref(),
            Some("Very Long Product")
        );
        assert_eq!(winget_installed_name("Absent.Id", out), None);
    }

    #[test]
    fn a_scoped_downgrade_passes_the_scope_to_the_reinstall_only() {
        let winget = native_manager(Os::Windows).unwrap();
        let cmd = winget.downgrade_in("Nushell.Nushell", "0.113.1", Some(InstallScope::Machine));
        let (uninstall, install) = cmd.split_once(" && ").expect("a chain of two");
        assert!(
            !uninstall.contains("--scope"),
            "the uninstall removes every copy: {cmd}"
        );
        assert!(
            install.contains("--version 0.113.1") && install.ends_with(" --scope machine"),
            "{cmd}"
        );
        assert_eq!(
            winget.downgrade_in("X.Y", "1.0", None),
            winget.downgrade("X.Y", "1.0")
        );
        // The probe quotes the name for PowerShell: a quote inside it is doubled.
        assert!(arp_probe_command("Bob's Tool").contains("'Bob''s Tool'"));
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

    /// The bytes winget prints when Talos runs it: no console attached (`quiet_command`),
    /// stdout a pipe, read raw. Captured on a Windows 11 VM (winget v1.11.510,
    /// 2026-10-08) with exactly `shell_probe`'s wrapping.
    ///
    /// ⚠️ `winget-list-real.txt` above was captured from a console and came out clean, so
    /// every parser test passed while the live scan read NOTHING. Two shapes only the
    /// pipe shows:
    /// - the progress spinner is written onto the header's own line (`\r   - \r…\rName`),
    ///   so a header offset counted from the start of that line lands columns to the right;
    /// - a truncated cell ends in `…`, ONE column on screen but three bytes in UTF-8, so a
    ///   byte offset drifts by two on every row after it.
    fn no_console_capture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/managers")
            .join(name);
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("{} unreadable: {e}", path.display()));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn winget_list_reads_the_output_of_a_process_without_a_console() {
        let m = WINGET
            .parse_presence(&no_console_capture("winget-list-no-console.txt"))
            .expect("the header is recognised");
        assert_eq!(m.get("git.git").map(String::as_str), Some("2.55.0.3"));
        assert_eq!(
            m.get("microsoft.windowsterminal").map(String::as_str),
            Some("1.24.11911.0")
        );
        // The row after a truncated name: `…` is three bytes for one column.
        assert_eq!(
            m.get("microsoft.dotnet.native.runtime").map(String::as_str),
            Some("2.2.28604.0")
        );
        assert_eq!(
            m.get("microsoft.vclibs.desktop.14").map(String::as_str),
            Some("14.0.33728.0")
        );
        // The held marker is still stripped on this shape.
        assert_eq!(
            m.get("redhat.virtio").map(String::as_str),
            Some("0.1.285-1")
        );
    }

    #[test]
    fn winget_upgrade_reads_the_output_of_a_process_without_a_console() {
        let raw = no_console_capture("winget-upgrade-no-console.txt");
        assert!(WINGET.is_recognised(&raw));
        let m = parse_winget_upgrade(&raw);
        assert_eq!(m.len(), 5, "upgradable rows in the capture: {m:?}");
        assert_eq!(
            m.get("git.git")
                .map(|o| (o.current.as_str(), o.available.as_str())),
            Some(("2.55.0.3", "2.55.0.5"))
        );
        assert_eq!(
            m.get("microsoft.windowsterminal")
                .map(|o| o.available.as_str()),
            Some("1.25.2733.0")
        );
    }

    #[test]
    fn winget_list_returns_maximum_when_multiple_versions_installed() {
        // Field-reported defect (beta.31 fixed parse_winget_version but missed
        // parse_winget_list, which is the twin that matters — bulk scan calls it).
        // Two versions of the same id on separate ROWS → parser must return the MAX,
        // because the newest is what PATH resolves to.
        let output = "\
Name                                                         Id                                                                                    Version                    Available           Source
---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
Nushell                                                      Nushell.Nushell                                                                       0.111.1                                        winget
Nushell                                                      Nushell.Nushell                                                                       0.114.0                                        winget
";
        let m = parse_winget_list(output).expect("recognised");
        assert_eq!(
            m.get("nushell.nushell").map(String::as_str),
            Some("0.114.0")
        );

        // Reversed: higher version FIRST.
        let output_rev = "\
Name                                                         Id                                                                                    Version                    Available           Source
---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
Nushell                                                      Nushell.Nushell                                                                       0.114.0                                        winget
Nushell                                                      Nushell.Nushell                                                                       0.111.1                                        winget
";
        let m = parse_winget_list(output_rev).expect("recognised");
        assert_eq!(
            m.get("nushell.nushell").map(String::as_str),
            Some("0.114.0"),
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
