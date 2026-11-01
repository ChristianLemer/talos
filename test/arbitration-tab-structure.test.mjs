// Structural guards on the Arbitration tab. Run: node --test
//
// A source-reading test catches this kind of structural claim exactly. (Its first guard,
// that the tab hid the ladder dial, left with the ladder on 2026-10-10.)
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

test("applyAdvanced returns to bundles when turning off from arbitration", () => {
  // If advanced goes off while on the Arbitration tab, the operator is stranded
  // on an invisible view until relaunch. The fix: return to the default tab.
  const fnStart = appjs.indexOf("function applyAdvanced(");
  assert.notEqual(fnStart, -1, "applyAdvanced function must exist");
  const fnEnd = appjs.indexOf("\n}", fnStart);
  const fnBody = appjs.slice(fnStart, fnEnd);

  // The function must check for tab-arbitration and call setTab
  assert.match(
    fnBody,
    /tab-arbitration/,
    "applyAdvanced must check if on the arbitration tab",
  );
  assert.match(
    fnBody,
    /setTab\s*\(\s*["']bundles["']\s*\)/,
    "applyAdvanced must return to bundles tab when appropriate",
  );
});

test("an empty Arbitration tab says so, and only once the machine has been read", () => {
  // Before 2026-11-01 the tab with nothing held was a header over a black page: nothing told
  // "nothing to arbitrate" from a view that failed to paint.
  const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
  assert.match(html, /id="arb-empty"[^>]*hidden/);
  assert.match(html, /body:not\(\.tab-arbitration\)\s+#arb-empty\s*\{\s*display:\s*none/);
  const fnStart = appjs.indexOf("function refreshArbitration(");
  const fnBody = appjs.slice(fnStart, appjs.indexOf("\n}", fnStart));
  assert.match(fnBody, /arb-empty/);
  assert.match(fnBody, /n !== 0 \|\| scanning \|\| !planSeen/);
});
