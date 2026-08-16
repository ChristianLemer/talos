// Structural guards on the Arbitration tab. Run: node --test
//
// The defect: the Arbitration tab hides the Apply button but NOT the ladder dial,
// so the dial's second click reaches applyAll() with a count describing a different
// row set. A source-reading test catches this kind of structural claim exactly.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

test("tab-arbitration hides #ladder, mirroring tab-catalog", () => {
  // Both readonly tabs (catalog, arbitration) hide the ladder dial because the
  // dial's second click reaches applyAll(), and that would Apply with a count
  // describing the wrong row set.
  assert.match(
    html,
    /body\.tab-catalog\s+#ladder\s*\{[^}]*display:\s*none/,
    "tab-catalog must hide #ladder (the established pattern)",
  );
  assert.match(
    html,
    /body\.tab-arbitration\s+#ladder\s*\{[^}]*display:\s*none/,
    "tab-arbitration must hide #ladder (pairs with tab-catalog)",
  );
});

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
