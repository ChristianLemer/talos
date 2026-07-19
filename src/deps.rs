// Port of deps.ts — package dependencies (`requires:`) over the FUTURE state.
// A requirement is not "is X here now?" but "will X be here after the Apply?".
// GLOBAL view (all packages) that per-step detection cannot have.
use std::collections::{HashMap, HashSet};

/// The minimum deps reasons over. The real Steps satisfy it structurally.
#[derive(Debug, Clone)]
pub struct DepNode {
    pub name: String,
    pub requires: Vec<String>,
    pub will_be_present: bool, // present now OR desired-present after the Apply
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
pub fn requires_reason(node: &DepNode, nodes: &[DepNode], idx: &HashMap<String, usize>) -> Option<String> {
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

/// Orders the install plan so a required package comes BEFORE its dependents.
/// `plan` carries `i` (index into nodes). Kahn, seeding the ready nodes in
/// the GIVEN order of the plan (visual order = stable tie-break). Edges to packages
/// OUT of the plan ignored (already present). A cycle → remaining items in given order
/// (defensive: never panics, never loses an item).
pub fn topo_sort<T: Clone>(plan: &[(usize, T)], nodes: &[DepNode], idx: &HashMap<String, usize>) -> Vec<(usize, T)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, requires: &[&str], present: bool) -> DepNode {
        DepNode { name: name.into(), requires: requires.iter().map(|s| s.to_string()).collect(), will_be_present: present }
    }

    #[test]
    fn reason_unmet_requirement() {
        let nodes = vec![node("a", &["b"], true), node("b", &[], false)];
        let idx = make_index(&nodes);
        assert_eq!(requires_reason(&nodes[0], &nodes, &idx), Some("requires b".into()));
    }

    #[test]
    fn reason_unknown_requirement() {
        let nodes = vec![node("a", &["ghost"], true)];
        let idx = make_index(&nodes);
        assert_eq!(requires_reason(&nodes[0], &nodes, &idx), Some("requires ghost (unknown)".into()));
    }

    #[test]
    fn reason_none_when_satisfied() {
        let nodes = vec![node("a", &["b"], true), node("b", &[], true)];
        let idx = make_index(&nodes);
        assert_eq!(requires_reason(&nodes[0], &nodes, &idx), None);
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
