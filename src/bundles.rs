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
    /// A VS Code extension id, `publisher.name`. The YAML key is `vscode-extension`; Rust
    /// cannot hold the hyphen, hence the rename — same as `claude-plugin` above.
    #[serde(default, rename = "vscode-extension")]
    pub vscode_extension: Option<String>,
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
    /// Reading the three fields through ONE accessor is also what spares them three
    /// `allow(dead_code)` of their own: an item that is read is alive, and everything above
    /// stays alive through this one function. That mattered while nothing consumed the
    /// declarations at all and it still holds now — `load_from_catalog` calls this to fill
    /// `Step::overrides`, so the allow this carried is gone, measured by stripping it under
    /// `-D warnings`.
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
/// No `allow` of its own, measured rather than assumed: `resolve_facts` reads it, and that
/// function now has a real caller of its own, so nothing here is alive only by permission.
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
// The `allow(dead_code)` this mechanism carried on both `overrides` and this function is
// GONE, stripped one at a time under `-D warnings` rather than as a batch. The caller is
// `ladder::resolve_plan_facts`, reached from `server::serve`: it looks each step's collected
// facts up on the share and lays `Step::overrides` over them, which is what finally makes an
// override written in catalog/ change something observable.
//
// It did NOT arrive when the Apply path started collecting facts. That wiring only WRITES
// observations, and feeding a RESOLVED fact back into it is the one thing the design forbids
// (see the ⚠️ above and behaviour.rs). It took a READER to make these live.
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
    /// Does this install INTO a host rather than onto the machine? A Claude plugin, a
    /// cross-agent skill — and, when its route lands, an editor extension.
    ///
    /// ⭐ DERIVED from the route (`is_extension_route`), never declared in YAML, exactly like
    /// `is_config` above. It earns its own ladder rung because it carries a promise the two
    /// neighbouring rungs do not: it TOUCHES THE NETWORK (a clone, a download) and it NEVER
    /// TOUCHES THE MACHINE — nothing enters Program Files or /Applications, nothing elevates.
    ///
    /// ✅ That second half is a measured property of the corpus, not a hope: across the 34
    /// shipped packages, no `claude-plugin` and no `skill` declares `uac` or `403`, while nine
    /// binary packages declare `uac: true`.
    pub is_extension: bool,
    pub version_regex: Option<String>,
    pub pin: Option<String>,
    #[allow(dead_code)]
    pub requires: Vec<String>,
    pub posture: Posture,
    pub categories: Vec<String>,
    /// What this package DECLARES about its behaviour, carried from the catalogue so a
    /// consumer can resolve it against what the fleet observed. NOT the resolved value:
    /// resolution needs the collected half, which lives on the share and is read by the
    /// server, not here.
    pub overrides: Overrides,
}

pub struct Commands {
    pub route: Option<String>,
    pub install: Option<String>,
    pub uninstall: Option<String>,
    pub upgrade: Option<String>,
    pub downgrade: Option<String>,
}

