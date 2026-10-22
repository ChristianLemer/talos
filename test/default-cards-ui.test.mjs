// Structural guards for `default: on` — the author's cards checked on a FIRST launch, which the
// engine decides from the ABSENCE of selection.json. Read as text: app.js is DOM-bound and
// imported by no test (see one-path-ui.test.mjs).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

function body(name) {
  const at = appjs.indexOf(`function ${name}(`);
  assert.notEqual(at, -1, `${name} moved — re-point this test`);
  return appjs.slice(at, appjs.indexOf("\n}", at));
}

// An Apply is a gesture, and the one that matters most: a machine that opens on the default
// cards and clicks only Apply must leave a selection behind. Without it every later launch is
// still a "first" one, and an author who later drops `default: on` turns what that Apply
// installed into "wanted absent" — the next Apply would uninstall it.
test("an Apply saves the selection it acts on", () => {
  assert.match(body("applyAll"), /persistSelection\(\)/);
});

// Before the plan lands the model holds no cards. A theme or a setting clicked in that window
// would save an EMPTY active set, and the first launch would stop seeding for good.
test("nothing is saved before the plan has been restored", () => {
  assert.match(body("persistSelection"), /if \(!planRestored\) return;/);
  const at = appjs.indexOf('case "plan":');
  const plan = appjs.slice(at, appjs.indexOf("case ", at + 12));
  assert.match(plan, /applySavedSelection\(msg\.selection\);[^]*planRestored = true;/);
});
