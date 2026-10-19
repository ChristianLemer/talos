// Structural guards for the ONE path: an Apply converges the whole desire, no rung.
//
// The ladder (⚡ config only · 🧩 extensions · 📦 apps) left on 2026-10-10: applying PART of a
// desire built states nothing reasons about — a fresh config over a stale app — and nobody ever
// moved the slider. The nature glyph stays on the row; only the filter left. Read as text for
// the reason ladder-ui.test.mjs gave: app.js and index.html are DOM-bound and imported by no test.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";

const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

test("no ladder is left in the page or the app", () => {
  assert.doesNotMatch(html, /id="ladder"|id="dial"|class="zone/);
  assert.doesNotMatch(appjs, /\brung\b|RUNGS|renderDial|setRungOut/);
  assert.ok(!existsSync(new URL("../public/ladder.js", import.meta.url)));
});

test("the apply message carries the desire and no rung", () => {
  const at = appjs.indexOf("function applyAll(");
  assert.notEqual(at, -1, "applyAll moved — re-point this test");
  const body = appjs.slice(at, appjs.indexOf("\n}", at));
  assert.match(body, /type:\s*"apply"/);
  assert.match(body, /unmanaged:/, "the derived out-of-scope list still travels");
  assert.doesNotMatch(body, /rung/);
});

test("the Apply button is the only way in, and it asks the shared plan", () => {
  // Moved from ladder-ui.test.mjs: the button is wired, and its emptiness comes from the same
  // function that lists what an Apply would touch — never a second rule.
  assert.match(appjs, /getElementById\("install-all"\)\.onclick = \(\) => applyAll\(\)/);
  const at = appjs.indexOf("function refreshLiveness()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /M\.applyPlan\(model\)/, "the button reads the shared plan");
});

test("the empty state speaks without a rung, in the list, never mid-scan", () => {
  assert.match(html, /id="steps-empty"/);
  assert.ok(
    html.indexOf('id="steps-empty"') < html.indexOf('id="steps"'),
    "the empty state belongs with the list, above its rows",
  );
  assert.match(appjs, /already matches what you asked for/);
  assert.doesNotMatch(appjs, /widen the scope/);
  // An un-probed row must never count as absent: no "nothing to send" while scanning.
  assert.match(appjs, /hint\.hidden = plan\.length !== 0 \|\| scanning/);
});
