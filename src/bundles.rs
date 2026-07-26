// Port of bundles.ts — scan bundles/ → { bundles, steps }. Pure (serde_yaml), no
// pty/network. The route table: a package = a NEED satisfied by a named route.
use serde::Deserialize;

use crate::managers::{managers, native_manager, IdField};
use crate::platform::Os;

// The install-policy vocabulary the front-end speaks (see model.js isLockedPosture:
// `forbidden` locks a row). `load_from_catalog` emits every package as `OptIn`
// today; per-package posture declared in catalog YAML is a planned add, at which
// point `parse` wires the other variants back in. Kept as the extension point.
#[allow(dead_code)] // only OptIn is constructed until per-package YAML posture lands
#[derive(Debug, Clone, PartialEq)]
pub enum Posture {
    Mandatory,
    OptOut,
    OptIn,
    Forbidden,
}

impl Posture {
    #[allow(dead_code)] // wiring stub for per-package YAML posture (see enum note)
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

#[derive(Debug, Deserialize, Default, Clone)]
pub struct RawPkg {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub winget: Option<String>,
    #[serde(default)]
    pub brew: Option<String>,
    #[serde(default)]
    pub cargo: Option<String>,
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default, rename = "npmFlags")]
    pub npm_flags: Option<String>,
    #[serde(default)]
    pub bun: Option<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default, rename = "runUninstall")]
    pub run_uninstall: Option<String>,
    #[serde(default, rename = "claude-plugin")]
    pub claude_plugin: Option<String>,
    #[serde(default)]
    pub marketplace: Option<String>,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default, rename = "skillName")]
    pub skill_name: Option<String>,
    #[serde(default)]
    pub detect: Option<String>,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default, rename = "version-regex")]
    pub version_regex: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub category: Vec<String>,
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
    pub categories: Vec<String>,
}

pub struct Commands {
    pub route: Option<String>,
    pub install: Option<String>,
    pub uninstall: Option<String>,
    pub upgrade: Option<String>,
    pub downgrade: Option<String>,
}