/// The NAMED ROUTE TABLE — port of commandsFor. Family 1 (system manager, arbitrated by
/// OS) first, then cargo/npm/bun/run/claude-plugin/skill/vscode-extension.
fn commands_for(pkg: &RawPkg, os: Os) -> Commands {
    // The version to INTERPOLATE INTO A COMMAND — empty unless the author declared an exact
    // one.
    //
    // ⚠️ A KEYWORD MUST NEVER LAND HERE. Before this, the binding was
    // `pkg.version.as_deref().unwrap_or("").trim().to_string()` and the branch below is
    // `if !ver.is_empty()`, so `version: "pending"` took the PINNED path and
    // `install_pinned` emitted `brew install --yes git@pending` /
    // `winget install --id Git.Git -e --version pending` — a command built to fail, on
    // install AND upgrade AND downgrade. Measured, not feared.
    //
    // ⭐ `latest` and `pending` are statements about POLICY (take the newest / hold this),
    // not about which version to fetch. `action_for` consults them through `Step.pin`; the
    // command builder must not see them at all. One classification (`classify_pin`), two
    // consumers, opposite needs.
    let ver = match classify_pin(pkg.version.as_deref()) {
        Some(PinKind::Exact(v)) => v,
        // Latest / Pending / Invalid → build the PLAIN command, exactly as an undeclared
        // version does.
        _ => String::new(),
    };
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
        // Before the `add`, SHOW what decides whether the clone can work. The row's
        // terminal used to carry only claude's own message, which is accurate but reads
        // wrong: *"Command 'git' not found or is in an unsafe location (current
        // directory)"* looks like "there is no git" and actually means "every git I found
        // lives under the working directory".
        //
        // ⚠️ The FIRST version of this diagnostic hunted a `url.*.insteadOf` rewrite. That
        // suspect was measured on the machine and REFUTED — no rewrite existed, the
        // credential helper was the plain `manager`, and there was no ~/.ssh/config. The
        // https→ssh switch is INTERNAL to claude (SSH is its default for a short
        // `owner/repo`), which is exactly what a `git config` read cannot show. So the
        // questions are now the two that were actually load-bearing:
        //   1. WHICH git, and the CWD it is judged against — the pair that produces the
        //      message. `pty::safe_working_dir` fixes the cwd; echoing it is how the row
        //      proves the fix is in force rather than asking anyone to trust it.
        //   2. is the clone going over https — `CLAUDE_CODE_PLUGIN_PREFER_HTTPS`, set in
        //      the pty environment, is what keeps the ssh agent (and 1Password) out of a
        //      public clone. Printing it makes an environment-only fix auditable.
        //
        // ⚠️ Joined with `;`, never `&&`: a diagnostic that can fail the install it was
        // added to debug is worse than no diagnostic. Every command is read-only and
        // non-interactive — a prompting command deadlocks a display-only row (memory
        // display-only-terminal).
        //
        // ⭐ AND THE LABELS CARRY NO PARENTHESIS AT ALL. The first version of this
        // diagnostic wrote them bare, which broke the whole route on Windows: PowerShell
        // reads an unquoted `(` as a sub-expression, parses the inside as CODE and RUNS
        // it. Reproduced locally under pwsh, four ways:
        //     echo [talos] cwd (uname -a):;   → printed the uname output: a process ran
        //     echo [talos] c (1 = no ssh):;   → ParserError, exit 1
        //     echo [talos] c (1 no ssh):;     → ParserError too, so it is NOT the `=`
        //     echo [talos] git candidates:;   → fine, brackets are harmless
        // So the label `(claude refuses a git BELOW it)` INVOKED claude, and the chain
        // died before `marketplace add` — which is why beta.23's and beta.24's fixes
        // never executed at all.
        //
        // Quoting would be enough, and is not what this does. A quote is one keystroke
        // from being lost by a later edit, and the failure it re-opens is silent on Mac
        // and total on Windows. Prose in a label buys nothing the doc comments above do
        // not already say, so the labels are now plain words: nothing to quote, nothing
        // to execute. The `unquoted_parens_free` guard stays as the backstop for anything
        // a future author writes.
        let git_diag = if pkg.marketplace.is_some() {
            match os {
                Os::Windows => concat!(
                    "echo '[talos] cwd -- claude refuses a git below it:'; ",
                    "$PWD.Path; ",
                    "echo '[talos] git candidates:'; ",
                    "where.exe git; ",
                    "echo '[talos] https for plugin clones, 1 = no ssh agent:'; ",
                    "$env:CLAUDE_CODE_PLUGIN_PREFER_HTTPS; ",
                )
                .to_string(),
                _ => concat!(
                    "echo '[talos] cwd:'; pwd; ",
                    "echo '[talos] git candidates:'; command -v git; ",
                    "echo '[talos] https for plugin clones:'; ",
                    "echo \"${CLAUDE_CODE_PLUGIN_PREFER_HTTPS:-unset}\"; ",
                )
                .to_string(),
            }
        } else {
            String::new()
        };
        let add = pkg
            .marketplace
            .as_deref()
            .map(|m| format!("claude plugin marketplace add \"{m}\" && "))
            .unwrap_or_default();
        return Commands {
            route: Some("claude-plugin".into()),
            // `install` THEN `update`, and the second is what actually does the work on a
            // machine that already has the plugin.
            //
            // ⚠️ MEASURED, not assumed. `claude plugin install` on an already-installed id
            // answers `✔ Plugin "…" is already installed (scope: user)` and changes nothing —
            // so changing a package's `marketplace:` to a corrected fork switches the SOURCE
            // (the new version is even downloaded into the cache) while leaving the OLD plugin
            // active. Talos would report a green row over a stale plugin.
            //
            // The root cause is upstream of this line and is NOT fixed here: `scan_outdated`
            // only asks the native manager (winget/brew), which knows nothing about Claude
            // plugins, so `f.outdated` is always false for this route and `action_for` can
            // never choose `Upgrade`. Every plugin and skill in the catalogue is un-updatable
            // by Talos for the same reason. Comparing the installed version against the
            // marketplace's would fix it properly and let the existing machinery do the rest;
            // that is its own piece of work.
            //
            // Until then this chain is deliberately opportunistic: `update` on an
            // already-current plugin is a no-op, so the cost is one extra command and the
            // benefit is that a corrected source actually reaches the machine.
            install: Some(format!(
                "{git_diag}{add}claude plugin install {id} --scope user && claude plugin update {id}"
            )),
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
    if let Some(id) = &pkg.vscode_extension {
        // The `code` CLI is the only way to fetch from the gallery, so the WRITE half shells
        // out — while presence is read from the profile manifest (vscode.rs). Not an
        // inconsistency: it is brew's own asymmetry, where `detect:` tells the truth and the
        // manager acts.
        //
        // ⭐ `--force` on the install, and it is load-bearing. MEASURED: without it, an
        // install of an already-present extension prints "Extension '…' v… is already
        // installed. Use '--force' option to update…" and EXITS 0 having done nothing. A
        // silent no-op that exits 0 is the worst shape available here — convergence logic
        // reads it as success. It is also the documented flag "to avoid prompts", which
        // matters because a prompting command deadlocks a display-only row.
        //
        // ⚠️ NO `version:` SUPPORT. Measured: `--install-extension id@<version>` exits 1
        // when the gallery no longer serves that version, and the gallery serves exactly one
        // version for many extensions. A pin here would build a command designed to fail, so
        // `ver` is deliberately not consulted in this branch.
        //
        // ⭐ Ignoring `ver` HERE is only half the job: a pin reaches `Action::Upgrade` by a
        // second road, through `Step.pin` and `action_for`, which consults the pin BEFORE
        // `outdated`. `load_from_catalog` is where that half is closed — it refuses to carry a
        // pin for any extension route. Both halves are needed; neither alone suffices.
        //
        // ⚠️ NO UPGRADE. `scan_outdated` asks the native manager only, so `f.outdated` is
        // always false for this route — the `outdated` road to Upgrade is closed, and the pin
        // road is closed at `Step.pin`. A wired `upgrade:` would be unreachable, exactly as the
        // claude-plugin route's is. The install is idempotent (measured: exit 0 on an
        // already-current extension), so a re-Apply is safe and no freshness is claimed.
        // `--update-extensions` is refused for a different reason: it updates EVERYTHING,
        // which contradicts the per-package model.
        return Commands {
            route: Some("vscode-extension".into()),
            install: Some(format!("code --install-extension {id} --force")),
            uninstall: Some(format!("code --uninstall-extension {id}")),
            upgrade: None,
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
        let is_extension = is_extension_route(cmd.route.as_deref());
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
            // For a content route the `detect:` field means "the NAME to look for", not a
            // command. A VS Code extension's name IS its id, so the id is the default and an
            // explicitly declared `detect:` still wins. Filling it here rather than adding a
            // field to `Step` is deliberate: Step is filled at four literal sites (here plus
            // three test builders), so every new field costs four edits.
            detect: p.detect.clone().or_else(|| p.vscode_extension.clone()),
            check: sub(p.check.clone()),
            is_config,
            is_extension,
            version_regex: p.version_regex.clone(),
            // ⚠️ A PIN IS ONLY CARRIED IF THE ROUTE CAN HONOUR IT. `action_for` consults the
            // pin BEFORE `outdated` (decision.rs:90), so a pin on a route with no `upgrade:`
            // command yields Action::Upgrade → `do_step` resolves it to None → a SILENT SKIP:
            // no step message, no terminal line, no reason. That is the one outcome this repo
            // refuses ("every gesture leaves a trace"), and the front would draw an update
            // button that does nothing.
            //
            // A `vscode-extension` cannot be pinned at all: MEASURED, `code
            // --install-extension id@<version>` exits 1 once the gallery stops serving that
            // version. So dropping the pin here is not a limitation being hidden — it is the
            // catalogue declaring something the route has no way to mean.
            //
            // ⭐ This closes the SAME latent hole in `claude-plugin` and `skill`, which are
            // extension routes and which likewise never consult `ver` when building their
            // commands. Reusing the already-derived `is_extension` is deliberate: one
            // derivation site, and it reads as "an extension carries no pin".
            pin: if is_extension {
                None
            } else {
                p.version.clone()
            },
            requires: p.requires.clone(),
            posture: Posture::OptIn, // catalog default: free + out-by-default
            // (bundle-driven: the bundle pull decides "in", not the posture).
            // per-package posture declared in YAML is a later add.
            categories: if p.category.is_empty() {
                vec!["misc".to_string()]
            } else {
                p.category.clone()
            },
            overrides: p.overrides(),
        });
    }
    Plan { steps }
}

