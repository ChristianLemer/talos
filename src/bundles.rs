// Scans bundles/ → { bundles, steps }. Pure (serde_yaml), no
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
    /// Behaviour OVERRIDES — an author's statement that beats what the fleet observed.
    /// `None` = no opinion (the collected fact stands); `Some(false)` = explicitly
    /// contradicting a collected `true`, which is the only way to un-ratchet a fact.
    ///
    /// These are also the SEED: a fresh machine has collected nothing, so the
    /// hand-written ones are what calibrate its first Apply.
    ///
    /// Read via `RawPkg::overrides`, never field-by-field, so the three-slot mapping
    /// lives in ONE place and stays pinned by one test.
    #[serde(default)]
    pub uac: Option<bool>,
    /// The YAML key is `403`. The Rust field cannot be, so serde renames it.
    #[serde(default, rename = "403")]
    pub forbidden: Option<bool>,
    /// A boolean, not a duration: the author says "this one is slow", while the
    /// MEASURED seconds come from the behaviour file. Declaring a number by hand
    /// would invite it to drift from what the machine actually observes.
    #[serde(default)]
    pub slow: Option<bool>,
}

impl RawPkg {
    /// What this package DECLARES about its behaviour. The one bridge from the parsed
    /// YAML to `resolve_facts`, so the three same-typed `Option<bool>` slots are
    /// transposed in at most one place — and that place has a test.
    ///
    /// This allow covers the three `RawPkg` fields as well: an allowed item is a live
    /// ROOT, so reading them here keeps them alive. Three field-level allows were measured
    /// first and are strictly redundant — the noise that becomes permanent. See
    /// `resolve_facts` for what removes both of the mechanism's allows.
    #[allow(dead_code)] // no consumer until something reads a package's facts to decide
    pub fn overrides(&self) -> Overrides {
        Overrides {
            uac: self.uac,
            forbidden: self.forbidden,
            slow: self.slow,
        }
    }
}

/// The three override slots, lifted out of RawPkg so the resolution is a pure
/// function of (collected, declared) and can be tested without building a package.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Overrides {
    pub uac: Option<bool>,
    pub forbidden: Option<bool>,
    pub slow: Option<bool>,
}

/// The duration a declared `slow: true` stands in for when nothing was ever measured.
/// A sentinel, not a real timing — the honest number always comes from the share.
///
/// It is an ANCHOR: the ladder's "slow" threshold must stay strictly BELOW it, or every
/// `slow: true` in the catalogue goes silently inert. It is `pub` for exactly that reason —
/// `ladder::SLOW_SECS` is asserted against it, which is the comparison the Part A plan
/// could only describe in a comment because this was a function-local const.
///
/// It sits ABOVE `resolve_facts`'s doc essay rather than between the essay and the `fn`:
/// `///` lines accumulate onto the next ITEM, so slipping a const in below them would have
/// silently re-parented that whole essay onto this constant.
///
/// No `allow` of its own, measured rather than assumed: `resolve_facts` reads it, and an
/// allowed item is a live ROOT, so this stays alive through it.
pub const DECLARED_SLOW_SECS: u64 = 600;

