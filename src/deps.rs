// Port de deps.ts — dépendances de paquets (`requires:`) sur l'état FUTUR.
// Une exigence n'est pas "X est là maintenant ?" mais "X sera là après l'Apply ?".
// Vue GLOBALE (tous les paquets) que la détection par-step ne peut avoir.
use std::collections::{HashMap, HashSet};

/// Le minimum sur quoi deps raisonne. Les vrais Step le satisfont structurellement.
#[derive(Debug, Clone)]
pub struct DepNode {
    pub name: String,
    pub requires: Vec<String>,
    pub will_be_present: bool, // présent maintenant OU désiré-présent après l'Apply
}

fn index_by_name(nodes: &[DepNode]) -> HashMap<String, usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.clone(), i))
        .collect()
}

/// La raison pour laquelle les exigences d'un nœud ne tiennent pas, ou None.
/// Une exigence tient ssi le paquet requis existe dans le plan ET will_be_present.
/// Première exigence en échec gagne le message.
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

/// Ordonne le plan d'install pour qu'un paquet requis passe AVANT ses dépendants.
/// `plan` porte des `i` (index dans nodes). Kahn, en amorçant les nœuds prêts dans
/// l'ordre DONNÉ du plan (ordre visuel = tie-break stable). Arêtes vers des paquets
/// HORS plan ignorées (déjà présents). Un cycle → items restants en ordre donné
/// (défensif : ne panique jamais, ne perd jamais un item).
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

    // Ordre donné du plan pour départager les indépendants (tie-break visuel stable).
    let given_order: Vec<usize> = plan.iter().map(|(i, _)| *i).collect();
    loop {
        // prêts = deps satisfaites, non émis, dans l'ordre donné.
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
    // Cycle / reste non émis → ordre donné (défensif).
    for p in plan {
        if !emitted.contains(&p.0) {
            out.push(p.clone());
        }
    }
    out
}

/// Construit l'index name→pos (exposé pour les appelants).
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
        // node 0 "app" requires "lib" (node 1). Plan donné [0,1] → doit sortir [1,0].
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
