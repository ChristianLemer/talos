// Structural guards on the ladder WIDGET, in the same spirit as veil.test.mjs and
// forbidden-ui.test.mjs — and for the same reason: the four defects below live in `app.js`
// and `index.html`, which are DOM-bound and imported by no test, and each is structural.
// This repo carries zero JS dependencies on purpose, so a DOM library is not on the table
// and reading the source as text catches these exactly.
//
// The behavioural half is elsewhere and cannot regress to any of these: ladder.test.mjs
// pins the rule and `rungFromInput`'s output, model.test.mjs pins the per-rung count. What
// none of them can see is whether the widget CALLS them — the widget is where the lie would
// be visible to a user and invisible to a test.
//
// ⚠️ The visual half — that the fill actually reads grey→green on screen, that the word is
// legible at every detent — was verified in the real Talos.app, at all five positions.
// These assertions pin the DECISIONS, not the rendering.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");

test("ladder: the rung crosses the wire through rungFromInput, never as `.value`", () => {
  // THE defect that would make the widget lie. `server::rung_from_wire` reads the field
  // with serde's `as_u64`, which rejects a JSON string AND a JSON float, and its None arm
  // lands on `Rung::Everything`. So `rung: e.target.value` (a string) or `Number(v)` on a
  // "2.0" would run EVERYTHING while the label read "Unattended · walk away" — doing more
  // than asked, silently, which is the direction a user cannot detect until it is done.
  const handler = appjs.match(/getElementById\("ladder-range"\)\s*\.addEventListener\(\s*"input"[\s\S]{0,300}?\}\);/);
  assert.ok(handler, "the range input must be wired on `input` (live, not on release)");
  assert.match(handler[0], /rung = L\.rungFromInput\(/,
    "the string `.value` must go through ladder.js's parse+clamp, nothing else");
  // And nothing anywhere may assign the raw value to `rung`.
  assert.ok(
    !/rung = (e\.target|r)\.value/.test(appjs),
    "a raw `.value` is a STRING on the wire → the server reads Everything",
  );
});

test("ladder: applyAll sends `rung`, beside `unmanaged` and for the same reason", () => {
  // A front-only filter would be cosmetic (`row_action` never sees the wire lists — the
  // Git-hazard lesson), so the rung MUST be told to the server, which builds the plan.
  const at = appjs.indexOf('type: "apply"');
  assert.notEqual(at, -1, "the apply message must exist");
  const msg = appjs.slice(at, appjs.indexOf("}));", at));
  assert.match(msg, /\brung,/, "the apply message must carry the rung");
  assert.match(msg, /unmanaged:/, "…beside unmanaged, which is sent for the same reason");
});

test("ladder: the fill is grey → GREEN, never red → green", () => {
  // Red is the diff language's word for "would remove" on every row and every button. NO
  // position of this control changes how much gets removed — `uninstall` is on every rung,
  // including 0 — so a red end would encode an axis the widget does not have. (The plan's
  // stated reason, "the leftmost rung removes nothing", is false; the conclusion survives
  // the correction, the premise does not. See index.html's #ladder rules.) Green already
  // means "a plain Apply would install/update this"; at this scale it means "more will be
  // applied", which is the same meaning at a different size, not a competing one.
  const rule = html.match(/#ladder-range\s*\{[^}]*\}/);
  assert.ok(rule, "#ladder-range needs a rule");
  assert.match(rule[0], /linear-gradient/, "the fill is one gradient stopped at --fill%");
  assert.match(rule[0], /#9ece6a/, "the same green the rows and buttons already use");
  assert.match(rule[0], /var\(--line\)/, "and plain grey for the part not included");
  // The red the diff language reserves for removal, in any of its shipped forms.
  for (const red of ["#f7768e", "red", "#ff"]) {
    assert.ok(!rule[0].includes(red), `the ladder fill must not use ${red} — red means remove`);
  }
  // The green must come FIRST in the gradient: reversed, the track would empty as you
  // asked for more, which reads as the opposite of what the control does.
  const stops = rule[0].slice(rule[0].indexOf("linear-gradient"));
  assert.ok(stops.indexOf("#9ece6a") < stops.indexOf("var(--line)"),
    "green is the LEFT half of the gradient (0% → --fill), grey the right");
});

test("ladder: the word, the emoji and the COUNT all read the same rung", () => {
  // The count is the only number on screen, and it is the reason the widget exists — "how
  // many, before I commit". Two live mutation survivors motivated this: `rungPlan(model, 4)`
  // and `rungPlan(model, rung + 1)` both passed everything else, and either would put a
  // number under a word it does not belong to — the widget saying "Unattended · 12 items"
  // while rung 2 touches 3. Worse than no count, because it is confidently wrong.
  const at = appjs.indexOf("function renderLadder()");
  assert.notEqual(at, -1, "renderLadder() must exist");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /const spec = L\.RUNGS\[rung\]/, "the word comes from the chosen rung");
  assert.match(body, /M\.rungPlan\(model, rung\)/,
    "…and so must the count: any other argument decorates the wrong word");
  // The fill too: it is the same one number, so it must be derived from it and not tracked
  // separately (a second source of truth would drift silently).
  assert.match(body, /setProperty\("--fill", `\$\{\(rung \//,
    "the fill percentage is computed from `rung`, not stored beside it");
});

test("ladder: it repaints from refreshLiveness — the one 'anything changed' hook", () => {
  // A count that goes stale is worse than no count: the user reads "3 items", toggles a
  // row, and the number still says 3. Every path that can change what a rung would touch
  // (a toggle, a scan verdict, an `outdated` pill, a Reset) already routes through
  // refreshLiveness, so the ladder hangs off that rather than off each caller.
  const at = appjs.indexOf("function refreshLiveness()");
  assert.notEqual(at, -1);
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.ok(body.includes("renderLadder()"), "refreshLiveness must repaint the ladder");
  // …and the widget must freeze with everything else: acting on an incomplete scan would
  // count un-probed rows as absent, and moving the rung mid-run would mean nothing.
  const rl = appjs.slice(appjs.indexOf("function renderLadder()"));
  assert.match(rl.slice(0, rl.indexOf("\n}\n")), /disabled = applyRunning \|\| scanning/);
});

test("ladder: the announcement is the WORD, not the number under it", () => {
  // A range input announces its `value`. On this control that is "4" — the one thing on
  // screen that carries no meaning at all, since the whole point of the widget is that
  // position alone does not read (the lesson the scope work paid for). `aria-valuetext`
  // overrides it with the reading a sighted user gets.
  //
  // Pinned because it is invisible: nothing about the app LOOKS wrong if this line is
  // deleted, so only a test notices. Set inside renderLadder rather than in the markup
  // because it changes on every move, exactly like the text it mirrors.
  const at = appjs.indexOf("function renderLadder()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /setAttribute\("aria-valuetext"/,
    "renderLadder must override the announced value with the word and the count");
  assert.match(body, /aria-valuetext",\s*`\$\{spec\.name\}, \$\{count\}`/,
    "…and it must be the SAME name and count the eye gets, not a second wording");
});
