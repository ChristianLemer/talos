// deps.ts — package dependencies (`requires:`), on FUTURE state.
//
// A requirement is not "is X here now?" but "will X be here after the Apply?".
// Each node carries `willBePresent` (present now OR desired-present), computed by
// the caller from live presence + desiredState. This module is pure and holds the
// GLOBAL view (all packages at once) that per-step detection can't — mirroring
// decision.js's role for per-package rules.
//
// Two jobs:
//   requiresReason — why a package can't be satisfied (a required package that
//                    won't be present), or undefined when it's fine.
//   topoSort       — order an install plan so every required package comes BEFORE
//                    its dependents; visual (given) order breaks ties.

// The minimal shape deps reasons over. Real Steps satisfy this structurally.
export interface DepNode {
  name: string;
  requires: string[];
  willBePresent: boolean; // present now OR desired-present after the Apply
}

// name → position, for O(1) requirement lookup.
export function indexByName(nodes: DepNode[]): Map<string, number> {
  const m = new Map<string, number>();
  nodes.forEach((n, i) => m.set(n.name, i));
  return m;
}

// The reason a node's requirements aren't met, or undefined when all hold. A
// requirement holds iff the required package exists in the plan AND willBePresent
// (future state). First failing requirement wins the message.
export function requiresReason(
  node: DepNode,
  nodes: DepNode[],
  idx: Map<string, number> = indexByName(nodes),
): string | undefined {
  for (const req of node.requires) {
    const at = idx.get(req);
    if (at === undefined) return `requires ${req} (unknown)`;
    if (!nodes[at].willBePresent) return `requires ${req}`;
  }
  return undefined;
}

// Order the install plan so a required package is installed BEFORE its dependents.
// Plan items carry `i` (index into nodes) plus any extra fields (e.g. action),
// preserved as-is. A dependency edge points required → dependent. Kahn's
// algorithm, seeding ready nodes in the plan's GIVEN order so independent items
// keep their visual order. Edges to packages NOT in the plan are ignored (already
// present, nothing to install). A cycle leaves the remaining items in given order
// (defensive — never throws, never drops an item).
export function topoSort<T extends { i: number }>(
  plan: T[],
  nodes: DepNode[],
  idx: Map<string, number> = indexByName(nodes),
): T[] {
  const inPlan = new Set(plan.map((p) => p.i));
  // For each plan item: how many of its requirements are ALSO in the plan (must
  // come first), and who depends on it (to release once installed).
  const remaining = new Map<number, number>(); // node index → unmet in-plan deps
  const dependents = new Map<number, number[]>(); // node index → node indexes needing it
  for (const p of plan) {
    let deps = 0;
    for (const req of nodes[p.i].requires) {
      const at = idx.get(req);
      if (at !== undefined && inPlan.has(at)) {
        deps++;
        const list = dependents.get(at) ?? [];
        list.push(p.i);
        dependents.set(at, list);
      }
    }
    remaining.set(p.i, deps);
  }

  const byIndex = new Map(plan.map((p) => [p.i, p]));
  const out: T[] = [];
  const emitted = new Set<number>();
  // Ready = deps met; drain in the plan's given order for a stable visual tie-break.
  let ready = plan.filter((p) => remaining.get(p.i) === 0).map((p) => p.i);
  while (ready.length) {
    const i = ready.shift()!;
    if (emitted.has(i)) continue;
    out.push(byIndex.get(i)!);
    emitted.add(i);
    const freed: number[] = [];
    for (const dep of dependents.get(i) ?? []) {
      remaining.set(dep, (remaining.get(dep) ?? 0) - 1);
      if (remaining.get(dep) === 0) freed.push(dep);
    }
    // Append freed dependents; keep ready ordered by the plan's given order.
    if (freed.length) {
      ready.push(...freed);
      ready = plan.filter((p) => ready.includes(p.i) && !emitted.has(p.i)).map((
        p,
      ) => p.i);
    }
  }
  // Cycle / anything left unemitted → append in given order (defensive).
  for (const p of plan) if (!emitted.has(p.i)) out.push(p);
  return out;
}
