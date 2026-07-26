// Structural guards on the re-scan veil. Run: node --test  (from talos/)
//
// No DOM library on purpose: this repo carries zero JS dependencies, and the
// defect these tests pin is STRUCTURAL (where the element lives, which code path
// clears it), not behavioural — so reading the source as text catches it exactly.
// The visual/hit-test half was verified in a real browser during a real scan.
//
// The bug: #steps-refresh was an absolutely-positioned child of #steps, so its
// `inset:0` resolved to the package list alone. The sticky Apply bar, the bundle
// cards, the tab strip and the header stayed crisp AND CLICKABLE while the server
// re-scanned — a crisp button invites a click that cannot be honoured.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

// The slice of markup from <div id="steps"> to its closing tag. #steps holds only
// dynamically-rendered rows, so its literal block in the source is short.
function stepsBlock() {
  const start = html.indexOf('<div id="steps"');
  assert.notEqual(start, -1, "#steps must exist");
  // The veil used to sit INSIDE here; anything before the next view boundary counts.
  const end = html.indexOf('<div id="view-log"', start);
  return html.slice(start, end === -1 ? undefined : end);
}

test("veil: #steps-refresh is NOT inside #steps (it must not be trapped there)", () => {
  assert.ok(
    !stepsBlock().includes('id="steps-refresh"'),
    "the veil is back inside #steps → `inset:0` covers the package list only",
  );
});

test("veil: it is positioned fixed, so `inset:0` means the viewport", () => {
  const rule = html.match(/#steps-refresh\s*\{[^}]*\}/);
  assert.ok(rule, "#steps-refresh needs a rule");
  assert.match(rule[0], /position:\s*fixed/, "absolute would re-trap it in a parent");
  assert.match(rule[0], /inset:\s*0/);
});

test("veil: it layers above the sticky Apply bar and below the dialogs", () => {
  const z = (sel) => {
    const m = html.match(new RegExp(`${sel}[^{]*\\{[^}]*z-index:\\s*(\\d+)`));
    assert.ok(m, `${sel} needs a z-index`);
    return Number(m[1]);
  };
  const veil = z("#steps-refresh");
  assert.ok(veil > z("\\.hero-apply"), "the sticky Apply bar must be covered");
  // A sudo prompt or a 403 modal raised DURING a scan has to stay answerable.
  assert.ok(veil < z("#overlay"), "dialogs must stay above the veil");
  assert.ok(veil < z("#splash"), "the boot splash must stay above the veil");
});

test("veil: every clear path goes through hideRescanVeil()", () => {
  // A full-screen veil that STICKS is far worse than a partial one — the window
  // would be unusable, not merely misleading. So no site may toggle the class by
  // hand: one show, one hide, and the three clears all call the hide.
  const strays = appjs.match(/classList\.(add|remove)\(\s*"steps-refreshing"/g) || [];
  assert.equal(
    strays.length,
    2,
    `only showRescanVeil/hideRescanVeil may touch the class, found ${strays.length} sites`,
  );
  const hides = appjs.match(/hideRescanVeil\(\)/g) || [];
  // definition + apply-plan + done + ws.onclose
  assert.ok(hides.length >= 4, `expected the 3 clear paths to call it, saw ${hides.length}`);
  // And the clears must be the REAL ones: named next to their triggers.
  for (const anchor of ['case "apply-plan":', 'case "done":', "ws.onclose"]) {
    const at = appjs.indexOf(anchor);
    assert.notEqual(at, -1, `${anchor} must exist`);
    assert.ok(
      appjs.slice(at, at + 700).includes("hideRescanVeil()"),
      `${anchor} must clear the veil`,
    );
  }
});

test("veil: the class rides <body>, whose subtree contains the veil", () => {
  assert.match(appjs, /function showRescanVeil\(\)\s*\{\s*document\.body\.classList\.add/);
  assert.match(appjs, /function hideRescanVeil\(\)\s*\{\s*document\.body\.classList\.remove/);
});
