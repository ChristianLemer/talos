// Package dependencies (`requires:`) over the FUTURE state.
// A requirement is not "is X here now?" but "will X be here after the Apply?".
// GLOBAL view (all packages) that per-step detection cannot have.
use std::collections::{HashMap, HashSet};

/// One `requires:` entry, split: the package NAME, and the lowest version of it that
/// satisfies, when the author wrote one (`Orca >= 1.4.252`).
///
/// ⭐ Only `>=`. A requirement says "this must be here and at least this new" — the one
/// fact a dependent can need from its host (a command that appeared in 1.4.252). No
/// ranges, no `<`: nothing in the corpus asks for one, and a ceiling would be a pin on
/// ANOTHER row, which is that row's own `version:` to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub name: String,
    pub min: Option<String>,
}

/// Reads one `requires:` entry. `Name`, `Name >= 1.2.3` and `Name ≥ 1.2.3` (the sign is
/// free to accept, so it is) give a name and maybe a minimum; anything else that carries a
/// comparison sign is MALFORMED and comes back `Err` with the reason, for `--check`.
///
/// The split is on the LAST sign: names carry spaces and `+` (`VC++ runtime`), never `>=`.
pub fn parse_requirement(raw: &str) -> Result<Requirement, String> {
    let raw = raw.trim();
    let split = raw
        .rfind(">=")
        .map(|at| (at, 2))
        .or_else(|| raw.rfind('≥').map(|at| (at, '≥'.len_utf8())));
    let Some((at, len)) = split else {
        if raw.contains(['<', '>', '=', '≤']) {
            return Err(format!(
                "`{raw}`: only `>=` is understood — `Name >= 1.2.3`"
            ));
        }
        return Ok(Requirement {
            name: raw.to_string(),
            min: None,
        });
    };
    let name = raw[..at].trim();
    let min = crate::decision::bare_version(&raw[at + len..]);
    if name.is_empty() || name.contains(['<', '>', '=', '≥', '≤']) {
        return Err(format!("`{raw}`: write `Name >= 1.2.3`, one sign"));
    }
    let numeric = !min.is_empty()
        && min
            .split('.')
            .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()));
    if !numeric {
        return Err(format!(
            "`{raw}`: `{min}` is not a version — digits and dots, like `1.4.252`"
        ));
    }
    Ok(Requirement {
        name: name.to_string(),
        min: Some(min.to_string()),
    })
}

/// The engine's reading of a `requires:` list: malformed entries are kept WHOLE as a name,
/// so they surface as `requires <entry> (unknown)` on the row — held, and said — rather than
/// silently satisfied. `--check` is where they are refused (`parse_requirement`'s `Err`).
pub fn read_requirements(raw: &[String]) -> Vec<Requirement> {
    raw.iter()
        .map(|r| {
            parse_requirement(r).unwrap_or_else(|_| Requirement {
                name: r.trim().to_string(),
                min: None,
            })
        })
        .collect()
}

/// `(name, lowest version)` for each requirement that sets one, ONE entry per name. Two
/// minimums on one name (`--check` refuses it) keep the HIGHER, so the engine
/// (`below_minimum` takes the first match) and the front (a map, the last key wins) can
/// never read different floors off the same list.
pub fn minimums_of(reqs: &[Requirement]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for r in reqs {
        let Some(min) = &r.min else { continue };
        match out.iter_mut().find(|(n, _)| *n == r.name) {
            Some((_, kept)) => {
                if crate::decision::compare_versions(min, kept) > 0 {
                    *kept = min.clone();
                }
            }
            None => out.push((r.name.clone(), min.clone())),
        }
    }
    out
}