/// The NAMED ROUTE TABLE — port of commandsFor. Family 1 (system manager, arbitrated by
/// OS) first, then cargo/npm/bun/run/claude-plugin/skill.
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
                upgrade: Some(mgr.upgrade(id, false)), // formula form; server.rs re-forces for casks at apply
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
    // npm BEFORE bun: both routes live here, but npm is the one whose runtime the
    // socle guarantees (Node.js is in Base — third-party plugin hooks hardcode
    // `node`). Bun installs the same binaries, yet betting the agent's own install
    // on a runtime nothing else in the ecosystem assumes buys nothing.
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
    // Bun route KEPT and working: Bun stays in the catalogue (installable on its own,
    // nothing pulls it), so a package may still route through it deliberately.
    if let Some(n) = &pkg.bun {
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
            route: Some("bun".into()),
            install: Some(format!("bun add -g {inst_target}")),
            uninstall: Some(format!("bun remove -g {n}")),
            upgrade: Some(format!("bun add -g {up_target}")),
            downgrade: if ver.is_empty() {
                None
            } else {
                Some(format!("bun remove -g {n} && bun add -g {inst_target}"))
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
        // `npx`, not `bunx`. The skills CLI is third-party JS, and third-party JS is
        // precisely what cannot be assumed to run on a non-Node runtime.
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
    pub steps: Vec<Step>,
}

/// FLAT-model loader (bundle-driven, spec Consolidation §6): emits EVERY catalog
/// package as a step (so the Catalog tab can list all), with `Step.bundle = ""` —
/// bundles no longer OWN packages, they only pull them (loaded separately via
/// profiles.rs as the top cards). Nothing is wanted by default; a package becomes
/// "in" only when an active bundle pulls it or the user toggles it. `{dir}`
/// resolves to the catalog dir (where sidecar files like starship.nu live).
/// `_bundles_dir` is unused here (bundles feed the cards, not the steps).
pub fn load_from_catalog(
    catalog_dir: &str,
    _bundles_dir: &str,
    os: Os,
    log: &dyn Fn(&str),
) -> Plan {
    let catalog = crate::catalog::load_catalog(catalog_dir);
    log(&format!(
        "catalog: {} packages from {catalog_dir}",
        catalog.len()
    ));
    let sub = |s: Option<String>| -> Option<String> { s.map(|v| v.replace("{dir}", catalog_dir)) };
    let mut steps: Vec<Step> = Vec::new();
    for cp in catalog.values() {
        let p = &cp.pkg;
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
            bundle: String::new(), // bundles don't own packages anymore
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
            posture: Posture::OptIn, // catalog default: free + out-by-default
            // (bundle-driven: the bundle pull decides "in", not the posture).
            // per-package posture declared in YAML is a later add.
            categories: if p.category.is_empty() {
                vec!["misc".to_string()]
            } else {
                p.category.clone()
            },
        });
    }
    Plan { steps }
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
    fn npm_route_global_install() {
        let mut p = pkg("Claude Code");
        p.npm = Some("@anthropic-ai/claude-code".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(c.route.as_deref(), Some("npm"));
        assert_eq!(
            c.install.as_deref(),
            Some("npm install -g @anthropic-ai/claude-code")
        );
        assert_eq!(
            c.uninstall.as_deref(),
            Some("npm uninstall -g @anthropic-ai/claude-code")
        );
        assert_eq!(
            c.upgrade.as_deref(),
            Some("npm install -g @anthropic-ai/claude-code@latest")
        );
    }

    #[test]
    fn npm_route_pinned() {
        let mut p = pkg("Claude Code");
        p.npm = Some("@anthropic-ai/claude-code".into());
        p.version = Some("2.1.220".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(
            c.install.as_deref(),
            Some("npm install -g @anthropic-ai/claude-code@2.1.220")
        );
        assert_eq!(
            c.downgrade.as_deref(),
            Some("npm uninstall -g @anthropic-ai/claude-code && npm install -g @anthropic-ai/claude-code@2.1.220")
        );
    }

    // Both JS routes stay in the table: Bun remains in the catalogue (nothing pulls
    // it), so a package may still declare `bun:`. npm wins when both are declared —
    // it is the route whose runtime the socle guarantees.
    #[test]
    fn npm_wins_over_bun_when_both_declared() {
        let mut p = pkg("Ambiguous");
        p.npm = Some("thing".into());
        p.bun = Some("thing".into());
        assert_eq!(commands_for(&p, Os::Darwin).route.as_deref(), Some("npm"));
    }

    #[test]
    fn bun_route_global_install() {
        let mut p = pkg("Claude Code");
        p.bun = Some("@anthropic-ai/claude-code".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(c.route.as_deref(), Some("bun"));
        assert_eq!(
            c.install.as_deref(),
            Some("bun add -g @anthropic-ai/claude-code")
        );
        assert_eq!(
            c.uninstall.as_deref(),
            Some("bun remove -g @anthropic-ai/claude-code")
        );
        assert_eq!(
            c.upgrade.as_deref(),
            Some("bun add -g @anthropic-ai/claude-code@latest")
        );
    }

    #[test]
    fn bun_route_pinned() {
        let mut p = pkg("Claude Code");
        p.bun = Some("@anthropic-ai/claude-code".into());
        p.version = Some("2.1.220".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(
            c.install.as_deref(),
            Some("bun add -g @anthropic-ai/claude-code@2.1.220")
        );
        assert_eq!(
            c.downgrade.as_deref(),
            Some("bun remove -g @anthropic-ai/claude-code && bun add -g @anthropic-ai/claude-code@2.1.220")
        );
    }

    #[test]
    // `npx`, not `bunx`: the skills CLI is third-party code, and third-party JS is
    // exactly what cannot be assumed to run on a non-Node runtime.
    fn skill_route_uses_npx() {
        let mut p = pkg("Rust best practices");
        p.skill = Some("apollographql/skills@rust-best-practices".into());
        p.skill_name = Some("rust-best-practices".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(c.route.as_deref(), Some("skill"));
        assert_eq!(
            c.install.as_deref(),
            Some("npx skills add apollographql/skills@rust-best-practices -g -y")
        );
        assert_eq!(
            c.uninstall.as_deref(),
            Some("npx skills remove rust-best-practices -y")
        );
        assert_eq!(
            c.upgrade.as_deref(),
            Some("npx skills update rust-best-practices -y")
        );
    }

    /// The SHIPPED catalogue, not a fixture: a package routed through npm/npx must
    /// REQUIRE Node.js. This is the bug's shape generalised — the route named one
    /// runtime, the machine had another, and nothing tied the two together. It lives
    /// here, beside the npm route, so reverting the routing decision takes its guard
    /// with it and nothing dangles. Its Bun twin is in catalog.rs.
    #[test]
    fn shipped_npm_routed_packages_require_node() {
        let cat = crate::catalog::load_catalog(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"));
        for c in cat.values() {
            let p = &c.pkg;
            // `skill:` installs through `npx`, which ships with Node (see commands_for).
            if p.npm.is_some() || p.skill.is_some() {
                assert!(
                    p.requires.iter().any(|r| r == "Node.js"),
                    "{}: routes through npm/npx but does not require Node.js",
                    p.name
                );
            }
        }
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

    #[test]
    fn emits_every_catalog_package_with_empty_bundle() {
        use std::fs;
        let root = std::env::temp_dir().join("talos-test-b2-catalog");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("catalog")).unwrap();
        fs::write(
            root.join("catalog/git.yaml"),
            "name: Git\nbrew: git\ncategory: [vcs]\n",
        )
        .unwrap();
        fs::write(
            root.join("catalog/node.yaml"),
            "name: Node.js\nbrew: node\n",
        )
        .unwrap();
        // Bundle-driven: EVERY catalog package emits once, bundle="" (bundles no
        // longer own packages). bundles_dir is unused now.
        let plan = load_from_catalog(
            root.join("catalog").to_str().unwrap(),
            "unused",
            Os::Darwin,
            &|_| {},
        );
        assert_eq!(plan.steps.len(), 2);
        let git = plan.steps.iter().find(|s| s.name == "Git").unwrap();
        assert_eq!(git.bundle, ""); // no owning bundle
        assert_eq!(git.categories, vec!["vcs"]);
        assert_eq!(git.route.as_deref(), Some("brew"));
        // Node has no category tag → defaults to misc.
        let node = plan.steps.iter().find(|s| s.name == "Node.js").unwrap();
        assert_eq!(node.categories, vec!["misc"]);
        let _ = fs::remove_dir_all(&root);
    }

    // B5: `requires:` from catalog YAML must reach the Step (the front does the
    // transitive pull over it — see model.js wantedNames).
    #[test]
    fn step_carries_requires_from_catalog() {
        use std::fs;
        let root = std::env::temp_dir().join("talos-test-b5-requires");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("catalog")).unwrap();
        fs::write(
            root.join("catalog/claude-code.yaml"),
            "name: Claude Code\nbun: '@anthropic-ai/claude-code'\nrequires:\n  - Bun\n",
        )
        .unwrap();
        let plan = load_from_catalog(
            root.join("catalog").to_str().unwrap(),
            "unused",
            Os::Darwin,
            &|_| {},
        );
        let cc = plan.steps.iter().find(|s| s.name == "Claude Code").unwrap();
        assert_eq!(cc.requires, vec!["Bun"]);
        let _ = fs::remove_dir_all(&root);
    }
}
