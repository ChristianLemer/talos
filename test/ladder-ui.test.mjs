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

test("dial: the three buttons are NESTED in the DOM, not painted to look nested", () => {
  // ⭐ The containment is the whole message: 📦's frame starts left of ⚡ and runs to the end,
  // so the eye reads "Apps contains the other two" with no copy at all. Real nesting is what
  // makes that impossible to break — two frames drawn side by side would drift the day someone
  // touches a padding, and a screen reader would walk a structure the eye does not see.
  const dial = html.match(/<div id="dial"[\s\S]*?<\/div>\s*<!--/);
  assert.ok(dial, "#dial must exist");
  const at = (s) => dial[0].indexOf(s);
  // ⚡ inside 🧩 inside 📦, checked by position rather than by parsing: each frame's opening
  // tag must precede the next, and all three buttons must sit inside the outermost.
  assert.ok(at("frame-apps") < at("frame-ext"), "🧩's frame opens inside 📦's");
  assert.ok(at("frame-ext") < at("frame-cfg"), "⚡'s frame opens inside 🧩's");
  // ⚠️ Buttons cannot nest in HTML — a <button> inside a <button> is invalid and browsers
  // unnest it silently, which would flatten the whole design. So the FRAMES are divs and each
  // rung's clickable surface is its own button.
  // ⚠️ Checked by BALANCE, not by a regex looking for a nested tag: `<button …>[\s\S]*?<button`
  // matches any two buttons in the block, however far apart, so it flagged three correct
  // siblings. Walking the tags is what actually answers "is one inside another".
  let depth = 0;
  for (const tag of dial[0].match(/<button|<\/button>/g) ?? []) {
    depth += tag === "<\/button>" ? -1 : 1;
    assert.ok(depth <= 1, "no button may contain a button — browsers unnest them silently");
  }
  assert.equal(depth, 0, "every button must be closed");
  for (const r of [0, 1, 2]) {
    assert.match(dial[0], new RegExp(`data-rung="${r}"`), `rung ${r} needs a button`);
  }
});

test("dial: one button per rung, derived from RUNGS", () => {
  // The one thing in this feature that cannot derive from RUNGS is the MARKUP — three buttons
  // are written by hand. So a rung added or removed without touching the html leaves a scope
  // unreachable, silently. Same guard the slider's `max` used to carry, for the same reason.
  const buttons = [...html.matchAll(/data-rung="(\d+)"/g)].map((m) => Number(m[1]));
  assert.deepEqual(
    buttons.sort(),
    RUNGS.map((_, i) => i),
    `the markup has ${buttons.length} buttons but RUNGS has ${RUNGS.length} rungs`,
  );
});

test("dial: the SECOND click applies — and only on the same, armed, non-empty scope", () => {
  // ⭐ THE load-bearing behaviour of the whole control, and every clause of it is a decision:
  //   - a different scope DISARMS, which is what makes the first click reversible without
  //     inventing a cancel affordance — and the safety property: you cannot arm 🧩 and then
  //     run 📦 with one click.
  //   - an armed EMPTY scope refuses, quietly, rather than starting a run with nothing in it.
  //   - only the armed path reaches applyAll.
  const at = appjs.indexOf("function onDialClick(");
  assert.notEqual(at, -1, "onDialClick must exist — one place decides what a click means");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /if \(r !== rung\)/, "a different scope must take its own branch");
  assert.match(body, /armed = false/, "…and disarm");
  assert.match(body, /if \(!armed\)/, "the first click on the chosen scope arms it");
  assert.match(body, /armed = true/);
  assert.match(body, /rungPlan\(model, rung\)\.length === 0/, "an empty scope must be refused");
  assert.match(body, /applyAll\(\)/, "and the armed second click runs");
  // The order matters: the empty check must come BEFORE applyAll, or a click on an empty
  // armed scope would start a run that has nothing to do — a full server round-trip to be
  // told `done {nothing:true}`, which this app already paid for once at the 403 retry card.
  assert.ok(
    body.indexOf("length === 0") < body.indexOf("applyAll()"),
    "the empty refusal must precede the run",
  );
  // Frozen while busy, like every other control: acting on an incomplete scan would count
  // un-probed rows as absent.
  assert.match(body, /applyRunning \|\| scanning/, "a click during a run or a scan does nothing");
});

test("dial: `armed` is not the same state as `rung`", () => {
  // A scope is ALWAYS chosen (the dial has no null state), so if `armed` were the same thing
  // the very first paint would be a loaded gun: one click anywhere would install. Two
  // variables, and the initial value of the second is what makes the control safe at rest.
  assert.match(appjs, /let armed = false;/, "armed must start false, and be its own variable");
});