/// Is this route one that installs INTO a host rather than onto the machine?
///
/// The single place the class is decided, so the ladder, the wire and the tests cannot drift
/// apart. `claude-plugin` clones a marketplace into `~/.claude`; `skill` drops a SKILL.md into
/// `~/.claude/skills` or `~/.agents/skills`. Neither writes outside the user's profile and
/// neither can elevate.
///
/// ⚠️ A `run:` route is NOT here even though it also stays local: it is a config-atom, the rung
/// BELOW, and its promise is stronger still (no network at all). The two classes are disjoint
/// by construction — a config-atom is recognised by having a `check:`, which no extension has.
///
/// ✅ `vscode-extension` landed on 2026-08-15 and cost exactly one arm here, which is what the
/// function existed to make true. It writes into `~/.vscode/extensions` — the user's profile,
/// never `Program Files` — downloads from the gallery (measured 4.3 s fresh), and cannot
/// elevate.
///
/// ⬜ A `code-insiders` or Cursor host would be a DIFFERENT route (different binary, different
/// profile folder, and Cursor does not even use the same gallery), and would join this list the
/// same way.
pub fn is_extension_route(route: Option<&str>) -> bool {
    matches!(
        route,
        Some("claude-plugin") | Some("skill") | Some("vscode-extension")
    )
}

/// What a catalogue author DECLARED in `version:`.
///
/// ⭐ One classification, consumed by two sides with opposite needs:
///   · `commands_for` must never see a keyword — `install_pinned` would emit
///     `brew install --yes git@pending`, a command built to fail;
///   · `action_for` must see it — the keyword is a statement about policy ("hold this", "take
///     the newest"), which is exactly what decides whether to act.
/// Reading the raw string twice is how those two would come to disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinKind {
    /// An exact version to converge on. The only variant a command may interpolate.
    Exact(String),
    /// "I decided: the newest." Behaves as today's silence — and is NOT redundant with it:
    /// the difference between an intention and an absence of decision is what a catalogue
    /// meant to be copied from must be able to express.
    Latest,
    /// "Nobody has decided yet." Held out of the batch; the gap stays visible.
    Pending,
    /// ⚠️ Anything else, carrying the word so the caller can name it in a log line. NEVER
    /// interpreted as a version: MEASURED, `compare_versions("2.50.1", "current") == 1`
    /// because a word has no leading digits and reads as 0.0.0 — so `action_for` would return
    /// Downgrade and offer the only destructive path in the app.
    Invalid(String),
}

