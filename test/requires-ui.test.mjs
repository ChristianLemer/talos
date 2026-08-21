// Structural guard on the ROW LABEL for a requirement-blocked row.
// Run: node --test "test/*.test.mjs"   (from talos/)
//
// No DOM library on purpose — same technique as reload-ui.test.mjs and veil.test.mjs:
// this repo carries zero JS dependencies, and what needs pinning here is structural.
//
// THE DEFECT THIS EXISTS FOR. `scopeReason` returns a CLOSED vocabulary for every
// other out-of-scope situation (`yours`, `available`, `no-route`, `not-managed`), and
// app.js renders it through the `SCOPE_LABEL` lookup table. A requirement reason is
// the one open-ended member of that set: it carries a package NAME, so it can never
// be a key. A bare `SCOPE_LABEL[reason]` yields `undefined`, which React-less DOM
// code writes into `textContent` as the string "undefined" — or, worse, blanks the
// label. Either way the operator gets a row that is inert with NO stated reason,
// which is precisely the silent state this whole change exists to abolish.
//
// Not caught by model.test.mjs: the model returns the right string, and that is
// exactly what makes the rendering half easy to miss.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { scopeReason } from "../public/scope.js";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

test("the scope label FALLS BACK to the reason itself when it is not a known token", () => {
  // The lookup must be guarded — `SCOPE_LABEL[reason] ?? reason` or an equivalent —
  // so an open-ended reason reaches the screen verbatim.
  assert.match(
    appjs,
    /SCOPE_LABEL\[reason\]\s*(\?\?|\|\|)\s*reason/,
    "SCOPE_LABEL[reason] alone drops an open-ended reason on the floor",
  );
});

test("a requirement reason is genuinely absent from SCOPE_LABEL's keys", () => {
  // The premise of the test above, asserted rather than assumed: if someone ever adds
  // a `requires` KEY to the table, the fallback stops being load-bearing and this
  // file should be re-read rather than silently kept.
  const table = appjs.slice(appjs.indexOf("const SCOPE_LABEL"), appjs.indexOf("};", appjs.indexOf("const SCOPE_LABEL")));
  const reason = scopeReason({
    canUninstall: true,
    external: false,
    isConfig: false,
    pulled: true,
    requiresReason: "requires Windows",
  });
  assert.equal(reason, "requires Windows");
  assert.ok(!table.includes("requires"), "SCOPE_LABEL grew a `requires` key — re-read this test");
});
