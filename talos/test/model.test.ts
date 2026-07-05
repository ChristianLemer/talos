import { assertEquals } from "@std/assert";
import {
  actionOf,
  applySavedSelection,
  bundleAct,
  bundleAnyActionable,
  bundleLocked,
  bundleToggleState,
  buttonAction,
  clearAllDecisions,
  createModel,
  desiredOf,
  isActionable,
  isDeviated,
  isLocked,
  loadPlan,
  persistablePkgs,
  setDecision,
  setStatusData,
  toggleOf,
} from "../public/model.js";

// A small fixture plan: one mandatory pkg, one opt-in, one opt-out, in 2 bundles.
function seed() {
  const m = createModel();
  loadPlan(
    m,
    [
      { name: "Base", posture: "mandatory", selectable: false },
      { name: "Extras", posture: "opt-in", selectable: true },
    ],
    [
      {
        i: 0,
        name: "Node",
        bundle: "Base",
        posture: "mandatory",
        canUninstall: true,
      },
      {
        i: 1,
        name: "rg",
        bundle: "Extras",
        posture: "opt-in",
        canUninstall: true,
      },
      {
        i: 2,
        name: "bat",
        bundle: "Extras",
        posture: "opt-in",
        canUninstall: true,
      },
    ],
  );
  return m;
}

Deno.test("loadPlan wires packages into bundles", () => {
  const m = seed();
  assertEquals(m.pkgs.size, 3);
  assertEquals(m.bundles.get("Extras")?.pkgIds, [1, 2]);
  assertEquals(m.pkgs.get(1)?.key, "Extras::rg");
});

Deno.test("posture defaults: mandatory locked in, opt-in defaults out", () => {
  const m = seed();
  assertEquals(isLocked(m, 0), true); // mandatory
  assertEquals(toggleOf(m, 0), "in");
  assertEquals(isLocked(m, 1), false); // opt-in
  assertEquals(toggleOf(m, 1), "out"); // opt-in defaults out
  assertEquals(desiredOf(m, 1), "absent");
});

Deno.test("setDecision moves an opt-in in; locked package refuses", () => {
  const m = seed();
  assertEquals(setDecision(m, 1, "in"), true);
  assertEquals(toggleOf(m, 1), "in");
  assertEquals(isDeviated(m, 1), true);
  assertEquals(setDecision(m, 0, "out"), false); // mandatory can't move
  assertEquals(toggleOf(m, 0), "in");
});

Deno.test("actionOf: opt-in wanted+absent → install; mandatory present → null", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setStatusData(m, 1, "waiting"); // absent
  assertEquals(actionOf(m, 1), "install");
  assertEquals(isActionable(m, 1), true);
  setStatusData(m, 0, "ok"); // mandatory present, current
  assertEquals(actionOf(m, 0), null);
});

Deno.test("buttonAction always inverts machine state", () => {
  const m = seed();
  setStatusData(m, 1, "waiting"); // absent
  assertEquals(buttonAction(m, 1)?.type, "install");
  setStatusData(m, 1, "ok"); // present
  assertEquals(buttonAction(m, 1)?.type, "uninstall");
});

Deno.test("bundle toggle state: mixed when packages disagree", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setDecision(m, 2, "out");
  assertEquals(bundleToggleState(m, "Extras"), "mixed");
  setDecision(m, 2, "in");
  assertEquals(bundleToggleState(m, "Extras"), "on-in");
});

Deno.test("Base bundle is locked (not selectable)", () => {
  const m = seed();
  assertEquals(bundleLocked(m, "Base"), true);
  assertEquals(bundleLocked(m, "Extras"), false);
});

Deno.test("persist + restore selection by key", () => {
  const m = seed();
  setDecision(m, 1, "in");
  const saved = { pkgs: persistablePkgs(m) };
  assertEquals(saved.pkgs, { "Extras::rg": "in" });
  const m2 = seed();
  applySavedSelection(m2, saved);
  assertEquals(toggleOf(m2, 1), "in");
});

Deno.test("clearAllDecisions returns everything to posture defaults", () => {
  const m = seed();
  setDecision(m, 1, "in");
  clearAllDecisions(m);
  assertEquals(toggleOf(m, 1), "out"); // back to opt-in default
  assertEquals(m.decision.size, 0);
});

Deno.test("bundleAnyActionable + bundleAct reflect pending package actions", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setStatusData(m, 1, "waiting"); // opt-in wanted but absent → install
  assertEquals(bundleAnyActionable(m, "Extras"), true);
  assertEquals(bundleAct(m, "Extras"), "add");
});

Deno.test("bundleToggleState: all-locked bundle defaults to on-in", () => {
  const m = seed();
  // Base holds only the mandatory Node (locked) → no free packages.
  assertEquals(bundleToggleState(m, "Base"), "on-in");
});