/// The minimum deps reasons over. The real Steps satisfy it structurally.
#[derive(Debug, Clone, Default)]
pub struct DepNode {
    pub name: String,
    pub requires: Vec<String>,
    /// `(requirement name, lowest version)` for each requirement written `Name >= x`.
    /// Names without one are absent here: a bare name asks for presence only.
    pub minimums: Vec<(String, String)>,
    /// The version observed for THIS package right now (`Presence.version`), when it said
    /// one. Read by the dependents that set a minimum on it.
    pub version: Option<String>,
    pub will_be_present: bool, // present now OR desired-present after the Apply
    /// Observed present RIGHT NOW, before any desire is applied. `will_be_present`
    /// cannot answer for this: it folds the desire in, and the two part company on a
    /// package that is installed and unwanted. See `requires_blocked`.
    pub present_now: bool,
    /// Would Talos ever act on this row? False for a package with no practicable route
    /// on this machine — a platform probe, a winget-only entry on a Mac. Same bit the
    /// front carries as `canUninstall` and scope.js reads as `no-route`, so the two
    /// sides of the rule are the same predicate rather than two approximations.
    pub reachable: bool,
}

fn index_by_name(nodes: &[DepNode]) -> HashMap<String, usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.clone(), i))
        .collect()
}

/// Is a PRESENT requirement too old for this node? The reason, or None when it is new
/// enough or no minimum was written.
///
/// ⭐ An unknown version is UNKNOWN (`talos-unknown-not-absent`): it is neither "new enough"
/// nor "too old", and the row is held saying so, never run on a guess. The requirement
/// itself is never touched by this rule — its own row decides its upgrade.
fn below_minimum(node: &DepNode, req: &DepNode) -> Option<String> {
    let (_, min) = node.minimums.iter().find(|(n, _)| *n == req.name)?;
    match req
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        None => Some(format!("requires {} >= {min} (version unknown)", req.name)),
        Some(v) if crate::decision::compare_versions(v, min) < 0 => Some(format!(
            "requires {} >= {min} (found {})",
            req.name,
            crate::decision::bare_version(v)
        )),
        Some(_) => None,
    }
}

/// The reason a node's requirements do not hold, or None.
/// A requirement holds iff the required package exists in the plan AND will_be_present.
/// First failing requirement wins the message.
pub fn requires_reason(
    node: &DepNode,
    nodes: &[DepNode],
    idx: &HashMap<String, usize>,
) -> Option<String> {
    for req in &node.requires {
        match idx.get(req) {
            None => return Some(format!("requires {req} (unknown)")),
            Some(&at) => {
                if !nodes[at].will_be_present {
                    return Some(format!("requires {req}"));
                }
            }
        }
    }
    None
}

/// WHY this node's `requires:` make it IMPOSSIBLE here, or None — the SCOPE reading,
/// as against `requires_reason`'s Apply reading. Same wording, deliberately: the row
/// says the same thing whether the verdict came from the scan or from the Apply.
///
/// A requirement holds iff it exists in the catalogue AND (it is present now, OR it is
/// on its way in and Talos can actually put it there). Two clauses, each earned:
///
/// · PRESENT NOW comes first and unconditionally. `Windows` has no route on Windows
///   either — nothing installs an operating system — so a rule that weighed
///   reachability first would block the row on the one machine where it belongs.
///
/// · WANTED IS NOT ENOUGH. The transitive pull (model.js `wantedNames`) means wanting a
///   row pulls its requirements in, so `will_be_present` is true for almost every
///   requirement of almost every wanted row. Reading it alone answers "satisfied" for
///   the very symptom this exists to fix — a measured no-op, caught by a test.
///
/// WHY NOT one function with a flag: the two readings differ on exactly one situation —
/// a requirement present but unwanted — and there they must differ. For the Apply,
/// `off` IS the uninstall instruction, so `requires_reason` is right to call it unmet.
/// For a derivation at rest, every package is either wanted or not, so reading
/// "unwanted" as "vanishing" would make every unrequested plugin row shout
/// `requires Nushell` beside an installed nushell — noise on the majority state. Two
/// names keep that distinction visible; a boolean parameter would bury it.
///
/// DIRECT requirements only, like `requires_reason`: a chain whose middle link is
/// itself blocked reports nothing. The gap is real and is shared with the front — one
/// rule, one hole, never two behaviours.
pub fn requires_blocked(
    node: &DepNode,
    nodes: &[DepNode],
    idx: &HashMap<String, usize>,
) -> Option<String> {
    for req in &node.requires {
        match idx.get(req) {
            None => return Some(format!("requires {req} (unknown)")),
            Some(&at) => {
                let r = &nodes[at];
                if r.present_now || (r.will_be_present && r.reachable) {
                    continue;
                }
                return Some(format!("requires {req}"));
            }
        }
    }
    None
}

