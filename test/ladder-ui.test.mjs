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
import { RUNGS } from "../public/ladder.js";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");

test("dial: the ZONE is the button, and the zones are really nested", () => {
  // ⭐ C's correction of a first version where each zone merely CONTAINED a button: the zone
  // itself is clickable. That is what lets choosing the outer one light the inner ones, which is
  // how the inclusion gets PAINTED instead of only drawn.
  const dial = html.match(/<div id="dial"[\s\S]*?\n        <\/div>/);
  assert.ok(dial, "#dial must exist");
  const m = dial[0];
  // Real DOM nesting, checked by position: 📦 opens, then 🧩 inside it, then ⚡ inside that.
  assert.ok(m.indexOf("zone-apps") < m.indexOf("zone-ext"), "🧩's zone opens inside 📦's");
  assert.ok(m.indexOf("zone-ext") < m.indexOf("zone-cfg"), "⚡'s zone opens inside 🧩's");
  // Each zone carries its own rung AND its own clickable role — that is the difference from the
  // version this replaced, where the frames were inert wrappers.
  for (const r of [0, 1, 2]) {
    assert.match(m, new RegExp(`data-rung="${r}"`), `rung ${r} needs a zone`);
  }
  assert.equal((m.match(/role="button"/g) ?? []).length, 3, "all three zones are buttons");
  // ⚠️ A div with role=button gets no keyboard activation for free. Buttons cannot nest in HTML
  // (browsers unnest them silently), so this is the unavoidable cost — and tabindex is what
  // keeps the control reachable at all without a mouse.
  assert.equal((m.match(/tabindex="0"/g) ?? []).length, 3, "each zone must be focusable");
  assert.ok(!/<button/.test(m), "no real <button> here — they cannot nest, hence role=button");
});

test("dial: one zone per rung, derived from RUNGS", () => {
  // The one thing in this feature that cannot derive from RUNGS is the MARKUP — three zones are
  // written by hand. A rung added or removed without touching the html leaves a scope
  // unreachable, silently. Same guard the slider's `max` used to carry, for the same reason.
  const zones = [...html.matchAll(/class="zone zone-\w+" data-rung="(\d+)"/g)].map((x) =>
    Number(x[1]),
  );
  assert.deepEqual(
    zones.sort(),
    RUNGS.map((_, i) => i),
    `the markup has ${zones.length} zones but RUNGS has ${RUNGS.length} rungs`,
  );
});

test("dial: choosing a scope lights it AND every zone inside it", () => {
  // ⭐ THE property C asked for, and the reason the colour matters at all: with three zones lit,
  // "📦 takes the other two with it" is read from the FILL rather than inferred from the geometry.
  // `r <= rung` is the whole rule, derived from the rung NUMBER — not from the DOM — so it comes
  // from the same source as the counts and cannot disagree with them.
  const at = appjs.indexOf("function renderDial(");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /const on = r <= rung;/,
    "a zone is lit when it is the chosen one OR inside it — that is the cumulation, painted");
  assert.match(body, /classList\.toggle\("on", on\)/, "…and the class follows that verdict");
  // ⚠️ ONE class, not two. There was an `armed` class for a while, when the dial's own second click
  // was the run; with Apply back in the bar there are only TWO states — in the scope or out of it —
  // so a second class would be an alias for the first and the next reader would hunt for a
  // difference that does not exist.
  assert.ok(
    !/classList\.toggle\("armed"/.test(body),
    "no `armed` class: the run left the dial, so the state it marked left with it",
  );
});

test("dial: a click only SELECTS — the run belongs to the Apply button", () => {
  // ⭐ C, once Apply was back: "ils ne peuvent pas déclencher le apply… il n'y a apply qui peut
  // fonctionner". This replaces a two-click arming scheme, and dropping it is safer as well as
  // simpler: a second click on the same spot is what people do when the first appears not to have
  // worked, and there it installed software. The irreversible act now has exactly one door.
  const at = appjs.indexOf("function onZoneClick(");
  assert.notEqual(at, -1, "onZoneClick must exist — one place decides what a click means");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /rung = r;/, "the clicked zone becomes the scope");
  assert.match(body, /refreshLiveness\(\)/, "…and the whole panel repaints, rows included");
  // ⚠️ THE load-bearing absence: no run may be reachable from a zone click.
  assert.ok(!/applyAll\(\)/.test(body), "a zone click must never start a run");
  assert.ok(!/armed/.test(body), "and there is no arming left to do");
  assert.match(body, /applyRunning \|\| scanning/, "a click during a run or a scan does nothing");
});

