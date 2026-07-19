// Port de bundles.ts — scan bundles/ → { bundles, steps }. Pur (serde_yaml), pas
// de pty/réseau. Le route table : un paquet = un NEED satisfait par une route nommée.
use serde::Deserialize;

use crate::managers::{managers, native_manager, IdField};
use crate::platform::Os;

#[derive(Debug, Clone, PartialEq)]
pub enum Posture {
    Mandatory,
    OptOut,
    OptIn,
    Forbidden,
}

impl Posture {
    fn parse(s: Option<&str>) -> Posture {
        match s {
            Some("opt-out") => Posture::OptOut,
            Some("opt-in") => Posture::OptIn,
            Some("forbidden") => Posture::Forbidden,
            _ => Posture::Mandatory,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Posture::Mandatory => "mandatory",
            Posture::OptOut => "opt-out",
            Posture::OptIn => "opt-in",
            Posture::Forbidden => "forbidden",
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct RawPkg {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    winget: Option<String>,
    #[serde(default)]
    brew: Option<String>,
    #[serde(default)]
    cargo: Option<String>,
    #[serde(default)]
    npm: Option<String>,
    #[serde(default, rename = "npmFlags")]
    npm_flags: Option<String>,
    #[serde(default)]
    run: Option<String>,
    #[serde(default, rename = "runUninstall")]
    run_uninstall: Option<String>,
    #[serde(default, rename = "claude-plugin")]
    claude_plugin: Option<String>,
    #[serde(default)]
    marketplace: Option<String>,
    #[serde(default)]
    skill: Option<String>,
    #[serde(default, rename = "skillName")]
    skill_name: Option<String>,
    #[serde(default)]
    detect: Option<String>,
    #[serde(default)]
    check: Option<String>,
    #[serde(default, rename = "version-regex")]
    version_regex: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawBundle {
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    selectable: Option<bool>,
    #[serde(default)]
    posture: Option<String>,
    #[serde(default)]
    packages: Vec<RawPkg>,
}

#[derive(Debug, Clone)]
pub struct BundleMeta {
    pub name: String,
    pub emoji: String,
    pub description: String,
    pub priority: i64,
    pub selectable: bool,
    pub posture: Posture,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub bundle: String,
    pub name: String,
    pub description: String,
    #[allow(dead_code)]
    pub install: Option<String>,
    pub uninstall: Option<String>,
    #[allow(dead_code)]
    pub upgrade: Option<String>,
    #[allow(dead_code)]
    pub downgrade: Option<String>,
    pub route: Option<String>,
    pub system_id: Option<String>,
    pub detect: Option<String>,
    pub check: Option<String>,
    pub is_config: bool,
    pub version_regex: Option<String>,
    pub pin: Option<String>,
    #[allow(dead_code)]
    pub requires: Vec<String>,
    pub posture: Posture,
}

pub struct Commands {
    pub route: Option<String>,
    pub install: Option<String>,
    pub uninstall: Option<String>,
    pub upgrade: Option<String>,
    pub downgrade: Option<String>,
}

/// Le NAMED ROUTE TABLE — port de commandsFor. Family 1 (system manager, arbitré par
/// OS) d'abord, puis cargo/npm/run/claude-plugin/skill.
fn commands_for(pkg: &RawPkg, os: Os) -> Commands {
    let ver = pkg.version.as_deref().unwrap_or("").trim().to_string();
    let none = Commands {
        route: None,
        install: None,
        uninstall: None,
        upgrade: None,
        downgrade: None,
    };

    if let Some(mgr) = native_manager(os) {
        let id = match mgr.id_field {
            IdField::Winget => &pkg.winget,
            IdField::Brew => &pkg.brew,
        };
        if let Some(id) = id {
            if !ver.is_empty() {
                let inst = mgr.install_pinned(id, &ver);
                return Commands {
                    route: Some(mgr.route.into()),
                    install: Some(inst.clone()),
                    uninstall: Some(mgr.uninstall(id)),
                    upgrade: Some(inst.clone()),
                    downgrade: Some(format!("{} && {}", mgr.uninstall(id), inst)),
                };
            }
            return Commands {
                route: Some(mgr.route.into()),
                install: Some(mgr.install(id)),
                uninstall: Some(mgr.uninstall(id)),
                upgrade: Some(mgr.upgrade(id)),
                downgrade: None,
            };
        }
    }
    if let Some(c) = &pkg.cargo {
        let inst = if ver.is_empty() {
            format!("cargo install {c}")
        } else {
            format!("cargo install {c} --version {ver}")
        };
        return Commands {
            route: Some("cargo".into()),
            install: Some(inst.clone()),
            uninstall: Some(format!("cargo uninstall {c}")),
            upgrade: Some(inst.clone()),
            downgrade: if ver.is_empty() {
                None
            } else {
                Some(format!("cargo uninstall {c} && {inst}"))
            },
        };
    }
    if let Some(n) = &pkg.npm {
        let flags = pkg
            .npm_flags
            .as_deref()
            .map(|f| format!("{f} "))
            .unwrap_or_default();
        let inst_target = if ver.is_empty() {
            n.clone()
        } else {
            format!("{n}@{ver}")
        };
        let up_target = if ver.is_empty() {
            format!("{n}@latest")
        } else {
            format!("{n}@{ver}")
        };
        return Commands {
            route: Some("npm".into()),
            install: Some(format!("npm install -g {flags}{inst_target}")),
            uninstall: Some(format!("npm uninstall -g {n}")),
            upgrade: Some(format!("npm install -g {flags}{up_target}")),
            downgrade: if ver.is_empty() {
                None
            } else {
                Some(format!(
                    "npm uninstall -g {n} && npm install -g {flags}{inst_target}"
                ))
            },
        };
    }
    if let Some(r) = &pkg.run {
        return Commands {
            route: Some("run".into()),
            install: Some(r.clone()),
            uninstall: pkg.run_uninstall.clone(),
            upgrade: None,
            downgrade: None,
        };
    }
    if let Some(id) = &pkg.claude_plugin {
        let add = pkg
            .marketplace
            .as_deref()
            .map(|m| format!("claude plugin marketplace add \"{m}\" && "))
            .unwrap_or_default();
        return Commands {
            route: Some("claude-plugin".into()),
            install: Some(format!("{add}claude plugin install {id} --scope user")),
            uninstall: Some(format!("claude plugin uninstall {id}")),
            upgrade: Some(format!("claude plugin update {id}")),
            downgrade: None,
        };
    }
    if let Some(src) = &pkg.skill {
        let name = pkg.skill_name.clone().unwrap_or_else(|| pkg.name.clone());
        return Commands {
            route: Some("skill".into()),
            install: Some(format!("npx skills add {src} -g -y")),
            uninstall: Some(format!("npx skills remove {name} -y")),
            upgrade: Some(format!("npx skills update {name} -y")),
            downgrade: None,
        };
    }
    none
}

pub struct Plan {
    pub bundles: Vec<BundleMeta>,
    pub steps: Vec<Step>,
}

/// Scan bundles/ (chaque sous-dossier avec bundle.yaml). YAML cassé → skip loggé.
/// Dossier absent → plan VIDE (l'exe ouvre inerte, ne crashe pas).
pub fn load_bundles(root: &str, os: Os, log: &dyn Fn(&str)) -> Plan {
    let mut bundles = Vec::new();
    let mut steps = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        log(&format!("no bundles dir at {root} — opening inert"));
        return Plan { bundles, steps };
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let bundle_dir = format!("{root}/{dir_name}");
        let file = format!("{bundle_dir}/bundle.yaml");
        let Ok(raw) = std::fs::read_to_string(&file) else {
            continue;
        };
        let b: RawBundle = match serde_yaml::from_str(&raw) {
            Ok(b) => b,
            Err(e) => {
                log(&format!("bundle skipped (bad YAML): {dir_name} — {e}"));
                continue;
            }
        };
        let posture = Posture::parse(b.posture.as_deref());
        let meta = BundleMeta {
            name: b.bundle.clone().unwrap_or_else(|| dir_name.clone()),
            emoji: b.emoji.clone().unwrap_or_else(|| "📦".into()),
            description: b.description.clone().unwrap_or_default(),
            priority: b.priority.unwrap_or(100),
            selectable: b.selectable.unwrap_or(true),
            posture: posture.clone(),
        };
        let sub = |s: Option<String>| -> Option<String> {
            s.map(|v| v.replace("{dir}", &bundle_dir))
        };
        for p in &b.packages {
            let cmd = commands_for(p, os);
            let mgr = managers()
                .into_iter()
                .find(|m| Some(m.route) == cmd.route.as_deref());
            let system_id = mgr.and_then(|m| match m.id_field {
                IdField::Winget => p.winget.clone(),
                IdField::Brew => p.brew.clone(),
            });
            let is_config =
                p.check.is_some() && (cmd.route.is_none() || cmd.route.as_deref() == Some("run"));
            steps.push(Step {
                bundle: meta.name.clone(),
                name: p.name.clone(),
                description: p.description.clone().unwrap_or_default(),
                install: sub(cmd.install),
                uninstall: sub(cmd.uninstall),
                upgrade: sub(cmd.upgrade),
                downgrade: sub(cmd.downgrade),
                route: cmd.route,
                system_id,
                detect: p.detect.clone(),
                check: sub(p.check.clone()),
                is_config,
                version_regex: p.version_regex.clone(),
                pin: p.version.clone(),
                requires: p.requires.clone(),
                posture: meta.posture.clone(),
            });
        }
        log(&format!(
            "bundle loaded: {} ({} packages) from {dir_name}",
            meta.name,
            b.packages.len()
        ));
        bundles.push(meta);
    }
    bundles.sort_by_key(|b| b.priority);
    let prio: std::collections::HashMap<String, i64> =
        bundles.iter().map(|b| (b.name.clone(), b.priority)).collect();
    steps.sort_by_key(|s| *prio.get(&s.bundle).unwrap_or(&100));
    Plan { bundles, steps }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str) -> RawPkg {
        RawPkg {
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn brew_route_on_mac() {
        let mut p = pkg("jq");
        p.brew = Some("jq".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(c.route.as_deref(), Some("brew"));
        assert_eq!(c.install.as_deref(), Some("brew install --yes jq"));
    }

    #[test]
    fn cargo_route_pinned() {
        let mut p = pkg("ripgrep");
        p.cargo = Some("ripgrep".into());
        p.version = Some("14.0".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(
            c.install.as_deref(),
            Some("cargo install ripgrep --version 14.0")
        );
        assert!(c.downgrade.is_some());
    }

    #[test]
    fn run_route() {
        let mut p = pkg("starship-cfg");
        p.run = Some("nu {dir}/patch.nu".into());
        p.check = Some("test -f x".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(c.route.as_deref(), Some("run"));
    }

    #[test]
    fn no_route_yields_none() {
        let c = commands_for(&pkg("bare"), Os::Darwin);
        assert!(c.route.is_none() && c.install.is_none());
    }
}
