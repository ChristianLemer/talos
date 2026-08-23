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

// The same source with every CSS and HTML comment blanked out.
//
// ⚠️ Not tidiness — validity. A selector-scoped assertion like "#splash declares no
// z-index" walks forward from the name looking for a `{…}`. Prose mentioning `#splash`
// therefore lets it walk into whatever rule comes NEXT, and the guard reports a property
// that belongs to a different selector. Caught the day it was written: the new guard failed
// on a comment that named both instances just above the shared `.veil` rule. This repo has
// been burned by the same class of mistake in Rust — read code, never prose.
const css = html.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/<!--[\s\S]*?-->/g, " ");

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

// ⭐ Since the 2026-08-23 merge these properties live on the SHARED `.veil` class, not on
// the id: the boot splash and the rescan veil are one object with two instances. So the
// chain has two links to guard — the class must carry the geometry, and the instance must
// carry the class. Asserting only the first would pass on a panel that never opted in.
test("veil: the rescan instance opts into the shared .veil class", () => {
  assert.match(
    html,
    /id="steps-refresh"[^>]*class="[^"]*\bveil\b/,
    "#steps-refresh must carry class=\"veil\" or it inherits none of the shared geometry",
  );
});

test("veil: it is positioned fixed, so `inset:0` means the viewport", () => {
  const rule = html.match(/\.veil\s*\{[^}]*\}/);
  assert.ok(rule, ".veil needs a rule");
  assert.match(rule[0], /position:\s*fixed/, "absolute would re-trap it in a parent");
  assert.match(rule[0], /inset:\s*0/);
});

test("veil: it layers above the sticky Apply bar and below the dialogs", () => {
  const z = (sel) => {
    const m = html.match(new RegExp(`${sel}[^{]*\\{[^}]*z-index:\\s*(\\d+)`));
    assert.ok(m, `${sel} needs a z-index`);
    return Number(m[1]);
  };
  // `.veil`, not `#steps-refresh`: one height for BOTH instances since the merge. The boot
  // splash used to sit at 80 — above the six modals at 60 — with no stated reason, which
  // left the first-boot sharing dialog invisible and unclickable behind it for the whole
  // scan. One class, one height, and the defect cannot come back per-instance.
  const veil = z("\\.veil");
  assert.ok(veil > z("\\.hero-apply"), "the sticky Apply bar must be covered");
  // A sudo prompt or a 403 modal raised DURING a scan has to stay answerable.
  //
  // ⭐ Named DIRECTLY since 2026-08-23. This used to assert against `#overlay`, a generic
  // panel that stood in for "the dialog layer" — and when `#overlay` was deleted (nothing
  // ever opened it) the test failed for a reason that had nothing to do with layering. A
  // proxy reference breaks on the proxy's fate instead of on the property it guards. These
  // two ARE the dialogs the comment above promises to keep answerable.
  assert.ok(veil < z("#sudo"), "the sudo prompt must stay above the veil");
  assert.ok(veil < z("#forbidden"), "the 403 modal must stay above the veil");
});

test("veil: NO instance declares a z-index of its own", () => {
  // ⬜ This replaces `veil < z("#splash")`, which the 2026-08-23 merge made
  // self-contradictory: it demanded the boot splash sit ABOVE the veil, and the merge's
  // whole point is that they are one object at one height.
  //
  // ⭐ The replacement guards the DEFECT rather than the old arrangement. The splash used
  // to carry `z-index:80` — above the six modals at 60 — with no stated reason, so on a
  // first boot the sharing dialog (opened the moment the plan arrives) sat invisible AND
  // unclickable behind it for the whole scan, and for the 120 s of the safety net if the
  // scan never finished. A per-instance z-index is exactly how that comes back.
  for (const id of ["#splash", "#steps-refresh"]) {
    const rules = css.match(new RegExp(`${id}[^{]*\\{[^}]*\\}`, "g")) || [];
    for (const r of rules) {
      assert.ok(
        !/z-index/.test(r),
        `${id} declares its own z-index — height belongs to .veil, for both instances`,
      );
    }
  }
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