/// Classify `version:`. `None` ⇒ nothing was declared (today's behaviour: chase the newest).
///
/// The discriminator for an exact version is "starts with a digit", deliberately NOT a semver
/// parse: this codebase does not do semver (`compare_versions` compares numeric prefixes), and
/// real versions here look like `2026.72.0` and `1.0-beta`.
pub fn classify_pin(declared: Option<&str>) -> Option<PinKind> {
    let raw = declared?.trim();
    if raw.is_empty() {
        return None;
    }
    // Case-insensitive: a maintainer who types `Pending` means the keyword. `eq_ignore_ascii_case`
    // allocates nothing, and both words are ASCII by construction.
    if raw.eq_ignore_ascii_case("latest") {
        return Some(PinKind::Latest);
    }
    if raw.eq_ignore_ascii_case("pending") {
        return Some(PinKind::Pending);
    }
    if raw.starts_with(|c: char| c.is_ascii_digit()) {
        return Some(PinKind::Exact(raw.to_string()));
    }
    Some(PinKind::Invalid(raw.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panics if `line` contains a parenthesis a shell would treat as CODE.
    ///
    /// ⭐ The defect this exists for shipped in beta.24 and broke the whole plugin route
    /// on Windows: three `echo` labels were written unquoted, and PowerShell reads an
    /// unquoted `(` as the start of a sub-expression — it parses the inside as code and
    /// RUNS it. Reproduced locally under `pwsh`:
    ///     echo [talos] cwd (uname -a):;   → printed uname's output; a process was spawned
    ///     echo [talos] c (1 = no ssh):;   → ParserError, exit 1
    ///     echo [talos] c (1 no ssh):;     → ParserError as well, so it is NOT the `=`
    ///     echo [talos] git candidates:;   → fine; brackets are harmless, only parens bite
    ///
    /// ⚠️ Why the rule is "no parenthesis in a label" and not "quote your parentheses".
    /// Quoting works — verified, `echo '[talos] cwd (uname -a):'` prints it literally —
    /// but it makes correctness depend on a single keystroke surviving every future edit,
    /// and its failure is INVISIBLE on Mac and TOTAL on Windows. A rule that cannot be
    /// half-applied is worth more than one that can. Parens are still allowed where they
    /// carry meaning — inside `$(...)`, `${...}` and PowerShell property access — because
    /// there they ARE the code, deliberately.
    ///
    /// Reports the offending fragment, not just a boolean: a failure naming the line but
    /// not the spot sends the reader back to a 300-character string to hunt for it.
    fn assert_shell_safe(line: &str, what: &str) {
        let chars: Vec<char> = line.chars().collect();
        let mut in_single = false;
        let mut in_double = false;
        for (i, &ch) in chars.iter().enumerate() {
            match ch {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                '(' if !in_single => {
                    // Meaningful, deliberate parens: `$(cmd)`, `${x}`, and PowerShell's
                    // `(expr).Prop` / `(Get-Command x)`. Only the first is a substitution
                    // this code writes today; the check names the rest so a legitimate
                    // future use is not blocked by a test that cannot tell them apart.
                    let preceded_by_dollar = i > 0 && chars[i - 1] == '$';
                    assert!(
                        preceded_by_dollar,
                        "{what}: an unquoted `(` is CODE to PowerShell and gets RUN. \
                         At char {i}: …{}… \nFull line: {line}",
                        chars[i.saturating_sub(20)..(i + 20).min(chars.len())]
                            .iter()
                            .collect::<String>()
                    );
                }
                _ => {}
            }
        }
        assert!(
            !in_single && !in_double,
            "{what}: an unbalanced quote — the rest of the line is not what it looks like: \
             {line}"
        );
    }

    #[test]
    fn the_shell_safety_rule_holds_before_anything_relies_on_it() {
        // Pin the checker itself, or a silent bug in it makes every caller vacuously
        // green — the way a guard stops guarding without anyone noticing.
        assert_shell_safe("echo '[talos] cwd -- safe:'; $PWD.Path", "quoted label");
        assert_shell_safe("echo '[talos] git candidates:'; where.exe git", "plain");
        assert_shell_safe("brew list --versions git", "no parens at all");
        // `$(...)` is a substitution: allowed, because there the parens ARE the intent.
        assert_shell_safe("echo \"$(git --version)\"", "deliberate substitution");

        // And the shapes that must FAIL. Each is a real line from the shipped defect.
        for bad in [
            "echo [talos] cwd (claude refuses a git BELOW it):; ",
            "echo [talos] https for plugin clones (1 = no ssh agent, no 1Password):; ",
            "echo [a] (git --version)",
            "echo 'unbalanced",
        ] {
            let caught = std::panic::catch_unwind(|| assert_shell_safe(bad, "mutant")).is_err();
            assert!(caught, "the checker must reject: {bad}");
        }
    }

    #[test]
    fn a_content_route_is_an_extension_and_a_binary_route_is_not() {
        // ⭐ DERIVED from the ROUTE, never declared in YAML — the same discipline `is_config`
        // follows, so a catalogue author cannot get it wrong or forget it.
        //
        // ⚠️ NOT derived from `requires:` being non-empty, which was the first idea and is
        // wrong: `astral` requires Bun AND Claude Code, but a binary package could equally
        // require Node. `requires` says "this needs something"; the ROUTE says "this installs
        // INTO something". The route is the fact, `requires` is its consequence.
        let mut p = pkg("Chiron");
        p.claude_plugin = Some("chiron@tekton".into());
        assert!(
            is_extension_route(commands_for(&p, Os::Darwin).route.as_deref()),
            "a claude-plugin installs into an agent, not onto the machine"
        );
        let mut s = pkg("Rust best practices");
        s.skill = Some("owner/repo".into());
        assert!(is_extension_route(
            commands_for(&s, Os::Darwin).route.as_deref()
        ));
        // A binary is not: it writes to Program Files / /Applications and may elevate.
        let mut q = pkg("Git");
        q.brew = Some("git".into());
        assert!(!is_extension_route(
            commands_for(&q, Os::Darwin).route.as_deref()
        ));
        // Nor is a config-atom — it is the rung BELOW, and the two classes must not overlap
        // (a config-atom has a `check:` and no manager route; an extension has neither).
        let mut c = pkg("Starship config");
        c.run = Some("nu patch.nu".into());
        c.check = Some("test -f x".into());
        assert!(!is_extension_route(
            commands_for(&c, Os::Darwin).route.as_deref()
        ));
    }

    #[test]
    fn a_vscode_extension_installs_with_force_and_uninstalls_without() {
        let mut p = pkg("Nushell language support");
        p.vscode_extension = Some("thenuprojectcontributors.vscode-nushell-lang".into());
        let c = commands_for(&p, Os::Darwin);
        assert_eq!(c.route.as_deref(), Some("vscode-extension"));
        let install = c.install.expect("an install command");
        // ⭐ --force is NOT optional. MEASURED: without it, `code --install-extension` on an
        // already-present extension LOGS "already installed" and exits 0 having done nothing —
        // a silent no-op that convergence logic reads as success. It is also the documented
        // flag that avoids prompts, which a display-only terminal requires (a command that
        // PROMPTS deadlocks the step).
        assert!(
            install.contains("--install-extension thenuprojectcontributors.vscode-nushell-lang")
                && install.contains("--force"),
            "install: {install}"
        );
        let uninstall = c.uninstall.expect("an uninstall command");
        assert!(
            uninstall.contains("--uninstall-extension"),
            "uninstall: {uninstall}"
        );
        assert!(
            !uninstall.contains("--force"),
            "nothing to force away: {uninstall}"
        );
        // ⬜ No upgrade: `scan_outdated` asks only winget/brew, so `outdated` is structurally
        // false for this route. Wiring one would make it unreachable — the mistake the
        // claude-plugin route made, whose `upgrade:` has never once been chosen.
        //
        // ⚠️ That closes only the `outdated` road to Upgrade. The PIN road is closed separately,
        // in `load_from_catalog` — see `an_extension_route_carries_no_pin_because_it_cannot_honour_one`.
        assert_eq!(c.upgrade, None, "an unreachable command is worse than none");
        assert_eq!(c.downgrade, None);
    }

    #[test]
    fn a_vscode_extension_ignores_a_pin_rather_than_building_a_command_that_fails() {
        // MEASURED: `code --install-extension id@0.21.1` exits 1 when the gallery no longer
        // serves that version — and the gallery serves ONE version for many extensions (checked:
        // 1 available version for even-better-toml). So a pin cannot be honoured, and a pinned
        // command would be a command built to fail.
        let mut p = pkg("Nushell language support");
        p.vscode_extension = Some("thenuprojectcontributors.vscode-nushell-lang".into());
        p.version = Some("2.0.4".into());
        let c = commands_for(&p, Os::Darwin);
        let install = c.install.expect("an install command");
        assert!(
            !install.contains("2.0.4"),
            "a pin must not reach the command: {install}"
        );
        assert!(
            !install.contains('@'),
            "no version suffix at all: {install}"
        );
    }

    #[test]
    fn a_vscode_extension_is_an_extension_and_not_a_config_atom() {
        let mut p = pkg("Nushell language support");
        p.vscode_extension = Some("thenuprojectcontributors.vscode-nushell-lang".into());
        let route = commands_for(&p, Os::Darwin).route;
        // 🧩, not ⚡: it DOWNLOADS from the gallery (measured 4.3 s for a fresh install), so the
        // "no network at all" promise of rung 0 does not hold. It never elevates and writes only
        // inside the user's profile, which is what rung 1 promises.
        assert!(is_extension_route(route.as_deref()), "route {route:?}");
    }

    #[test]
    fn a_system_route_still_wins_over_a_vscode_extension_declaration() {
        // Route arbitration is ORDERED, and the system manager is family 1. A package declaring
        // both must not silently become an extension — the rung would then promise "never
        // elevates" about a winget install that can.
        let mut p = pkg("Confused");
        p.brew = Some("git".into());
        p.vscode_extension = Some("some.ext".into());
        assert_eq!(commands_for(&p, Os::Darwin).route.as_deref(), Some("brew"));

        // And the boundary on the other side: the branch is LAST, so any other route also wins.
        // Not a safety property like the system-manager case above — but the comment claims
        // "last", and an unpinned claim drifts.
        let mut n = pkg("Also confused");
        n.npm = Some("some-pkg".into());
        n.vscode_extension = Some("some.ext".into());
        assert_eq!(commands_for(&n, Os::Darwin).route.as_deref(), Some("npm"));
    }

    #[test]
    fn an_extension_route_carries_no_pin_because_it_cannot_honour_one() {
        // ⚠️ `action_for` consults the pin BEFORE `outdated` (decision.rs:90), and no
        // extension route builds an `upgrade:` command. A carried pin would therefore produce
        // Action::Upgrade with nothing to run — `do_step` returns a default outcome and the
        // row is SKIPPED IN SILENCE, which is the one thing this codebase refuses.
        //
        // Measured for the vscode route: `code --install-extension id@<version>` exits 1 once
        // the gallery drops that version, so the pin is not merely unsupported — it is
        // unmeanable.
        let plan = load_from_catalog(
            concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/bundles"),
            Os::Darwin,
            &|_| {},
        );
        for s in &plan.steps {
            if s.is_extension {
                assert!(
                    s.pin.is_none(),
                    "{}: an extension route carries a pin it cannot honour",
                    s.name
                );
            }
        }
        // And a synthetic package, so the guard does not depend on what the catalogue happens
        // to ship today.
        let mut p = pkg("Pinned extension");
        p.vscode_extension = Some("some.ext".into());
        p.version = Some("1.2.3".into());
        let c = commands_for(&p, Os::Darwin);
        assert!(is_extension_route(c.route.as_deref()));
        assert_eq!(c.upgrade, None, "no upgrade command exists to honour a pin");
    }

    /// The five members of the class in the SHIPPED catalogue, named on purpose.
    ///
    /// A content-shaped assertion, deliberately: naming them means DELETING a plugin turns
    /// this red, which a "some package is an extension" check would sail past. Same reasoning
    /// as `the_shipped_seed_reaches_overrides`. Adding a sixth extension does not fail it —
    /// nothing here asserts a total.
    #[test]
    fn the_shipped_extensions_are_the_content_routes() {
        let cat = crate::catalog::load_catalog(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"));
        for (id, expected) in [
            ("chiron", true),
            ("astral", true),
            ("jj-skills", true),
            ("nushell-dev", true),
            ("rust-best-practices", true),
            ("vscode-nushell-lang", true),
            // The controls: a binary, and a config-atom — the rungs on either side.
            ("git", false),
            ("starship-config", false),
        ] {
            let cp = cat
                .values()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("{id} is shipped"));
            let route = commands_for(&cp.pkg, Os::Darwin).route;
            assert_eq!(
                is_extension_route(route.as_deref()),
                expected,
                "{id}: route {route:?}"
            );
        }
    }

    #[test]
    fn the_shipped_vscode_extension_detects_by_its_id_without_declaring_one() {
        let plan = load_from_catalog(
            concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/bundles"),
            Os::Darwin,
            &|_| {},
        );
        let s = plan
            .steps
            .iter()
            .find(|s| s.id == "vscode-nushell-lang")
            .expect("the shipped example must be there");
        assert_eq!(s.route.as_deref(), Some("vscode-extension"));
        assert!(s.is_extension, "🧩, so an Apply at ⚡ must not fetch it");
        assert!(!s.is_config);
        // The id reaches `detect` even though the YAML declares no `detect:` — that fallback is
        // what lets the catalogue author avoid writing the id twice.
        assert_eq!(
            s.detect.as_deref(),
            Some("thenuprojectcontributors.vscode-nushell-lang")
        );
        assert!(
            s.requires.iter().any(|r| r == "Visual Studio Code"),
            "the host must be declared so topo_sort orders it first: {:?}",
            s.requires
        );
    }

    fn pkg(name: &str) -> RawPkg {
        RawPkg {
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_claude_plugin_install_also_updates() {
        // MEASURED: `claude plugin install <id>` on an already-installed plugin answers
        // "already installed" and changes nothing. So pointing a package at a corrected fork
        // switches the SOURCE while leaving the OLD plugin active — Talos would show a green
        // row over a stale plugin. The chained `update` is what actually lands the fix.
        //
        // Pinned because nothing else would notice its removal: the route's own tests only
        // checked `route`, and `scan_outdated` never marks a plugin outdated (it asks the
        // native manager only), so `action_for` can never reach the `upgrade` command either.
        let mut p = pkg("jj skills");
        p.claude_plugin = Some("jj@claude-plugin-jj".into());
        p.marketplace = Some("ChristianLemer/claude-plugin-jj".into());
        let c = commands_for(&p, Os::Windows);
        assert_eq!(c.route.as_deref(), Some("claude-plugin"));
        let install = c.install.as_deref().expect("an install command");
        assert!(
            install.contains("marketplace add \"ChristianLemer/claude-plugin-jj\""),
            "the fork must be registered first: {install}"
        );
        assert!(
            install.contains("plugin install jj@claude-plugin-jj"),
            "…then installed: {install}"
        );
        assert!(
            install.contains("&& claude plugin update jj@claude-plugin-jj"),
            "…and UPDATED, or an already-installed plugin never moves: {install}"
        );
        // ⭐ A `marketplace add` shells out to git, and claude's failure message reads as
        // "no git" when it actually means "every git found lives under the cwd". So the
        // row must show the PAIR that produces that verdict — the cwd and the candidates —
        // plus whether the clone will go over https (the thing that keeps the ssh agent,
        // and 1Password, out of a public clone).
        //
        // ⚠️ These two assertions replaced a pair that looked for a `url.*.insteadOf`
        // rewrite. That suspect was measured on the machine and refuted; keeping its test
        // would have kept the wrong question alive.
        assert!(
            install.contains("where.exe git") || install.contains("command -v git"),
            "the row must SHOW the git candidates: {install}"
        );
        assert!(
            install.contains("CLAUDE_CODE_PLUGIN_PREFER_HTTPS"),
            "…and whether the clone goes over https rather than ssh: {install}"
        );
        assert!(
            install.contains("cwd") || install.contains("$PWD") || install.contains("pwd"),
            "…and the cwd it is judged against, which is what decides: {install}"
        );
        // ⚠️ Every added command must be non-interactive and must NOT be able to fail the
        // chain: a diagnostic that breaks the install it was added to debug is worse than
        // no diagnostic (memory display-only-terminal — a prompting command deadlocks).
        assert!(
            !install.contains("where.exe git &&"),
            "the diagnostic must not gate the install with &&: {install}"
        );
        // ⭐ And it must PARSE. Everything above checks that commands are PRESENT; none of
        // it would have caught the defect that actually shipped — three labels written
        // without quotes, so PowerShell parsed each parenthesis as a sub-expression and
        // executed it. The chain died at its first line with `InvalidLeftHandSide`, exit
        // 1, before `marketplace add`, which is why beta.23's and beta.24's fixes never
        // ran. Presence is not parseability.
        assert_shell_safe(install, "the plugin install chain");

        // A package with no marketplace still installs and updates — the `add` is what is
        // optional, not the update.
        let mut q = pkg("some plugin");
        q.claude_plugin = Some("x@y".into());
        let d = commands_for(&q, Os::Windows).install.unwrap();
        assert!(
            !d.contains("marketplace add"),
            "no marketplace declared: {d}"
        );
        assert!(
            d.contains("&& claude plugin update x@y"),
            "still updates: {d}"
        );
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

    /// Every command the SHIPPED catalogue produces, on every OS, must parse.
    ///
    /// ⭐ This is the coverage the per-package test above cannot give. That one builds a
    /// `pkg()` by hand, so it proves the labels THIS code writes are safe — and says
    /// nothing about a `run:` or `check:` an author adds tomorrow, which goes through the
    /// same shell wrapping and fails the same way: at Apply time, on Windows only, with a
    /// parser error that reads like a broken package rather than a broken label.
    ///
    /// ⚠️ Passes trivially today — no shipped command carries a parenthesis. That is
    /// precisely why it goes in NOW rather than after the next incident: a tripwire has to
    /// exist before the file that trips it, and one that costs nothing while green is the
    /// cheapest kind there is. It is also the repo's own idiom (`catalog.rs` and the npm
    /// route already assert against the shipped catalogue, not fixtures).
    #[test]
    fn every_shipped_command_parses_on_every_os() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog");
        let mut checked = 0usize;
        for e in std::fs::read_dir(&dir).expect("the shipped catalogue must be readable") {
            let path = e.expect("readable entry").path();
            // The two `.nu` sidecars are FILES, not command lines — the command that
            // invokes them is what this checks, and it lives in the yaml.
            if path.extension().and_then(|x| x.to_str()) != Some("yaml") {
                continue;
            }
            let id = path.file_stem().unwrap().to_string_lossy().to_string();
            let raw = std::fs::read_to_string(&path).expect("readable");
            let Some(cp) = crate::catalog::parse_catalog_entry(&raw, &id) else {
                continue;
            };
            for os in [Os::Windows, Os::Darwin, Os::Linux] {
                let c = commands_for(&cp.pkg, os);
                for (verb, cmd) in [
                    ("install", &c.install),
                    ("uninstall", &c.uninstall),
                    ("upgrade", &c.upgrade),
                    ("downgrade", &c.downgrade),
                ] {
                    if let Some(cmd) = cmd {
                        checked += 1;
                        assert_shell_safe(cmd, &format!("{id} {os:?} {verb}"));
                    }
                }
            }
            // `check:` is wrapped by the same `shell_probe` and is the OTHER command an
            // author writes by hand, so it falls under the same rule.
            if let Some(chk) = cp.pkg.check.as_deref() {
                checked += 1;
                assert_shell_safe(chk, &format!("{id} check"));
            }
        }
        // ⚠️ Without this, a renamed folder or a parse that starts returning None turns
        // the whole test into a green no-op — the classic way a shipped-catalogue guard
        // stops guarding while still reporting success.
        assert!(
            checked > 30,
            "only {checked} shipped commands checked — the catalogue was not really read"
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

        // Observed elevating on Windows via winget. The first four are C's, the last two
        // came from a colleague's real Windows machine on 2026-08-04 — the first field data the
        // telemetry ever produced, and the reason this list is expected to grow.
        for stem in [
            // C's, from her own field use
            "7-zip",
            "aws-cli",
            "node",
            "visual-studio-code",
            // reported by a colleague, 2026-08-04
            "starship",
            "notepad-plus-plus",
            // promoted from behaviour/ — the fleet found these before anyone declared them
            "git",
            "greenshot",
            "nushell",
        ] {
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
        // And the ones a corporate firewall answers 403 on. rclone is C's; jj and ripgrep came
        // from the colleague's machine on 2026-08-04.
        //
        // ⚠️ `403` is a NETWORK fact, not a Windows one — `forbidden::is403` has no platform
        // gate, so a Mac behind the same firewall collects it too. Do not "fix" these into a
        // Windows-only list.
        for stem in ["rclone", "jj", "ripgrep", "bat", "uv"] {
            let o = declared(stem);
            assert_eq!(o.forbidden, Some(true), "{stem} must declare 403: true");
            assert_eq!(o.uac, None, "{stem} says nothing about elevation");
        }

        // A package with no declaration must stay silent — otherwise the seed is not a
        // seed but a default, and "no opinion" would have collapsed into "false".
        // The control must declare NOTHING, and it keeps moving as facts get promoted: jq
        // became a fixture's subject, then bat gained a 403 from the fleet. `marktext` is the
        // current choice — a package no report and no machine has flagged. When it too gets
        // promoted, move this rather than deleting it: without a control, "no opinion" could
        // silently collapse into "false" and nothing would notice.
        assert_eq!(declared("marktext").uac, None);
        assert_eq!(declared("marktext").forbidden, None);
        assert_eq!(declared("marktext").slow, None);
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

    #[test]
    fn a_declared_version_is_classified_once_and_only_once() {
        // ⭐ ONE classification, because the value has TWO consumers with opposite needs: the
        // command builder must NEVER see a keyword (it would emit `brew install git@pending`),
        // while `action_for` MUST see it (it decides whether to act at all). Two readings of the
        // same string is how they would come to disagree.
        assert_eq!(classify_pin(None), None, "nothing declared is not a pin");
        assert_eq!(
            classify_pin(Some("")),
            None,
            "an empty value is nothing declared"
        );
        assert_eq!(
            classify_pin(Some("   ")),
            None,
            "whitespace is nothing declared"
        );
        assert_eq!(
            classify_pin(Some("0.113.1")),
            Some(PinKind::Exact("0.113.1".into()))
        );
        assert_eq!(
            classify_pin(Some(" 1.10 ")),
            Some(PinKind::Exact("1.10".into())),
            "trimmed"
        );
        assert_eq!(classify_pin(Some("latest")), Some(PinKind::Latest));
        assert_eq!(classify_pin(Some("pending")), Some(PinKind::Pending));
        // Case-insensitive: a maintainer typing `Pending` means the keyword, not a version.
        assert_eq!(classify_pin(Some("Pending")), Some(PinKind::Pending));
        assert_eq!(classify_pin(Some("LATEST")), Some(PinKind::Latest));
    }

    #[test]
    fn an_unknown_word_is_invalid_never_a_version() {
        // ⚠️ THE LOAD-BEARING CASE. MEASURED: `compare_versions("2.50.1", "current") == 1` and
        // `compare_versions("0.0.0", "current") == 0` — a word's leading digits are absent, so it
        // reads as 0.0.0, `action_for` concludes "installed is ABOVE the pin" and returns
        // Downgrade. That surfaces the manual downgrade button, whose command is
        // `uninstall && install pkg@current` — the only destructive path in the app, reachable by
        // writing one innocent word.
        //
        // ⚠️ AND WORSE: `bundles.rs`'s `ver` binding feeds `install_pinned`, so the word also
        // reaches `brew install --yes git@current` / `winget install --version current` — a
        // command built to fail, on install AND upgrade AND downgrade.
        assert_eq!(
            classify_pin(Some("current")),
            Some(PinKind::Invalid("current".into()))
        );
        assert_eq!(
            classify_pin(Some("stable")),
            Some(PinKind::Invalid("stable".into()))
        );
        assert_eq!(
            classify_pin(Some("pendign")),
            Some(PinKind::Invalid("pendign".into())),
            "a typo must be caught, not silently treated as 0.0.0"
        );
    }

    #[test]
    fn a_version_that_merely_starts_with_a_digit_is_exact() {
        // The discriminator is "does it start with a digit", not a semver parse: this codebase
        // deliberately does NOT do full semver (compare_versions takes numeric prefixes), and
        // real catalogue versions include shapes like `2026.72.0` and `1.0-beta`.
        assert_eq!(
            classify_pin(Some("2026.72.0")),
            Some(PinKind::Exact("2026.72.0".into()))
        );
        assert_eq!(
            classify_pin(Some("1.0-beta")),
            Some(PinKind::Exact("1.0-beta".into()))
        );
        assert_eq!(classify_pin(Some("7")), Some(PinKind::Exact("7".into())));
    }

    #[test]
    fn a_keyword_never_reaches_an_install_command() {
        // ⚠️ MEASURED BEFORE THE FIX: `bundles.rs`'s `if !ver.is_empty()` sent any non-empty word
        // down the PINNED path, so `install_pinned` produced, verbatim:
        //     brew install --yes git@pending
        //     winget install --id Git.Git -e --version pending …
        // A command built to fail, on install AND upgrade AND downgrade. The keyword is a
        // statement about POLICY, not about which version to fetch, so it must be stripped before
        // any command is built.
        for word in ["pending", "latest", "Pending"] {
            let mut p = pkg("Git");
            p.brew = Some("git".into());
            p.version = Some(word.to_string());
            let c = commands_for(&p, Os::Darwin);
            let install = c.install.expect("an install command");
            assert!(
                !install.contains(word) && !install.contains('@'),
                "{word} reached the command: {install}"
            );
            assert_eq!(
                install, "brew install --yes git",
                "the plain, unpinned install"
            );
            // The pinned path also produces a `downgrade`; an unpinned package has none.
            assert_eq!(
                c.downgrade, None,
                "{word} must not fabricate a downgrade path"
            );
        }
    }

    #[test]
    fn an_invalid_word_never_reaches_a_command_either() {
        // An unknown word is refused (Task 4 logs it), but `commands_for` must not depend on that
        // refusal: defence at the point of use, so a future caller cannot bypass the load-time check.
        let mut p = pkg("Git");
        p.brew = Some("git".into());
        p.version = Some("current".into());
        let install = commands_for(&p, Os::Darwin)
            .install
            .expect("an install command");
        assert_eq!(install, "brew install --yes git", "no @current: {install}");
    }

    #[test]
    fn an_exact_version_still_pins_the_command() {
        // The guard must not break the feature it guards. This is the existing behaviour, pinned
        // here because the `ver` binding is what Task 3 changes.
        let mut p = pkg("Nushell");
        p.brew = Some("nushell".into());
        p.version = Some("0.113.1".into());
        let c = commands_for(&p, Os::Darwin);
        let install = c.install.expect("an install command");
        assert!(install.contains("nushell@0.113.1"), "install: {install}");
        assert!(
            c.downgrade.is_some(),
            "an exact pin keeps its manual downgrade path"
        );
    }
}
