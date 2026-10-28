// `replaces:` — the OVERRIDE. A wanted package makes what it stands in for wanted absent,
// whatever that row's own intention says, and never writes that intention: turn the
// replacement off and the replaced row wants what it wanted before. Run: node --test (from talos/)
//
// Chosen over "the replaced row falls to off" (2026-10-28): that wrote into another row's
// intention from a gesture on this one, lost what it was (On, Auto, Manual), and left nothing
// installed when the beta was turned off again.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  actionOf,
  announcedRemovals,
  createModel,
  desiredOf,
  loadPlan,
  manualToggle,
  planActions,
  replacedBy,
  replacedReason,
  setDecision,
  setScope,
  setStatusData,
  wantedNames,
  applySavedActiveBundles,
  removeProfile,
  profileStateOf,
  setHold,
  setRemovalHold,
  holdReason,
  replacedIndices,
} from "../public/model.js";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

// Stable is On and installed; Beta stands in for it and is not wanted yet. Stable needs Lib.
function channels() {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Stable", canUninstall: true, requires: ["Lib"] },
    { i: 1, name: "Beta", canUninstall: true, replaces: ["Stable"] },
    { i: 2, name: "Lib", canUninstall: true },
  ], []);
  setDecision(m, 0, "in");
  for (const [i, status] of [[0, "ok"], [1, "waiting"], [2, "ok"]]) setStatusData(m, i, status);
  return m;
}

test("a replacement nobody wants changes nothing", () => {
  const m = channels();
  assert.equal(replacedBy(m, 0), null);
  assert.equal(desiredOf(m, 0), "present");
  assert.deepEqual(planActions(m), []);
});

test("wanting the replacement removes what it replaces, announced, intention untouched", () => {
  const m = channels();
  setDecision(m, 1, "in");
  assert.equal(replacedBy(m, 0), "Beta");
  assert.equal(replacedReason(m, 0), "replaced by Beta");
  assert.equal(desiredOf(m, 0), "absent");
  assert.equal(manualToggle(m, 0), "in", "the override is derived, never written");
  assert.equal(actionOf(m, 0), "uninstall");
  assert.equal(actionOf(m, 1), "install");
  // The removals are on the button before the click, so the server's guard lets them through.
  assert.deepEqual(announcedRemovals(m), [0, 2]);
  // A replaced row pulls none of its requirements: Lib was wanted only through Stable.
  assert.ok(!wantedNames(m).has("Lib"));
  assert.equal(actionOf(m, 2), "uninstall");
});

test("turning the replacement off gives the replaced row its intention back", () => {
  const m = channels();
  setDecision(m, 1, "in");
  // After the Apply: Beta installed, Stable gone.
  setStatusData(m, 0, "waiting");
  setStatusData(m, 1, "ok");
  setDecision(m, 1, "out");
  assert.equal(replacedBy(m, 0), null);
  assert.equal(actionOf(m, 0), "install", "Stable comes back, as its toggle always said");
  assert.equal(actionOf(m, 1), "uninstall");
});

test("a bundle pulling both: the replacement still wins", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Stable", canUninstall: true },
    { i: 1, name: "Beta", canUninstall: true, replaces: ["Stable"] },
  ], [
    { name: "Main", packages: ["Stable"], needs: [] },
    { name: "Preview", packages: ["Beta"], needs: [] },
  ]);
  applySavedActiveBundles(m, ["Main", "Preview"]);
  setStatusData(m, 0, "ok");
  setStatusData(m, 1, "waiting");
  assert.deepEqual(planActions(m).sort(), ["install", "uninstall"]);
  removeProfile(m, "Preview");
  assert.equal(actionOf(m, 0), null, "Preview off: Stable is simply kept");
});

test("a Manual row is never touched, nor what it requires", () => {
  // Review finding 3: out of scope, the row was spared but its requirements were pulled out.
  const m = channels();
  setDecision(m, 1, "in");
  setScope(m, 0, "out");
  assert.equal(replacedBy(m, 0), null);
  assert.equal(actionOf(m, 0), null);
  assert.ok(wantedNames(m).has("Lib"), "Stable is kept, so what it needs is kept");
  assert.deepEqual(announcedRemovals(m), []);
});