/// What the app should BELIEVE about a package: the fleet's observation, with the
/// catalogue's explicit statement taking precedence.
///
/// This is the escape hatch that makes a monotone ratchet safe. Facts only ever go
/// false→true from observation, so without a declared override a wrong `uac` (an
/// installer's own window mistaken for an elevation) or a stale `403` (the firewall
/// opened) would be permanent. Here it is correctable, in a versioned file, with a
/// diff that says who decided.
///
/// This function is PURE and reads `collected` without altering it, so nothing here can
/// reach the shared file.
///
/// ⚠️ AND A CALLER MUST NOT PUT IT BACK. This is a RULE, not a property anything enforces:
/// what comes out is a BELIEF, and it is the very same `behaviour::Facts` type that
/// `merge_into` and `merge_and_write` accept, so feeding a resolved value back into a
/// record compiles perfectly and would ratchet a declared `slow`'s sentinel into the share
/// PERMANENTLY — a wrong value on a monotone file, which is exactly what `behaviour.rs`
/// promises no write can produce. Merge only what a machine actually observed. Kept that
/// way, the collected fact survives on the share and returns the moment the override is
/// removed: nothing is un-observed, only re-interpreted.
///
/// `slow` is asymmetric on purpose: declaring it true must WORK on a machine that has
/// measured nothing, so it maps to a sentinel duration; declaring it false resets the
/// measurement to 0, saying "whatever you timed, treat this as quick".
///
/// ⚠️ That 0 COLLIDES with a documented convention: `Facts::slow_secs` and
/// `consent::HistEntry::secs` both read 0 as "never measured". So a `slow: false` package
/// will be counted among the UNKNOWNS in a total the ladder spec asks to report explicitly
/// (`at most ~4 min (3 unknown)`), rather than as a measured-quick one. Harmless for the
/// rung filter — 0 is not slow, which is the right answer — but a caller building that
/// count should know the two are indistinguishable here.
// TWO allows for this mechanism, measured by stripping each under `-D warnings` rather than
// guessed. `overrides` covers itself and the three `RawPkg` fields it reads; this one covers
// itself AND the `DECLARED_SLOW_SECS` above, which is alive only through this root (measured:
// stripping this allow reports the const unused too). An allowed item IS a live root, so
// `Overrides` is kept alive redundantly — by
// either allow independently, since `overrides` constructs it and `resolve_facts` takes it.
// Neither attribute subsumes the other all the same: dropping `overrides`'s leaves the three
// fields unread (reported as one warning), dropping this one leaves `resolve_facts` unused.
//
// Both leave together, and they did NOT leave when the Apply path started collecting facts:
// that wiring only WRITES observations, and feeding a RESOLVED fact back into it is the one
// thing the design forbids (see behaviour.rs). The first caller is whatever reads a
// package's facts to DECIDE something: the ladder, in its own plan.
#[allow(dead_code)] // no consumer until something reads a package's facts to decide
pub fn resolve_facts(
    collected: &crate::behaviour::Facts,
    declared: &Overrides,
) -> crate::behaviour::Facts {
    crate::behaviour::Facts {
        uac: declared.uac.unwrap_or(collected.uac),
        forbidden: declared.forbidden.unwrap_or(collected.forbidden),
        slow_secs: match declared.slow {
            // A measured duration WORSE than the sentinel is the honest number and wins:
            // the author says "slow", the machine says how slow.
            Some(true) => collected.slow_secs.max(DECLARED_SLOW_SECS),
            Some(false) => 0,
            None => collected.slow_secs,
        },
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    /// The catalogue id (the yaml file's stem, unless the file declares an explicit
    /// `id:`) — what names this package's behaviour file, so `behaviour/<id>.yaml` sits
    /// beside `catalog/<id>.yaml`. Distinct from `name`, which is the human label shown
    /// on the row and is free to change without orphaning any collected fact.
    pub id: String,
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
        //
        // TWO consents, not one, and only the second used to be answered. The trailing
        // `-y` goes to `skills`; before that ever runs, NPX asks its own question —
        // "Need to install the following packages: skills@1.5.21 / Ok to proceed? (y)"
        // — because the CLI is not installed locally. Nothing types into that pty, so
        // the row sat on "installing…" for good, with no way to cancel it. `npx --yes`
        // answers the fetch prompt (npm docs: "skip this prompt with the -y or --yes
        // option"); the tool's own `-y` still answers the tool's.
        //
        // Not paranoia about interactivity in general: a pty with nobody at the keyboard
        // must never be handed a question. Any prompt we cannot pre-answer is a hang.
        return Commands {
            route: Some("skill".into()),
            install: Some(format!("npx --yes skills add {src} -g -y")),
            uninstall: Some(format!("npx --yes skills remove {name} -y")),
            upgrade: Some(format!("npx --yes skills update {name} -y")),
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
            id: cp.id.clone(),
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
            Some("npx --yes skills add apollographql/skills@rust-best-practices -g -y")
        );
        assert_eq!(
            c.uninstall.as_deref(),
            Some("npx --yes skills remove rust-best-practices -y")
        );
        assert_eq!(
            c.upgrade.as_deref(),
            Some("npx --yes skills update rust-best-practices -y")
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

    // ---- Behaviour OVERRIDES: the escape hatch out of a monotone ratchet ----

    #[test]
    fn catalogue_overrides_win_over_collected_facts() {
        use crate::behaviour::Facts;
        // The share says "this elevates and is blocked and is slow".
        let collected = Facts {
            uac: true,
            forbidden: true,
            slow_secs: 900,
        };
        // The catalogue says otherwise, explicitly. The author wins: this is the
        // escape hatch that makes the ratchet correctable.
        let declared = Overrides {
            uac: Some(false),
            forbidden: Some(false),
            slow: None,
        };
        let f = resolve_facts(&collected, &declared);
        assert!(!f.uac, "an explicit false in the catalogue wins");
        assert!(!f.forbidden);
        assert_eq!(
            f.slow_secs, 900,
            "not declared → the collected value stands"
        );
    }

    #[test]
    fn an_absent_override_does_not_override() {
        use crate::behaviour::Facts;
        let collected = Facts {
            uac: true,
            forbidden: false,
            slow_secs: 100,
        };
        let f = resolve_facts(&collected, &Overrides::default());
        assert!(f.uac, "absent means 'no opinion', not 'false'");
        assert_eq!(f.slow_secs, 100);
    }

    #[test]
    fn a_declared_fact_seeds_a_machine_with_no_data() {
        use crate::behaviour::Facts;
        // A fresh machine has collected nothing. C's hand-written observations
        // (7-Zip, AWS CLI, Node.js, VS Code elevate; rclone is blocked) are the SEED,
        // so the first Apply on a new machine is already calibrated.
        let declared = Overrides {
            uac: Some(true),
            forbidden: None,
            slow: None,
        };
        let f = resolve_facts(&Facts::default(), &declared);
        assert!(f.uac, "the catalogue speaks when nothing was collected");
    }

    /// `slow` is the ONE asymmetric field — the catalogue says a boolean, the share holds
    /// seconds — so both of its declared branches need pinning. Neither is covered by the
    /// tests above, which only exercise `slow: None`: swapping the `max` for a `min`, or
    /// the sentinel for `0`, would leave all three of them green.
    #[test]
    fn a_declared_slow_works_with_no_measurement_and_never_lowers_one() {
        use crate::behaviour::Facts;
        // Nothing measured. `slow: true` must still land somewhere a rung will call slow,
        // otherwise declaring it would be inert on exactly the fresh machine it is for.
        let seeded = resolve_facts(&Facts::default(), &slow_is(Some(true)));
        assert!(
            seeded.slow_secs >= 600,
            "a declared slow must be slow with nothing measured, got {}",
            seeded.slow_secs
        );
        // A real measurement WORSE than the sentinel is the honest number and must survive:
        // the author says "slow", the machine says "how slow".
        let measured = Facts {
            slow_secs: 4_000,
            ..Facts::default()
        };
        assert_eq!(
            resolve_facts(&measured, &slow_is(Some(true))).slow_secs,
            4_000,
            "the sentinel must not lower a measured duration"
        );
        // And an explicit false clears it outright: "whatever you timed, treat this as quick".
        assert_eq!(
            resolve_facts(&measured, &slow_is(Some(false))).slow_secs,
            0,
            "an explicit false discards the measurement"
        );
        // The other two facts are untouched by the slow slot.
        let both = Facts {
            uac: true,
            forbidden: true,
            slow_secs: 10,
        };
        let r = resolve_facts(&both, &slow_is(Some(false)));
        assert!(r.uac && r.forbidden, "slow does not speak for uac or 403");
    }

    fn slow_is(slow: Option<bool>) -> Overrides {
        Overrides {
            slow,
            ..Overrides::default()
        }
    }

    /// The two halves JOINED: a catalogue file's bytes all the way to the resolved facts.
    /// Neither half proves this on its own — the parse test stops at `RawPkg`, and
    /// `resolve_facts` takes an `Overrides` somebody has to build. The lift between them is
    /// three same-typed `Option<bool>`s, so transposing two of them — or dropping one — is a
    /// silent bug that only an end-to-end assertion catches. All three slots are crossed
    /// here, from bytes a catalogue file could really contain (the shapes Task 7 will ship).
    #[test]
    fn a_declared_403_in_a_catalogue_file_reaches_the_resolved_facts() {
        use crate::behaviour::Facts;
        let cp = crate::catalog::parse_catalog_entry(
            "name: rclone\nwinget: Rclone.Rclone\n\"403\": true\n",
            "rclone",
        )
        .expect("parses");
        // Nothing collected — the seed case, a fresh machine.
        let f = resolve_facts(&Facts::default(), &cp.pkg.overrides());
        assert!(f.forbidden, "the declared 403 arrived");
        assert!(!f.uac, "and did not leak into the uac slot");
        assert_eq!(f.slow_secs, 0, "nor into the duration");

        // And the mirror: a declared `uac` must not read back as a 403.
        let cp = crate::catalog::parse_catalog_entry("name: 7-Zip\nuac: true\n", "7-zip")
            .expect("parses");
        let f = resolve_facts(&Facts::default(), &cp.pkg.overrides());
        assert!(f.uac);
        assert!(
            !f.forbidden,
            "the two Option<bool> slots are not transposed"
        );

        // `slow` crosses the lift too. EITHER branch below catches a lift that drops the
        // field — measured, by deleting one and applying the mutant: with nothing collected
        // a dropped `Some(true)` yields 0, not 600, and over a measurement a dropped
        // `Some(false)` yields 300, not 0. Both are kept because they pin different
        // MEANINGS, not for redundant coverage: that a declared slow works on a machine
        // with no data, and that a declared quick clears a real measurement.
        let cp = crate::catalog::parse_catalog_entry("name: Obsidian\nslow: false\n", "obsidian")
            .expect("parses");
        let measured = Facts {
            slow_secs: 300,
            ..Facts::default()
        };
        assert_eq!(
            resolve_facts(&measured, &cp.pkg.overrides()).slow_secs,
            0,
            "a declared `slow: false` must reach resolve_facts and clear the measurement"
        );
        let cp = crate::catalog::parse_catalog_entry("name: Obsidian\nslow: true\n", "obsidian")
            .expect("parses");
        assert!(
            resolve_facts(&Facts::default(), &cp.pkg.overrides()).slow_secs >= 600,
            "and a declared `slow: true` must reach it as the sentinel"
        );

        // A file declaring nothing yields the no-opinion overrides, so a collected fact
        // stands untouched — this is what all 31 shipped files do today.
        let cp = crate::catalog::parse_catalog_entry("name: jq\nbrew: jq\n", "jq").expect("parses");
        assert_eq!(cp.pkg.overrides(), Overrides::default());
        let collected = Facts {
            uac: true,
            forbidden: true,
            slow_secs: 77,
        };
        assert_eq!(
            resolve_facts(&collected, &cp.pkg.overrides()),
            collected,
            "a silent catalogue changes nothing"
        );
    }

    #[test]
    fn overrides_parse_from_yaml_with_the_403_key() {
        let raw = "\
name: rclone
winget: Rclone.Rclone
uac: false
\"403\": true
slow: true
";
        let cp = crate::catalog::parse_catalog_entry(raw, "rclone").expect("parses");
        assert_eq!(cp.pkg.uac, Some(false));
        assert_eq!(cp.pkg.forbidden, Some(true));
        assert_eq!(cp.pkg.slow, Some(true));
    }

    #[test]
    fn a_catalogue_file_without_overrides_still_parses() {
        // All existing files have none of these fields. They must keep working.
        let raw = "name: jq\nwinget: jqlang.jq\nbrew: jq\n";
        let cp = crate::catalog::parse_catalog_entry(raw, "jq").expect("parses");
        assert_eq!(cp.pkg.uac, None);
        assert_eq!(cp.pkg.forbidden, None);
        assert_eq!(cp.pkg.slow, None);
    }

    /// The SHIPPED catalogue, not a fixture. The synthetic test above proves that ONE
    /// hand-written file with no override fields parses; it says nothing about the 31 real
    /// ones. This walks them individually — `load_catalog` SKIPS an unparseable file in
    /// silence, so a count taken through it could stay plausible while a file rotted — and
    /// then checks that the loader still yields one entry per file, which is what a caller
    /// actually gets.
    ///
    /// The count is a floor, not the exact 31, deliberately: adding a package is a normal
    /// gesture and must not turn this file red for an unrelated reason. What the floor
    /// protects is the loop's meaning — a glob that matched nothing would pass vacuously.
    #[test]
    fn every_shipped_catalogue_file_still_parses_after_the_new_fields() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"));
        let mut files = 0usize;
        for entry in std::fs::read_dir(dir)
            .expect("catalog/ is shipped")
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue; // sidecars like starship.nu are not packages
            }
            files += 1;
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap();
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                crate::catalog::parse_catalog_entry(&raw, stem).is_some(),
                "{} no longer parses",
                path.display()
            );
        }
        assert!(
            files > 20,
            "only {files} catalogue files walked — the glob found (almost) nothing"
        );
        assert_eq!(
            crate::catalog::load_catalog(dir.to_str().unwrap()).len(),
            files,
            "the loader must still yield one entry per catalogue file"
        );
    }

    #[test]
    fn the_shipped_seed_reaches_overrides() {
        // The walk above proves every file still PARSES, which already catches a bad VALUE:
        // `uac: ture` is not a bool, so `parse_catalog_entry` returns None and the walk
        // trips (measured, both ways). What it does NOT catch is a bad KEY — `uacc: true`
        // parses fine, because serde ignores an unknown field in silence, leaving the
        // declaration inert and a fresh machine's first Apply uncalibrated. That one hole is
        // why this test exists: it pins the VALUES that ship, through the same lift the
        // resolution uses.
        //
        // Names the five stems on purpose rather than walking for whatever declares `uac`.
        // The fixed set is the stronger guard — DELETING a seed turns it red, which a
        // "some file declares something" check would sail past — and adding a sixth
        // observation still will not, since nothing here asserts a total.
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"));
        let declared = |stem: &str| {
            let raw = std::fs::read_to_string(dir.join(format!("{stem}.yaml"))).unwrap();
            crate::catalog::parse_catalog_entry(&raw, stem)
                .expect("parses")
                .pkg
                .overrides()
        };

        // C observed these four elevating on Windows via winget.
        for stem in ["7-zip", "aws-cli", "node", "visual-studio-code"] {
            let o = declared(stem);
            assert_eq!(o.uac, Some(true), "{stem} must declare uac: true");
            // ⚠️ A LATENT false red: if any of these four is ever observed hitting a 403 at
            // a corporate network, declaring it here turns this line red. That is the tripwire working —
            // update the line, do not delete the assertion.
            assert_eq!(o.forbidden, None, "{stem} says nothing about the firewall");
        }

        // And rclone meeting a corporate firewall, which pins the `rename = "403"` bridge.
        // (Measured: serde_yaml accepts a BARE `403:` here too — it matches the renamed
        // field on the key's text, integer-looking or not. The shipped file quotes it for
        // the reader, not out of necessity.)
        let o = declared("rclone");
        assert_eq!(o.forbidden, Some(true), "rclone must declare 403: true");
        assert_eq!(o.uac, None, "rclone says nothing about elevation");

        // A package with no declaration must stay silent — otherwise the seed is not a
        // seed but a default, and "no opinion" would have collapsed into "false".
        assert_eq!(declared("jq").uac, None);
        assert_eq!(declared("jq").forbidden, None);
        assert_eq!(declared("jq").slow, None);
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
