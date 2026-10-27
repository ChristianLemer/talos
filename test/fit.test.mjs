// The fit line: how many rows of the content THIS machine can get, and why the others
// cannot. Run: node --test "test/*.test.mjs"   (from talos/)
//
// What is pinned here is that the line is a COUNT of the rows' own reasons — the same
// strings the rows show, never a new category. Where it departs from a row, the
// departure is the machine's truth and is pinned by name below (a present OS package,
// a scope the user forced): model.js fitSummary lists them.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  createModel,
  fitLine,
  fitReason,
  fitSummary,
  loadPlan,
  scopeReason,
  setDecision,
  setExternal,
  setPresence,
  setScope,
} from "../public/model.js";

// A Linux-like machine: the OS package is detect-only and absent, two rows need it, one
// row needs an editor nobody wants, a winget-only row has no route, a config-atom has no
// way back but is a file Talos can write, and a tool is installed elsewhere.
function linuxBox() {
  const m = createModel();
  loadPlan(
    m,
    [
      { i: 0, name: "Windows", bundle: "", posture: "opt-in", canUninstall: false },
      { i: 1, name: "7-Zip", bundle: "", posture: "opt-in", canUninstall: true, requires: ["Windows"] },
      { i: 2, name: "VC++", bundle: "", posture: "opt-in", canUninstall: true, requires: ["Windows"] },
      { i: 3, name: "Editor", bundle: "", posture: "opt-in", canUninstall: true },
      { i: 4, name: "Extension", bundle: "", posture: "opt-in", canUninstall: true, requires: ["Editor"] },
      { i: 5, name: "Starship config", bundle: "", posture: "opt-in", canUninstall: false, isConfig: true },
      { i: 6, name: "jq", bundle: "", posture: "opt-in", canUninstall: true },
      { i: 7, name: "Git", bundle: "", posture: "opt-in", canUninstall: true },
    ],
    [],
  );
  for (const i of [0, 1, 2, 3, 4, 5]) setPresence(m, i, false);
  setPresence(m, 6, true);
  setPresence(m, 7, true);
  return m;
}

test("fitSummary: counts every row of the content, grouped by the rows' own reasons", () => {
  const s = fitSummary(linuxBox());
  assert.equal(s.total, 8);
  assert.equal(s.reachable, 4); // Editor, Starship config, jq, Git
  assert.deepEqual(s.groups, [
    { reason: "requires Windows", count: 2 },
    { reason: "requires Editor", count: 1 },
    { reason: "no-route", count: 1 },
  ]);
});

test("fitLine: one sentence, plural agreement, the row's words", () => {
  assert.equal(
    fitLine(fitSummary(linuxBox())),
    "4 of 8 within reach here · 2 require Windows · 1 requires Editor · 1 has no route here",
  );
});

test("fitLine: the all-reachable case is said too — a confirmation, not only an alarm", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "jq", bundle: "", posture: "opt-in", canUninstall: true },
    { i: 1, name: "Git", bundle: "", posture: "opt-in", canUninstall: true },
  ], []);
  assert.equal(fitLine(fitSummary(m)), "2 of 2 within reach here");
});

test("fitLine: nothing to count, nothing to say", () => {
  assert.equal(fitLine(fitSummary(createModel())), "");
});

test("fitSummary: a PRESENT detect-only row is within reach — the OS package on its own OS", () => {
  // The Windows row has no install route anywhere; on Windows it is simply there. Counting
  // it as `no route` would make the line lie on the very machine the kit was written for.
  const m = linuxBox();
  setPresence(m, 0, true);
  const s = fitSummary(m);
  assert.equal(s.reachable, 7);
  assert.deepEqual(s.groups, [{ reason: "requires Editor", count: 1 }]);
});