test("dial: a click resolves the INNERMOST zone, never an ancestor", () => {
  // ⚠️ The price of "the zone is the button": the zones are nested, so a click on ⚡ also lands on
  // 🧩 and 📦 as it bubbles. Three separate listeners would fire three times and the outermost
  // would win — clicking the small frame would choose 📦, the exact opposite of what the eye
  // picked. One delegated listener + `closest` from the real target is the fix.
  assert.match(appjs, /getElementById\("dial"\)\?\.addEventListener\(\s*"click"/,
    "ONE delegated listener on the container, not one per zone");
  assert.match(appjs, /e\.target\.closest\?\.\(".zone"\)/,
    "the innermost zone under the pointer is what decides — closest, from the event target");
  // The label must not intercept: a click on the text has to belong to its zone, and pointer
  // events on a child could hand it to whichever ancestor the browser reports.
  assert.match(html, /\.zone-label[^}]*pointer-events:none/,
    "the label is transparent to the pointer so the ZONE always owns the click");
  // Keyboard too — a div with role=button activates on nothing by default.
  assert.match(appjs, /"keydown"/, "Enter/Space must work: role=button gets nothing for free");
  assert.match(appjs, /e\.preventDefault\(\)/, "…and Space must not scroll the panel");
});

test("dial: an empty scope LOOKS unclickable and says so", () => {
  // A filled zone with nothing in it, clicked, does nothing — and a gesture that leaves no trace
  // is a defect in this app, not a no-op. So the state is visible three ways: the class, the
  // words on the zone, and the note under the dial.
  const at = appjs.indexOf("function renderDial(");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /classList\.toggle\("empty"/, "an empty chosen scope must be marked");
  assert.match(body, /nothing to do/, "…and say so on the zone itself");
  // ⚠️ NOT disabled: the other zones are how a user picks a wider scope, so making the chosen one
  // inert would remove the affordance that fixes the situation.
  assert.ok(
    !/zone\.disabled = /.test(body),
    "an empty scope stays clickable so the scope can still be changed",
  );
  assert.match(html, /\.zone\.on\.empty/, "the empty state needs a rule of its own");
});