/// The first PRESENT requirement that is below this node's minimum for it, as the row's
/// reason, or None. Absent requirements are not judged here: `requires_blocked` /
/// `requires_reason` answer for presence, and one on its way in arrives at whatever its
/// route lays down (`--check` refuses an exact pin below the minimum).
///
/// ⭐ A minimum is a HOLD, not a scope verdict: it stops what would RUN the dependent
/// against a host too old for it, never a removal — see `held_by_minimum`.
pub fn minimum_hold(
    node: &DepNode,
    nodes: &[DepNode],
    idx: &HashMap<String, usize>,
) -> Option<String> {
    node.requires.iter().find_map(|req| {
        let r = &nodes[*idx.get(req)?];
        if r.present_now {
            below_minimum(node, r)
        } else {
            None
        }
    })
}

/// Does a minimum that does not hold stop THIS action? Install, upgrade and downgrade
/// (an update-on is an upgrade): yes. A removal: never — it does not need its requirement
/// (`drop_requiring_held` says the same), and a row whose host is too old must stay
/// removable. model.js `holdReason` is the front's twin (`HELD_ACTIONS` + `minimumReason`).
pub fn held_by_minimum(
    action: crate::decision::Action,
    node: &DepNode,
    nodes: &[DepNode],
    idx: &HashMap<String, usize>,
) -> Option<String> {
    if action == crate::decision::Action::Uninstall {
        return None;
    }
    minimum_hold(node, nodes, idx)
}

/// Orders the install plan so a required package comes BEFORE its dependents.
/// `plan` carries `i` (index into nodes). Kahn, seeding the ready nodes in
/// the GIVEN order of the plan (visual order = stable tie-break). Edges to packages
/// OUT of the plan ignored (already present). A cycle → remaining items in given order
/// (defensive: never panics, never loses an item).
pub fn topo_sort<T: Clone>(
    plan: &[(usize, T)],
    nodes: &[DepNode],
    idx: &HashMap<String, usize>,
) -> Vec<(usize, T)> {
    let in_plan: HashSet<usize> = plan.iter().map(|(i, _)| *i).collect();
    let mut remaining: HashMap<usize, usize> = HashMap::new();
    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, _) in plan {
        let mut deps = 0;
        for req in &nodes[*i].requires {
            if let Some(&at) = idx.get(req) {
                if in_plan.contains(&at) {
                    deps += 1;
                    dependents.entry(at).or_default().push(*i);
                }
            }
        }
        remaining.insert(*i, deps);
    }

    let by_index: HashMap<usize, &(usize, T)> = plan.iter().map(|p| (p.0, p)).collect();
    let mut out: Vec<(usize, T)> = Vec::with_capacity(plan.len());
    let mut emitted: HashSet<usize> = HashSet::new();

    // Given order of the plan to break ties among independents (stable visual tie-break).
    let given_order: Vec<usize> = plan.iter().map(|(i, _)| *i).collect();
    loop {
        // ready = deps satisfied, not emitted, in the given order.
        let ready: Vec<usize> = given_order
            .iter()
            .copied()
            .filter(|i| !emitted.contains(i) && remaining.get(i).copied().unwrap_or(0) == 0)
            .collect();
        let Some(&i) = ready.first() else { break };
        out.push((*by_index.get(&i).unwrap()).clone());
        emitted.insert(i);
        if let Some(deps) = dependents.get(&i) {
            for &dep in deps {
                if let Some(r) = remaining.get_mut(&dep) {
                    *r = r.saturating_sub(1);
                }
            }
        }
    }
    // Cycle / remainder not emitted → given order (defensive).
    for p in plan {
        if !emitted.contains(&p.0) {
            out.push(p.clone());
        }
    }
    out
}