test("dial: the third line becomes a VERB when armed, and names the removals", () => {
  // ⚠️ Without this the second click reads as a double-click that missed — and it installs
  // software. The count turning into "▶ apply 7 · remove 2" is what distinguishes them.
  //
  // ⭐ And the removals are named SEPARATELY, because Apply does not only act on the scope: it
  // also honours the rows the user unchecked. A button that said "Extensions" and quietly
  // removed three things would be the button lying.
  const at = appjs.indexOf("function renderDial(");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /armed/, "the label must know whether this click would run");
  assert.match(body, /▶ apply/, "an armed scope reads as a verb");
  assert.match(body, /remove \$\{removes\}/, "…and names the removals it would also do");
  assert.match(body, /actionOf\(model, i\) === "uninstall"/,
    "the removal count comes from the ACTION, not from a guess about the row");
  // The counts come from rungPlan — the same function the server's filter mirrors — so the
  // button and the plan cannot disagree about what it would touch.
  assert.match(body, /M\.rungPlan\(model, r\)/, "the count must come from the shared rule");
});

test("dial: an empty scope LOOKS unclickable and says so", () => {
  // A green button with nothing in it, clicked, does nothing — and a gesture that leaves no
  // trace is a defect in this app, not a no-op. So the state is visible in three ways: the
  // `.empty` class, the words on the button, and the note under the dial.
  const at = appjs.indexOf("function renderDial(");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /classList\.toggle\("empty"/, "an empty chosen scope must be marked");
  assert.match(body, /nothing to do/, "…and say so on the button itself");
  // ⚠️ NOT disabled: the other two buttons are how a user picks a wider scope, and disabling
  // the chosen one would remove the affordance that fixes the situation.
  assert.match(body, /btn\.disabled = busy;/,
    "only busy disables a button — an empty scope stays clickable so the scope can be changed");
  // And the CSS must actually mute it, or the class is decoration.
  assert.match(html, /\.dial-btn\.chosen\.empty/, "the empty state needs a rule of its own");
});

test("dial: the chosen scope is PLAIN BLUE — not green, not red", () => {
  // ⭐ C: "do not change to green… just use the blue that is fine (plain blue)". And it is the
  // better call for a reason worth keeping: #9ece6a means "a plain Apply would do this" on every
  // ROW of the panel, so a green button sitting above green rows would read as one more verdict
  // rather than as the control that CAUSES them. Blue is the app’s "this is what you adjust"
  // hue, so the chosen scope becomes the SOLID form of what it was outlined in — nothing new
  // enters the palette and no colour changes meaning.
  const rules = html.match(/\.dial-btn \{[\s\S]*?\.dial-btn\.chosen\.empty \.dial-where[^}]*\}/);
  assert.ok(rules, "the dial’s rules must exist");
  assert.match(rules[0], /\.dial-btn\.chosen \{[^}]*background:#3d59a1/,
    "the chosen button is FILLED with the selection blue");
  // ⚠️ Neither verdict colour may appear: green is "would install", red is "would remove", and
  // the dial is the control, not a verdict about a package.
  for (const forbidden of ["#9ece6a", "#f7768e"]) {
    assert.ok(!rules[0].includes(forbidden),
      `the dial must not use ${forbidden} — that colour already means something about a ROW`);
  }
  // The frames carry the same blue as an outline, which is what makes "chosen" read as the same
  // thing filled in rather than as a different state.
  assert.match(html, /\.dial-frame[^}]*#3d59a1/, "the frames carry the selection blue");
});

test("dial: the button announces all three lines, and whether it would run", () => {
  // A <button> announces its text content, which here is three separate spans — a screen
  // reader would recite them without the relation between them. aria-label carries the same
  // reading the eye gets, INCLUDING "click again to run", which is the one thing a blind user
  // cannot infer from a colour.
  const at = appjs.indexOf("function renderDial(");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /setAttribute\(\s*"aria-label"/, "each button must carry an aria-label");
  assert.match(body, /Click again to run/, "…and say when the next click acts");
  assert.match(body, /aria-pressed/, "and which scope is chosen, as state not as colour");
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

test("dial: Apply is GONE from the top bar, and nothing still reaches for it", () => {
  // ⚠️ The dangerous half of removing a button is not the markup — it is a leftover
  // `getElementById("install-all").onclick`, which throws on a null and takes every listener
  // registered AFTER it down with it. That is how a removed Apply button silently unwires Quit.
  assert.ok(!/id="install-all"/.test(html), "the Apply button must be gone from the bar");
  // ⚠️ Comments are stripped first: the note that REPLACED the listener names it on purpose,
  // so a naive search matches the very documentation of the removal.
  const code = appjs.replace(/\/\/.*$/gm, "");
  assert.ok(
    !/getElementById\("install-all"\)\s*\./.test(code),
    "nothing may dereference the removed button — a null here kills the listeners below it",
  );
  // applyAll itself stays: it is the single entry point, now called by the dial's second click.
  assert.match(appjs, /function applyAll\(\)/, "applyAll must remain the one way to run");
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
