// A removal is named on the button before the click — the rows here, the server's guard in
// server.rs (`drop_unannounced`). Run: node --test (from talos/)
//
// THE DEFECT THIS EXISTS FOR (measured 2026-10-27). Bundle Extras turned off, its two `run:`
// packages present, the page reloaded: the button said "2 installs" and the Apply uninstalled
// two more rows. The rule was the same on both sides; the FACTS were not. The scan handler
// labelled a config-atom wanted absent "yours" and stopped there, before its presence reached
// the model — so `actionOf` saw it not present and announced nothing, while the server re-probed
// it present and removed it through its `runUninstall`. Turned off live, the presence had
// already landed, which is why the live toggle did announce "2 to remove".
//
// The doctrine decides which side was wrong: a row with a way back is in scope (scope.js), and
// a package no active bundle pulls is wanted absent. So the server was right to remove them and
// the button was wrong to stay silent.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  announcedRemovals,
  applySavedActiveBundles,
  createModel,
  loadPlan,
  planActions,
  setStatusData,
} from "../public/model.js";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

// The demo of the measurement: Base pulls Alpha (present) and Beta (absent); Extras, turned
// off, pulled Delta and Epsilon, two present config-atoms with a way back.
function reloaded() {
  const m = createModel();
  const atom = { isConfig: true, canUninstall: true };
  loadPlan(m, [
    { i: 0, name: "Alpha", canUninstall: true },
    { i: 1, name: "Beta", canUninstall: true },
    { i: 2, name: "Delta", ...atom },
    { i: 3, name: "Epsilon", ...atom },
  ], [
    { name: "Base", packages: ["Alpha", "Beta"], needs: [] },
    { name: "Extras", packages: ["Delta", "Epsilon"], needs: [] },
  ]);
  applySavedActiveBundles(m, ["Base"]); // what selection.json holds after the reload
  for (const [i, status] of [[0, "ok"], [1, "waiting"], [2, "ok"], [3, "ok"]]) {
    setStatusData(m, i, status);
  }
  return m;
}

test("an untouched member of a bundle turned off is a removal, and the button counts it", () => {
  const m = reloaded();
  assert.deepEqual(planActions(m), ["install", "uninstall", "uninstall"]);
});

test("announcedRemovals: the rows whose uninstall or reinstall the button counts", () => {
  const m = reloaded();
  assert.deepEqual(announcedRemovals(m), [2, 3]);
  // A reinstall at the pin is a removal first, and the bar says it as one (bar.js).
  setStatusData(m, 0, "ok");
  Object.assign(m.pkgs.get(0), { pin: "1.0", installedVersion: "2.0", canDowngrade: true });
  assert.deepEqual(planActions(m), ["downgrade", "install", "uninstall", "uninstall"]);
  assert.deepEqual(announcedRemovals(m), [0, 2, 3]);
});

// app.js is DOM-bound and imported by no test: what follows reads it as text.
function arm(name) {
  const at = appjs.indexOf(`case "${name}":`);
  assert.notEqual(at, -1, `the front no longer handles \`${name}\``);
  return appjs.slice(at, appjs.indexOf("\n    case ", at + 10));
}

test("a scan verdict always reaches the model — no label leaves before the presence", () => {
  const body = arm("state");
  // The shortcut that lost the fact: a config-atom wanted absent became `self` and broke out.
  assert.doesNotMatch(body, /setStatus\(msg\.i, "self"\)/);
  const record = body.indexOf("setStatus(");
  assert.notEqual(record, -1);
  assert.doesNotMatch(body.slice(0, record), /\bbreak;/, "nothing may leave before setStatus");
});

test("a diff's `self` settles the row on its presence, and changes nothing else", () => {
  const body = arm("step");
  assert.match(body, /setStatus\(msg\.i, M\.presenceStatus\(model, msg\.i\)\)/);
  assert.doesNotMatch(body, /setStatus\(msg\.i, "self"\)/);
  // Found at the review: "ok" folds the row the operator opened to read the diff, and clears
  // what it is behind on.
  assert.match(body, /rows\[msg\.i\]\.details\.open = open/);
  assert.match(body, /Object\.assign\(p, \{ outdated, available \}\)/);
});

test("the Apply sends the removals the button announced", () => {
  const start = appjs.indexOf("function applyAll()");
  const body = appjs.slice(start, appjs.indexOf("\nfunction ", start + 10));
  assert.match(body, /removals: M\.announcedRemovals\(model\)/);
});

test("an unannounced removal is said on the row and never kept as a hold", () => {
  // Found at the review: sent as `held`, it was stored until a Refresh — a pinned row's
  // reinstall then stayed out of the count forever, and every Apply held it again.
  const body = arm("unannounced");
  assert.match(body, /statusLabel\.textContent = msg\.reason/);
  assert.doesNotMatch(body, /setHold/);
  assert.match(body, /refreshLiveness\(\)/, "the button counts the removal at once");
});

test("a removal said once is announced at the next click: nothing in the model holds it", () => {
  // The model half of the review's finding: what the server left, the button counts next.
  const m = reloaded();
  Object.assign(m.pkgs.get(0), { pin: "1.0", installedVersion: "2.0", canDowngrade: true });
  assert.deepEqual(announcedRemovals(m), [0, 2, 3]);
});