test("dial: a chosen zone is PLAIN BLUE — not green, not red", () => {
  // ⭐ C: "do not change to green… just use the blue that is fine (plain blue)". The better call,
  // and the reason is worth keeping: #9ece6a means "a plain Apply would do this" on every ROW of
  // the panel, so a green zone above green rows reads as one more verdict rather than as the
  // control that CAUSES them. Blue is the app's "this is what you adjust" hue, so a lit zone is
  // simply the SOLID form of its own outline.
  const rules = html.match(/\.zone \{[\s\S]*?\.dial\.busy \.zone[^}]*\}/);
  assert.ok(rules, "the zone rules must exist");
  // ⭐ A RAMP, light inside → dark outside, and it fixes a defect the rendered panel exposed: one
  // flat blue merged all three lit zones into a single rectangle and the NESTING vanished exactly
  // when it mattered most, at the widest scope. Asserted as an ORDER rather than as three literal
  // values, so the palette can be retuned without touching this — what must hold is that each
  // level differs from its parent.
  const shade = (cls) => rules[0].match(new RegExp(`\\.zone-${cls}\\.on \\{[^}]*background:(#[0-9a-f]{6})`))?.[1];
  const [cfg, ext, apps] = ["cfg", "ext", "apps"].map(shade);
  assert.ok(cfg && ext && apps, "each level needs its own lit shade");
  assert.equal(new Set([cfg, ext, apps]).size, 3, "three DISTINCT shades, or the nesting merges");
  // ⚠️ DARK INSIDE → LIGHT OUTSIDE, and this direction was measured rather than chosen: the zones
  // are 113px / 234px / 1050px wide, so the innermost has a fraction of the surface. A pale fill
  // on 113px reads as an OUTLINE — clicking ⚡ looked like nothing had turned on at all. Small and
  // dark shows; small and pale disappears. Area beats hue, which is why the first ramp (light
  // inside) had to be reversed.
  //
  // Pinned as a RELATION via a brightness sum, so the palette can be retuned without touching
  // this — what must hold is the direction.
  const lum = (h) => parseInt(h.slice(1, 3), 16) + parseInt(h.slice(3, 5), 16) + parseInt(h.slice(5), 16);
  assert.ok(lum(cfg) < lum(ext) && lum(ext) < lum(apps),
    `dark inside → light outside: got ⚡${cfg} 🧩${ext} 📦${apps}`);
  assert.match(rules[0], /\.zone \{[^}]*#3d59a1/, "and the outline stays the app's selection blue");
  // ⚠️ Red must not appear at all: red is the diff language's word for "would remove", and the
  // dial removes nothing by itself.
  assert.ok(!rules[0].includes("#f7768e"), "the dial must not use red — red means remove");
  // ⭐ NO GREEN AT ALL, and that is C's decision after seeing it: Apply is a BUTTON again, and
  // green already means "a plain Apply would do this" on every ROW — a green scope would compete
  // with both. The selected scope is the same blue at FULL strength instead, so the whole dial
  // stays in the family of "this is what you adjust".
  // ⚠️ Comments stripped first — for the THIRD time today a text-level guard matched its own
  // documentation: the rule that explains why the row green is not used had to name it.
  const decls = rules[0].replace(/\/\*[\s\S]*?\*\//g, "");
  assert.ok(!decls.includes("#9ece6a"),
    "the dial must not use the row green — vivid blue carries selection instead");
  // ⚠️ ONE ramp, not two. There were two for a while — muted for "included", vivid for "armed" —
  // and the second went with the arming when the run moved to the Apply button. Two states only:
  // in the scope (this ramp) or out of it (the base rule, barely there).
});

test("dial: it repaints from refreshLiveness — the one 'anything changed' hook", () => {
  // A count that goes stale is worse than no count: the user reads "3 items", toggles a row,
  // and the number still says 3. Every path that changes what a scope would touch (a toggle, a
  // scan verdict, an `outdated` pill, a Reset) already routes through refreshLiveness.
  const at = appjs.indexOf("function refreshLiveness()");
  assert.notEqual(at, -1);
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.ok(body.includes("renderDial()"), "refreshLiveness must repaint the dial");
});

test("dial: Apply is BACK in the bar, and both paths reach the same run", () => {
  // ⭐ C, after using it: "j'ai quand même un bouton apply". This test asserted the OPPOSITE an
  // hour ago, and the reversal is the decision: folding "how far" and "go" into one object made
  // the dial's second click carry an irreversible act that looked like a double-click, and left
  // the panel with no single place meaning "do it".
  assert.match(html, /id="install-all"/, "the Apply button must exist again");
  assert.match(appjs, /getElementById\("install-all"\)\.onclick = \(\) => applyAll\(\)/,
    "…and be wired");
  // ⭐ AND IT IS THE ONLY DOOR. C: "il n'y a apply qui peut fonctionner" — the dial's own second
  // click used to run too, and removing that is what makes this test the inverse of the one it
  // replaced. One place performs the irreversible act, and it is the one labelled with the verb.
  assert.match(appjs, /function applyAll\(\)/, "applyAll stays the single way to run");
  const clickAt = appjs.indexOf("function onZoneClick(");
  const clickBody = appjs.slice(clickAt, appjs.indexOf("\n}\n", clickAt));
  assert.ok(!/applyAll\(\)/.test(clickBody), "a zone click must NOT run — only Apply does");
  // The button must refuse an empty scope, asking the SAME question the dial asks — a second rule
  // here is how the button and the zones would come to disagree about what "nothing" means.
  const live = appjs.indexOf("function refreshLiveness()");
  const liveBody = appjs.slice(live, appjs.indexOf("\n}\n", live));
  assert.match(liveBody, /M\.rungPlan\(model, rung\)\.length === 0/,
    "emptiness comes from the shared rule, not from a copy");
  assert.match(liveBody, /g\.disabled = busy \|\| scopeEmpty/, "and an empty scope disables it");
  // ⚠️ It applies the SELECTED scope, not everything — so the tooltip has to name which, or the
  // word "Apply" quietly means something narrower than it says.
  assert.match(liveBody, /L\.RUNGS\[rung\]\.name/, "the title must name the scope it would run");
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

test("ladder: a row out of reach is DIMMED, never hidden", () => {
  // "Every gesture leaves a trace" has been paid for twice in this app — a cancelled row
  // that VANISHED was a real bug, twice — and a plan that silently drops rows is that same
  // defect at a larger scale: the user would have no way to learn that a package exists but
  // costs too much for this rung. Dimming is the whole point: moving the slider teaches
  // which packages cost what.
  const rule = html.match(/details\.rung-out\s*\{[^}]*\}/);
  assert.ok(rule, "details.rung-out needs a rule");
  assert.match(rule[0], /opacity:/, "dimmed…");
  assert.ok(!/display\s*:\s*none/.test(rule[0]), "…and NOT hidden");
  assert.ok(!/visibility\s*:\s*hidden/.test(rule[0]), "…nor invisible, which is the same thing");
  // And it must actually dim: the .plan-add/.plan-remove rule sets opacity:1 at the same
  // specificity, so `details.rung-out` has to come AFTER it in the sheet or it loses — and
  // an in-plan row is the ONLY kind that can be out of a rung's reach, so losing there means
  // losing everywhere. (Live mutation survivor: moved above, every test still passed and
  // nothing dimmed on screen.)
  assert.ok(html.indexOf("details.plan-add, details.plan-remove { opacity:1; }")
      < html.indexOf("details.rung-out {"),
    "details.rung-out must come after the plan rows' opacity:1, or it never applies");
  // The row's DOM does not change shape as the slider moves — the label exists on every row
  // and is revealed by the class, like .untouched-note. An empty flex item would otherwise
  // widen every row by summary's gap.
  assert.match(html, /\.rung-why \{ display:none; \}/,
    "the reason label is hidden until .rung-out, not created and destroyed");
});

test("ladder: rungReason reads the KIND and no fact at all", () => {
  // ⭐ THIS TEST REPLACES a shape guard that pinned a `while` loop walking down the rungs to
  // DERIVE which fact held a row back. That loop was a measured mutation survivor's antidote:
  // `if (p?.slow) return "slow"` was exactly equivalent under the old table, so only the shape
  // could be pinned, not the behaviour.
  //
  // The loop is gone because the question is: no fact can hold a row out of a rung any more, so
  // there is no fact to derive. What must be pinned instead is the inverse — that rungReason
  // reads NO fact whatsoever. A fact creeping back in here would caption a row with something
  // the rule does not act on, which is the same class of lie in a new shape.
  //
  // Read from model.js (not app.js) because that is where the rule layer lives.
  const modeljs = readFileSync(new URL("../public/model.js", import.meta.url), "utf8");
  const at = modeljs.indexOf("export function rungReason(");
  assert.notEqual(at, -1, "rungReason must exist in model.js, beside rungPlan");
  const body = modeljs.slice(at, modeljs.indexOf("\n}\n", at));
  // Comments are stripped first: they legitimately NAME the facts to explain why they left.
  const code = body.replace(/\/\/.*$/gm, "");
  for (const fact of ["slow", "uac", "forbidden"]) {
    assert.ok(
      !new RegExp(`\\b${fact}\\b`).test(code),
      `rungReason must not read \`${fact}\` — the rungs no longer sort by it, so a caption \
built on it would describe something the rule does not do`,
    );
  }
  // It must still ASK the rule rather than restate it: a copied table would eventually put a
  // reason on a row that IS in the plan.
  assert.match(code, /rungAllows\(/, "the verdict comes from the rule, never from a second copy");
  // And the two things it MAY read are the classes, which is what the rungs are now built on.
  assert.match(code, /isExtension/, "the kind is what decides, so the kind is what it reads");
});

test("dial: the note under it names WHERE the work is, and only when that helps", () => {
  // C, at the second sighting of the old slider: "le apply reste actif même s’il semble ne plus
  // rien y avoir à exécuter — est-ce normal?". It was not, and the answer survives the widget
  // that prompted it: a control that offers an action with nothing to do is the dishonesty this
  // app already paid for once (the 403 retry card that promised a download and did nothing).
  //
  // The button half is covered above (`.empty`, "nothing to do", still clickable so the scope
  // can be widened). What is left here is the NOTE, and its condition.
  const at = appjs.indexOf("function renderDial(");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  // Emptiness comes from rungPlan, never a second rule: the button, the note and the server’s
  // own filter have to agree about what "nothing" means.
  assert.match(body, /M\.rungPlan\(model, rung\)\.length === 0/,
    "emptiness is asked of the shared rule — a second one would drift from the count");
  // ⚠️ Shown ONLY when a wider scope would actually do something. With nothing to do at all,
  // the empty panel is already the message and a note would be noise.
  assert.match(body, /wider > 0/, "the note appears only when a wider scope has work");
  assert.match(body, /further right/, "…and it names where that work is");
  assert.match(body, /blocked\.hidden = !show/, "otherwise it is hidden, not left stale");
});
