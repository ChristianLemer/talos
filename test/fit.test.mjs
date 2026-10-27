// The fit line: how many rows of the content THIS machine can get, and why the others
// cannot. Run: node --test "test/*.test.mjs"   (from talos/)
//
// What is pinned here is that the line is a COUNT of the rows' own reasons — the same
// strings the rows show, never a new category — so it cannot read false against them.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createModel, fitLine, fitSummary, loadPlan, setDecision, setPresence } from "../public/model.js";

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
