// Package dependencies (`requires:`) over the FUTURE state.
// A requirement is not "is X here now?" but "will X be here after the Apply?".
// GLOBAL view (all packages) that per-step detection cannot have.
use std::collections::{HashMap, HashSet};

/// The minimum deps reasons over. The real Steps satisfy it structurally.
#[derive(Debug, Clone)]
pub struct DepNode {
    pub name: String,
    pub requires: Vec<String>,
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
}
