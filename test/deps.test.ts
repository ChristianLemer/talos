// Pure dependency logic: future-state satisfaction + topological install order.
// Operates on plain {name, requires} nodes so it's testable without the engine.
import { assertEquals } from "@std/assert";
import { indexByName, requiresReason, topoSort } from "../src/deps.ts";

// A minimal node: its name, what it requires (by name), and whether it WILL be
// present after the Apply (present now OR desired-present).
const NODES = [
  { name: "Node.js", requires: [], willBePresent: true },
  { name: "Claude Code", requires: ["Node.js"], willBePresent: true },
  { name: "Chiron", requires: ["Claude Code"], willBePresent: true },
];

// --- indexByName -------------------------------------------------------------

Deno.test("indexByName: maps each name to its position", () => {
  const idx = indexByName(NODES);
  assertEquals(idx.get("Node.js"), 0);
  assertEquals(idx.get("Chiron"), 2);
  assertEquals(idx.get("nope"), undefined);
});

// --- requiresReason: FUTURE-STATE satisfaction -------------------------------

Deno.test("requiresReason: satisfied when the required package WILL be present", () => {
  // Node.js is willBePresent (mandatory) → Claude Code's requirement holds,
  // even though this is the future state, not the present.
  assertEquals(requiresReason(NODES[1], NODES), undefined);
});

Deno.test("requiresReason: unsatisfied when the required package won't be there", () => {
  const nodes = [
    { name: "Node.js", requires: [], willBePresent: false }, // unchecked / absent + unwanted
    { name: "Claude Code", requires: ["Node.js"], willBePresent: true },
  ];
  assertEquals(requiresReason(nodes[1], nodes), "requires Node.js");
});

Deno.test("requiresReason: unknown required name → says so", () => {
  const nodes = [{ name: "X", requires: ["Ghost"], willBePresent: true }];
  assertEquals(requiresReason(nodes[0], nodes), "requires Ghost (unknown)");
});

Deno.test("requiresReason: no requires → always satisfied", () => {
  assertEquals(requiresReason(NODES[0], NODES), undefined);
});

// --- topoSort: required BEFORE dependent, visual order as tie-break ----------

// Plan items carry the node index and keep any extra fields (e.g. action).
Deno.test("topoSort: a required package comes before its dependent", () => {
  // Plan given in the WRONG order (dependent first) — topoSort must fix it.
  const plan = [{ i: 2 }, { i: 1 }, { i: 0 }]; // Chiron, Claude Code, Node.js
  const sorted = topoSort(plan, NODES);
  assertEquals(sorted.map((p) => p.i), [0, 1, 2]); // Node.js → Claude → Chiron
});

Deno.test("topoSort: independent packages keep their given (visual) order", () => {
  const indep = [
    { name: "A", requires: [], willBePresent: true },
    { name: "B", requires: [], willBePresent: true },
    { name: "C", requires: [], willBePresent: true },
  ];
  const plan = [{ i: 0 }, { i: 1 }, { i: 2 }];
  assertEquals(topoSort(plan, indep).map((p) => p.i), [0, 1, 2]);
});

Deno.test("topoSort: only orders items IN the plan (a required item absent from the plan is ignored)", () => {
  // Plan installs only Claude Code (Node.js already present, not in plan).
  const plan = [{ i: 1 }];
  assertEquals(topoSort(plan, NODES).map((p) => p.i), [1]);
});

Deno.test("topoSort: a cycle falls back to the given order (never throws)", () => {
  const cyclic = [
    { name: "A", requires: ["B"], willBePresent: true },
    { name: "B", requires: ["A"], willBePresent: true },
  ];
  const plan = [{ i: 0 }, { i: 1 }];
  assertEquals(topoSort(plan, cyclic).map((p) => p.i), [0, 1]);
});

Deno.test("topoSort: preserves the action field, required item ordered first", () => {
  // Claude Code (i:1) requires Node.js (i:0), both in plan → Node.js first.
  const plan = [{ i: 1, action: "install" }, { i: 0, action: "install" }];
  const sorted = topoSort(plan, NODES);
  assertEquals(sorted[0], { i: 0, action: "install" });
  assertEquals(sorted[1], { i: 1, action: "install" });
});
