// Structural guard for the `self-updated` message: the server says a row's own update ran ok in
// this session, and the front must carry it into the model `actionOf` reads. Read as text: app.js
// is DOM-bound and imported by no test (see one-path-ui.test.mjs).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

test("`self-updated` sets the row done (or forgets it) in the model, then repaints and re-counts the Apply", () => {
  const at = appjs.indexOf('case "self-updated":');
  assert.notEqual(at, -1, "the front no longer handles `self-updated`");
  const arm = appjs.slice(at, appjs.indexOf("case ", at + 20));
  assert.match(arm, /M\.setUpdatedThisSession\(model, msg\.i, msg\.done !== false\);[^]*refreshLiveness\(\);/);
});
