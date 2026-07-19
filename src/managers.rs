// Port of managers.ts — SystemManager: winget & brew = 1 route, 2 incarnations.
// Commands = String (the pty/shell_probe runs them). PURE, testable parsers.
use std::collections::HashMap;

use crate::platform::Os;

#[derive(Debug, Clone, PartialEq)]
pub struct Outdated {
    pub current: String,
    pub available: String,
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
            "winget" => format!("winget uninstall --id {id} -e --source winget"),
            _ => format!("brew uninstall {id}"),
        }
    }
    pub fn upgrade(&self, id: &str) -> String {
        match self.route {
            "winget" => format!("winget upgrade --id {id} -e --source winget --accept-source-agreements --accept-package-agreements"),
            _ => format!("brew upgrade --yes {id}"),
        }
    }
    pub fn presence_command(&self, id: &str) -> String {
        match self.route {
            "winget" => format!("winget list --id {id} --exact --source winget --accept-source-agreements"),
            _ => format!("brew list --versions {id} || brew list --cask --versions {id}"),
        }
    }
    pub fn outdated_scan_command(&self) -> String {
        match self.route {
            "winget" => "winget upgrade --accept-source-agreements --source winget".into(),
            _ => "brew outdated --json=v2".into(),
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
        map.insert(id.to_lowercase(), Outdated { current, available });
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
    fn brew_version_parse() {
        assert_eq!(parse_brew_version("git", "git 2.50.1"), "2.50.1");
        assert_eq!(parse_brew_version("git", "git version 2.50.1"), ""); // rejects "version"
    }

    #[test]
    fn brew_outdated_json() {
        let j = r#"{"formulae":[{"name":"jq","installed_versions":["1.7"],"current_version":"1.8"}],"casks":[]}"#;
        let m = parse_brew_outdated(j);
        assert_eq!(
            m.get("jq"),
            Some(&Outdated {
                current: "1.7".into(),
                available: "1.8".into()
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
}