/// Builds the name→pos index (exposed for callers).
pub fn make_index(nodes: &[DepNode]) -> HashMap<String, usize> {
    index_by_name(nodes)
}

/// name→pos index straight from the names, WITHOUT needing DepNodes. The scope has
/// to be known BEFORE the probes run, and a DepNode wants `will_be_present` — which
/// is exactly what the probes are for. Same map as make_index (a test pins that).
pub fn index_of_names<'a>(names: impl Iterator<Item = &'a str>) -> HashMap<String, usize> {
    names.enumerate().map(|(i, n)| (n.to_string(), i)).collect()
}
/// Which packages must be RE-PROBED before an Apply that touches `seeds`
/// (= the on ∪ off indices). Answer: the seeds, plus what their `requires` pull,
/// transitively — never less.
///
/// Directed ONE WAY: requirements, not dependents. Seeding `jj` pulls nothing extra,
/// but seeding `jj skills` pulls `jj` — because the plan's shape depends on whether
/// the requirement will be there (requires_reason, topo_sort), while a package that
/// merely *depends* on a seed is not being touched and its own presence is unchanged.
///
/// `requires[i]` = the requirement names of package i (parallel to the catalogue).
/// Unknown names are ignored (requires_reason reports them; scope can't probe a
/// package that doesn't exist). Cycles terminate — a visited set, not recursion.
pub fn rescan_scope(
    requires: &[Vec<String>],
    idx: &HashMap<String, usize>,
    seeds: &[usize],
) -> HashSet<usize> {
    let mut scope: HashSet<usize> = HashSet::new();
    let mut queue: Vec<usize> = seeds.to_vec();
    while let Some(i) = queue.pop() {
        if i >= requires.len() || !scope.insert(i) {
            continue; // out of range, or already walked (this is the cycle guard)
        }
        for req in &requires[i] {
            if let Some(&at) = idx.get(req) {
                queue.push(at);
            }
        }
    }
    scope
}