test("a replacement that cannot land replaces nothing", () => {
  // Review finding 1: the plan was [uninstall Stable] and no install — neither left.
  const cases = {
    "no route here": (m) => { m.pkgs.get(1).canUninstall = false; },
    "a requirement it cannot get": (m) => { m.pkgs.get(1).requires = ["Windows"]; },
    "parked on Manual": (m) => { setScope(m, 1, "out"); },
  };
  for (const [why, cut] of Object.entries(cases)) {
    const m = channels();
    loadPlan(m, [
      { i: 0, name: "Stable", canUninstall: true, requires: ["Lib"] },
      { i: 1, name: "Beta", canUninstall: true, replaces: ["Stable"] },
      { i: 2, name: "Lib", canUninstall: true },
      { i: 3, name: "Windows", canUninstall: false },
    ], []);
    setDecision(m, 0, "in");
    setDecision(m, 1, "in");
    for (const [i, status] of [[0, "ok"], [1, "waiting"], [2, "ok"], [3, "waiting"]]) setStatusData(m, i, status);
    cut(m);
    assert.equal(replacedBy(m, 0), null, why);
    assert.equal(actionOf(m, 0), null, why);
  }
});

test("a removal kept because the replacement is not there is not counted again", () => {
  const m = channels();
  setDecision(m, 1, "in");
  assert.deepEqual(replacedIndices(m), [0], "sent so the server makes it wait");
  setStatusData(m, 1, "failed");
  setRemovalHold(m, 0, "kept: Beta replaces it and is not installed — nothing would be left");
  assert.match(holdReason(m, 0), /is not installed/);
  assert.ok(!announcedRemovals(m).includes(0), "the button no longer offers it");
  // The replacement lands (a retry, the next Apply): the hold has nothing left to say.
  setStatusData(m, 1, "ok");
  assert.equal(holdReason(m, 0), null);
  assert.ok(announcedRemovals(m).includes(0));
});

test("an install's hold never blocks a removal the user asks for, nor the reverse", () => {
  // Review B: a brew-pin hold on a row the user then refuses stopped its removal.
  const m = channels();
  setHold(m, 0, "brew offers 2.0, not 1.0");
  setDecision(m, 0, "out");
  assert.equal(actionOf(m, 0), "uninstall");
  assert.equal(holdReason(m, 0), null);
  assert.ok(announcedRemovals(m).includes(0));
  assert.deepEqual(replacedIndices(m), [], "the user's own refusal waits for nothing");
  // A kept removal outlives the override only as text: refused by hand, the row goes.
  setRemovalHold(m, 0, "kept: …");
  setDecision(m, 1, "in");
  assert.equal(holdReason(m, 0), null);
});

test("a replacement the server holds replaces nothing", () => {
  // Review A: Beta's brew pin held at plan time, and Stable went alone.
  const m = channels();
  setDecision(m, 1, "in");
  setHold(m, 1, "brew offers 2.0, not 1.0");
  assert.equal(replacedBy(m, 0), null);
  assert.equal(actionOf(m, 0), null);
});

test("a bundle whose member is replaced still reads full", () => {
  // Review finding 6: the card read `hollow` — "you pulled one out" — when nobody had.
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Stable", canUninstall: true },
    { i: 1, name: "Beta", canUninstall: true, replaces: ["Stable"] },
  ], [{ name: "Main", packages: ["Stable"], needs: [] }]);
  applySavedActiveBundles(m, ["Main"]);
  setDecision(m, 1, "in");
  assert.equal(replacedBy(m, 0), "Beta");
  assert.equal(profileStateOf(m, "Main"), "full");
});

test("the row says why its toggle and its action disagree", () => {
  const at = appjs.indexOf("M.scopeReason(model, i) ?? M.replacedReason(model, i)");
  assert.notEqual(at, -1, "the replaced row's label is no longer painted");
});
