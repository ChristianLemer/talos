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