/// The packages that `requires:` the package called `name` — DIRECTLY, in catalogue order.
///
/// ⭐ The other direction from `rescan_scope`, and asked at a different moment. Before an
/// Apply, a dependent is untouched and its presence unchanged, so the scope leaves it out.
/// AFTER a step changed its requirement, that no longer holds for every family: upgrading
/// Nushell kills the plugins registered against the old one (a deleted Cellar, a binary built
/// for the previous minor), without touching them. So the Apply asks this once the step has
/// run, and re-probes what it returns.
///
/// Direct only, on purpose: it is the bound. A dependent repaired by that pass does not make
/// ITS dependents re-checked — one extra pass per Apply, never a cascade.
pub fn dependents_of(requires: &[Vec<String>], name: &str) -> Vec<usize> {
    requires
        .iter()
        .enumerate()
        .filter(|(_, reqs)| reqs.iter().any(|r| r == name))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, requires: &[&str], present: bool) -> DepNode {
        DepNode {
            name: name.into(),
            requires: requires.iter().map(|s| s.to_string()).collect(),
            will_be_present: present,
            present_now: present,
            reachable: true,
            ..Default::default()
        }
    }

    /// A node spelled out on all three axes, for the blocked-reading tests.
    fn dep(
        name: &str,
        requires: &[&str],
        will_be_present: bool,
        present_now: bool,
        reachable: bool,
    ) -> DepNode {
        DepNode {
            name: name.into(),
            requires: requires.iter().map(|s| s.to_string()).collect(),
            will_be_present,
            present_now,
            reachable,
            ..Default::default()
        }
    }

    #[test]
    fn blocked_when_the_requirement_is_absent_and_unreachable() {
        // The measured case (2026-08-21): `VC++ runtime` requires `Windows`, on a Mac.
        // `Windows` is a probe-only package — `detect: cmd /c ver`, no route — so it is
        // absent AND can never be laid down. The dependent must leave the perimeter.
        let nodes = vec![
            dep("VC++ runtime", &["Windows"], true, false, true),
            dep("Windows", &[], false, false, false),
        ];
        let idx = make_index(&nodes);
        assert_eq!(
            requires_blocked(&nodes[0], &nodes, &idx),
            Some("requires Windows".into())
        );
    }

    #[test]
    fn a_present_requirement_holds_even_when_it_is_unreachable() {
        // ⭐ The half that keeps Windows working. `Windows` has no route THERE either —
        // `reachable` is false on every platform, because nothing installs an OS — so a
        // rule reading reachability alone would block the row on the one machine where
        // it belongs. Presence is checked FIRST and unconditionally.
        let nodes = vec![
            dep("VC++ runtime", &["Windows"], true, false, true),
            dep("Windows", &[], true, true, false),
        ];
        let idx = make_index(&nodes);
        assert_eq!(requires_blocked(&nodes[0], &nodes, &idx), None);
    }

    #[test]
    fn an_absent_requirement_on_its_way_in_holds() {
        // A requirement installed in the SAME Apply must not block its dependent, or a
        // bundle could never lay down a package and its plumbing in one gesture.
        let nodes = vec![
            dep("Polars", &["Nushell"], true, false, true),
            dep("Nushell", &[], true, false, true),
        ];
        let idx = make_index(&nodes);
        assert_eq!(requires_blocked(&nodes[0], &nodes, &idx), None);
    }

    #[test]
    fn wanting_a_requirement_is_not_enough_when_it_cannot_be_installed() {
        // ⭐ THE trap that made the first version of this rule vacuous, front and back.
        // The transitive pull means wanting a row pulls its requirements in, so
        // `will_be_present` is true for essentially every requirement of every wanted
        // row. Reading it ALONE would have answered "satisfied" for the measured
        // symptom and shipped a no-op. Reachability is the discriminating half.
        let nodes = vec![
            dep("VC++ runtime", &["Windows"], true, false, true),
            dep("Windows", &[], true, false, false), // pulled in, but nothing installs an OS
        ];
        let idx = make_index(&nodes);
        assert_eq!(
            requires_blocked(&nodes[0], &nodes, &idx),
            Some("requires Windows".into())
        );
    }

    #[test]
    fn blocked_names_a_dangling_requirement() {
        let nodes = vec![dep("a", &["ghost"], true, false, true)];
        let idx = make_index(&nodes);
        assert_eq!(
            requires_blocked(&nodes[0], &nodes, &idx),
            Some("requires ghost (unknown)".into())
        );
    }

    #[test]
    fn blocked_and_reason_read_the_same_situation_differently() {
        // The two functions exist side by side ON PURPOSE, and this pins the ONE case
        // where they part company: a requirement PRESENT but wanted-off.
        //   · requires_reason  → unmet. Correct for the Apply, where `off` IS the
        //                        uninstall instruction: it really is about to go.
        //   · requires_blocked → holds. Correct for a derivation AT REST: every package
        //                        is either wanted or not, so reading "unwanted" as
        //                        "vanishing" would make every unrequested plugin row
        //                        shout `requires Nushell` beside an installed nushell.
        // Collapsing them into one function with a flag would hide exactly this.
        let nodes = vec![
            dep("Polars", &["Nushell"], true, false, true),
            dep("Nushell", &[], false, true, true), // installed, and nothing wants it
        ];
        let idx = make_index(&nodes);
        assert_eq!(
            requires_reason(&nodes[0], &nodes, &idx),
            Some("requires Nushell".into())
        );
        assert_eq!(requires_blocked(&nodes[0], &nodes, &idx), None);
    }

    #[test]
    fn blocked_reports_the_first_failing_requirement() {
        // Same convention as requires_reason — one message, the first that fails, so the
        // row's single label is deterministic rather than whichever the map yielded.
        let nodes = vec![
            dep("a", &["b", "c"], true, false, true),
            dep("b", &[], false, false, false),
            dep("c", &[], false, false, false),
        ];
        let idx = make_index(&nodes);
        assert_eq!(
            requires_blocked(&nodes[0], &nodes, &idx),
            Some("requires b".into())
        );
    }

    #[test]
    fn reason_unmet_requirement() {
        let nodes = vec![node("a", &["b"], true), node("b", &[], false)];
        let idx = make_index(&nodes);
        assert_eq!(
            requires_reason(&nodes[0], &nodes, &idx),
            Some("requires b".into())
        );
    }

    #[test]
    fn reason_unknown_requirement() {
        let nodes = vec![node("a", &["ghost"], true)];
        let idx = make_index(&nodes);
        assert_eq!(
            requires_reason(&nodes[0], &nodes, &idx),
            Some("requires ghost (unknown)".into())
        );
    }

    #[test]
    fn reason_none_when_satisfied() {
        let nodes = vec![node("a", &["b"], true), node("b", &[], true)];
        let idx = make_index(&nodes);
        assert_eq!(requires_reason(&nodes[0], &nodes, &idx), None);
    }

    #[test]
    fn scope_is_the_seeds_when_nothing_requires() {
        let reqs = vec![vec![], vec![], vec![]];
        let idx = index_of_names(["a", "b", "c"].into_iter());
        assert_eq!(rescan_scope(&reqs, &idx, &[1]), HashSet::from([1]));
    }

    #[test]
    fn scope_pulls_requirements_transitively() {
        // a requires b, b requires c → seeding a must pull all three.
        let reqs = vec![vec!["b".into()], vec!["c".into()], vec![]];
        let idx = index_of_names(["a", "b", "c"].into_iter());
        assert_eq!(rescan_scope(&reqs, &idx, &[0]), HashSet::from([0, 1, 2]));
    }

    #[test]
    fn scope_does_not_pull_dependents() {
        // a requires b. Seeding b must NOT drag a in: nothing about a changed.
        let reqs = vec![vec!["b".into()], vec![]];
        let idx = index_of_names(["a", "b"].into_iter());
        assert_eq!(rescan_scope(&reqs, &idx, &[1]), HashSet::from([1]));
    }

    #[test]
    fn dependents_are_the_direct_requirers_in_catalogue_order() {
        // polars and xlsx require Nushell; a plugin of polars requires polars only.
        let reqs = vec![
            vec!["Nushell".into()],
            vec![],
            vec!["Polars".into()],
            vec!["Git".into(), "Nushell".into()],
        ];
        assert_eq!(dependents_of(&reqs, "Nushell"), vec![0, 3]);
        // ⭐ Direct only: Polars's own dependent is not Nushell's — that is the bound.
        assert_eq!(dependents_of(&reqs, "Polars"), vec![2]);
        assert!(dependents_of(&reqs, "Nobody").is_empty());
    }

    #[test]
    fn scope_always_contains_every_seed() {
        // THE invariant the scoped Apply rests on: every row that gets an action is a
        // seed, so every row whose presence the decision READS has been re-probed. A
        // seed dropped from the scope would mean deciding on a remembered verdict.
        let reqs = vec![vec![], vec!["a".into()], vec![]];
        let idx = index_of_names(["a", "b", "c"].into_iter());
        let seeds = [2usize, 0, 1];
        let scope = rescan_scope(&reqs, &idx, &seeds);
        for s in seeds {
            assert!(scope.contains(&s), "seed {s} missing from the scope");
        }
    }

    #[test]
    fn scope_ignores_out_of_range_seeds() {
        // A stale index from the front must not panic the Apply, nor smuggle in a row.
        let reqs = vec![vec![]];
        let idx = index_of_names(["a"].into_iter());
        assert_eq!(rescan_scope(&reqs, &idx, &[0, 99]), HashSet::from([0]));
    }

    #[test]
    fn scope_ignores_unknown_requirements() {
        let reqs = vec![vec!["ghost".into()]];
        let idx = index_of_names(["a"].into_iter());
        assert_eq!(rescan_scope(&reqs, &idx, &[0]), HashSet::from([0]));
    }

    #[test]
    fn scope_terminates_on_a_cycle() {
        let reqs = vec![vec!["b".into()], vec!["a".into()]];
        let idx = index_of_names(["a", "b"].into_iter());
        assert_eq!(rescan_scope(&reqs, &idx, &[0]), HashSet::from([0, 1]));
    }

    #[test]
    fn scope_unions_several_seeds() {
        let reqs = vec![vec!["c".into()], vec![], vec![]];
        let idx = index_of_names(["a", "b", "c"].into_iter());
        assert_eq!(rescan_scope(&reqs, &idx, &[0, 1]), HashSet::from([0, 1, 2]));
    }

    #[test]
    fn index_of_names_matches_make_index() {
        let nodes = vec![node("a", &[], true), node("b", &[], true)];
        assert_eq!(
            index_of_names(nodes.iter().map(|n| n.name.as_str())),
            make_index(&nodes)
        );
    }

    #[test]
    fn topo_required_before_dependent() {
        // node 0 "app" requires "lib" (node 1). Given plan [0,1] → must output [1,0].
        let nodes = vec![node("app", &["lib"], true), node("lib", &[], true)];
        let idx = make_index(&nodes);
        let plan = vec![(0usize, "app"), (1usize, "lib")];
        let out = topo_sort(&plan, &nodes, &idx);
        assert_eq!(out.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![1, 0]);
    }

    #[test]
    fn topo_independents_keep_visual_order() {
        let nodes = vec![node("a", &[], true), node("b", &[], true)];
        let idx = make_index(&nodes);
        let plan = vec![(0usize, "a"), (1usize, "b")];
        let out = topo_sort(&plan, &nodes, &idx);
        assert_eq!(out.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1]);
    }

    // --- `requires: [Name >= x]` ------------------------------------------------------

    #[test]
    fn parse_reads_a_bare_name_and_both_signs() {
        let bare = parse_requirement("VC++ runtime").unwrap();
        assert_eq!((bare.name.as_str(), bare.min), ("VC++ runtime", None));
        for raw in [
            "Orca >= 1.4.252",
            "Orca>=1.4.252",
            "Orca ≥ 1.4.252",
            " Orca >= v1.4.252 ",
        ] {
            let r = parse_requirement(raw).unwrap();
            assert_eq!(r.name, "Orca", "{raw}");
            assert_eq!(r.min.as_deref(), Some("1.4.252"), "{raw}");
        }
    }

    #[test]
    fn parse_refuses_every_other_comparison_and_a_non_version() {
        for raw in [
            "Orca > 1.4",
            "Orca < 2",
            "Orca = 1.4",
            "Orca <= 1.4",
            "Orca ≤ 1.4",
            ">= 1.4",
            "Orca >= ",
            "Orca >= latest",
            "Orca >= 1.4.x",
            "Orca >= 1..4",
            "Orca >= 1.4 >= 1.5",
        ] {
            assert!(parse_requirement(raw).is_err(), "{raw} should be refused");
        }
    }

    #[test]
    fn the_engine_keeps_a_malformed_entry_whole_so_the_row_says_it() {
        let reqs = read_requirements(&["Orca > 1.4".into(), "Nushell".into()]);
        assert_eq!(reqs[0].name, "Orca > 1.4");
        assert_eq!(reqs[0].min, None);
        assert_eq!(reqs[1].name, "Nushell");
    }

    /// `plugin` requires `Orca >= 1.4.252`; Orca is present at `found`.
    fn with_minimum(found: Option<&str>) -> Vec<DepNode> {
        let mut nodes = vec![
            dep("plugin", &["Orca"], true, false, true),
            dep("Orca", &[], true, true, true),
        ];
        nodes[0].minimums = vec![("Orca".into(), "1.4.252".into())];
        nodes[1].version = found.map(String::from);
        nodes
    }

    #[test]
    fn a_present_requirement_below_the_minimum_holds_the_row_naming_both_versions() {
        let nodes = with_minimum(Some("v1.4.237"));
        let idx = make_index(&nodes);
        let want = Some("requires Orca >= 1.4.252 (found 1.4.237)".to_string());
        assert_eq!(minimum_hold(&nodes[0], &nodes, &idx), want);
        // A HOLD, not a scope verdict: the presence readings do not see it.
        assert_eq!(requires_blocked(&nodes[0], &nodes, &idx), None);
        assert_eq!(requires_reason(&nodes[0], &nodes, &idx), None);
    }

    #[test]
    fn at_or_above_the_minimum_the_requirement_holds() {
        for found in ["1.4.252", "1.4.300", "1.10.0", "2"] {
            let nodes = with_minimum(Some(found));
            let idx = make_index(&nodes);
            assert_eq!(minimum_hold(&nodes[0], &nodes, &idx), None, "{found}");
        }
    }

    #[test]
    fn an_unknown_version_is_held_never_guessed() {
        for found in [None, Some(""), Some("  ")] {
            let nodes = with_minimum(found);
            let idx = make_index(&nodes);
            let want = Some("requires Orca >= 1.4.252 (version unknown)".to_string());
            assert_eq!(minimum_hold(&nodes[0], &nodes, &idx), want, "{found:?}");
        }
    }

    #[test]
    fn an_absent_requirement_on_its_way_in_is_not_second_guessed() {
        // Absent, wanted, reachable: it arrives at whatever its route lays down. The minimum
        // does not hold the dependent on a version nobody has observed yet.
        let mut nodes = with_minimum(None);
        nodes[1].present_now = false;
        let idx = make_index(&nodes);
        assert_eq!(minimum_hold(&nodes[0], &nodes, &idx), None);
        assert_eq!(requires_blocked(&nodes[0], &nodes, &idx), None);
    }

    #[test]
    fn a_minimum_only_reads_its_own_requirement() {
        // A second, bare requirement present at any version is untouched by Orca's minimum.
        let mut nodes = with_minimum(Some("1.4.300"));
        nodes[0].requires.push("Nushell".into());
        nodes.push(dep("Nushell", &[], true, true, true));
        let idx = make_index(&nodes);
        assert_eq!(minimum_hold(&nodes[0], &nodes, &idx), None);
    }

    #[test]
    fn a_minimum_holds_every_action_but_a_removal() {
        // The pre-tag finding: Orca below the floor (or unreadable) must not make a plugin
        // that requires it impossible to REMOVE. A removal never needs its requirement.
        use crate::decision::Action;
        for found in [Some("1.4.237"), None] {
            let nodes = with_minimum(found);
            let idx = make_index(&nodes);
            for a in [Action::Install, Action::Upgrade, Action::Downgrade] {
                assert!(
                    held_by_minimum(a, &nodes[0], &nodes, &idx).is_some(),
                    "{a:?} {found:?}"
                );
            }
            assert_eq!(
                held_by_minimum(Action::Uninstall, &nodes[0], &nodes, &idx),
                None
            );
            assert_eq!(
                requires_blocked(&nodes[0], &nodes, &idx),
                None,
                "stays in scope"
            );
        }
    }

    #[test]
    fn two_minimums_on_one_name_keep_the_higher_once() {
        let reqs = read_requirements(&[
            "Orca >= 2.0".into(),
            "Nushell".into(),
            "Orca >= 1.0".into(),
            "Orca >= 2.1".into(),
        ]);
        assert_eq!(
            minimums_of(&reqs),
            vec![("Orca".to_string(), "2.1".to_string())]
        );
    }
}