test("fitSummary: it follows the live facts — wanting the editor unblocks its extension", () => {
  // No stale verdict: the requirement reason is recomputed from the model each time, as the
  // row's own label is, so the line moves when the rows move.
  const m = linuxBox();
  setDecision(m, 3, "in");
  assert.deepEqual(fitSummary(m).groups, [
    { reason: "requires Windows", count: 2 },
    { reason: "no-route", count: 1 },
  ]);
});

test("fitReason: for every row the user left alone, it is the row's own out-of-reach reason", () => {
  // THE property the line promises, for rows that are absent or have a route: read against
  // the rows, it says what they say. Where a row shows a requirement or `no-route`, the line
  // counts that; anything else (in scope, `yours`, `available`, external) is within reach.
  // The one documented exception — a PRESENT row with no route — has its own test.
  const m = linuxBox();
  setExternal(m, 7, true); // Git installed elsewhere
  for (const i of m.pkgs.keys()) {
    const row = scopeReason(m, i);
    const expected = row === "no-route" || row?.startsWith("requires ") ? row : null;
    assert.equal(fitReason(m, i), expected, `row ${m.pkgs.get(i).name}: row says ${row}`);
  }
});

test("fitReason: a present row with no route reads `—` but is within reach — it is there", () => {
  // The exception to the property above, on the very machine a Windows kit is written for:
  // the OS package is detect-only, so its row says `no-route`, yet the machine has it.
  const m = linuxBox();
  setPresence(m, 0, true);
  assert.equal(scopeReason(m, 0), "no-route");
  assert.equal(fitReason(m, 0), null);
});

test("fitReason: the user's scope does not move what the machine can reach", () => {
  // Out by hand: still within reach. In by hand on a blocked row: the row's label goes back
  // to presence, but the Apply still drops it with the same `requires Windows`.
  const m = linuxBox();
  setScope(m, 6, "out");
  assert.equal(fitReason(m, 6), null);
  setScope(m, 1, "in");
  assert.equal(scopeReason(m, 1), null);
  assert.equal(fitReason(m, 1), "requires Windows");
  // The same for a row with no route forced in: the row drops its `—`, the route is still missing.
  setScope(m, 0, "in");
  assert.equal(scopeReason(m, 0), null);
  assert.equal(fitReason(m, 0), "no-route");
});

test("fitReason: a config-atom is within reach unless its requirement fails", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Git", bundle: "", posture: "opt-in", canUninstall: true },
    { i: 1, name: "Git default branch", bundle: "", posture: "opt-in", canUninstall: false, isConfig: true, requires: ["Git"] },
  ], []);
  setPresence(m, 0, false);
  setPresence(m, 1, false);
  assert.equal(fitReason(m, 1), "requires Git");
  setPresence(m, 0, true);
  assert.equal(fitReason(m, 1), null);
});

test("fitLine: a dangling requirement is counted under its own words, conjugated", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "A", bundle: "", posture: "opt-in", canUninstall: true, requires: ["Gone"] },
    { i: 1, name: "B", bundle: "", posture: "opt-in", canUninstall: true, requires: ["Gone"] },
  ], []);
  assert.equal(fitLine(fitSummary(m)), "0 of 2 within reach here · 2 require Gone (unknown)");
});

test("the fit line is painted from refreshLiveness, never mid-scan, and only in advanced mode", () => {
  // Structural: refreshLiveness runs on every scan event, step and gesture, so a paint
  // hooked there cannot go stale; the CSS keeps it out of simple mode entirely.
  const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
  const body = appjs.slice(appjs.indexOf("function refreshLiveness"));
  assert.match(body.slice(0, body.indexOf("\n}\n")), /paintFit\(\)/);
  // …and it holds still while the scan runs: an un-probed row is not absent.
  const paint = appjs.slice(appjs.indexOf("function paintFit"));
  assert.match(paint.slice(0, paint.indexOf("\n}\n")), /scanning\) return/);
  assert.match(html, /id="fit-line"/);
  assert.match(html, /body:not\(\.advanced\)\s+#fit-line\s*\{\s*display:\s*none/);
});
