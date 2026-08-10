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
  // It must be the SAME name and count the eye gets — the variables, never a second literal.
  // (The estimate joined this reading later; its own test pins the full shape, including the
  // no-estimate fallback. This one stays about the name and the count, which are never absent.)
  const vt = body.slice(body.indexOf('setAttribute("aria-valuetext"'));
  assert.match(vt.slice(0, vt.indexOf("\n")), /\$\{spec\.name\}, \$\{count\}/,
    "…and it must be the SAME name and count the eye gets, not a second wording");
});

test("ladder: moving the rung repaints the ROWS, not only the widget", () => {
  // THE stale-panel bug, and the reason this task exists at all. The widget said "3 items" —
  // a number — while the truth sat on screen in rows that did not move, and a summary that
  // contradicts what it summarises is worse than no summary. C: "j'aurais voulu que les
  // packages en dessous réagissent".
  //
  // `renderLadder()` repaints the word, the fill and the count and NOTHING about a row, so
  // the handler must call `refreshLiveness()` — the "anything changed" hook, which repaints
  // every row and then calls renderLadder itself. Pinned as source text because the failure
  // is invisible to every other test: the count would still be right.
  const handler = appjs.match(
    /getElementById\("ladder-range"\)\s*\.addEventListener\(\s*"input"[\s\S]{0,300}?\}\);/,
  );
  assert.ok(handler, "the range input must be wired on `input`");
  assert.match(handler[0], /refreshLiveness\(\)/,
    "the rung handler must repaint the ROWS — renderLadder alone leaves the panel stale");
  // And refreshLiveness must in fact be the hook that paints the third state, or the line
  // above would be satisfied by a function that no longer does it.
  const at = appjs.indexOf("function refreshLiveness()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /setRungOut\(i, M\.rungReason\(rung,/,
    "…and refreshLiveness is where a row learns whether it is out of the rung's reach");
});

test("ladder: `rung-out` is DERIVED from rungReason, never from a second rule", () => {
  // A copied rule would drift and eventually dim a row that IS in the plan — the one failure
  // that turns the dimming from a hint into a lie. model.js's rungReason asks `rungAllows`
  // and is exhaustively pinned against it in ladder.test.mjs, so app.js must ask IT and must
  // not read a fact of its own.
  const at = appjs.indexOf("function setRungOut(");
  assert.notEqual(at, -1, "setRungOut() must exist — one place sets the class and the word");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /classList\.toggle\("rung-out", !!why\)/,
    "the class follows the reason exactly: no reason → not dimmed");
  assert.match(body, /r\.why\.textContent = why \? RUNG_WHY\[why\] : ""/,
    "and the WORD comes from the same reason, so the two cannot disagree");
  // No second rule anywhere in app.js: the facts must not be consulted to decide dimming.
  const paint = appjs.slice(appjs.indexOf("function refreshLiveness()"));
  const upTo = paint.slice(0, paint.indexOf("\n}\n"));
  for (const fact of ["\\.slow", "\\.uac", "\\.forbidden"]) {
    assert.ok(!new RegExp(`rung-out[\\s\\S]{0,200}${fact}`).test(upTo),
      `the dimming must not read ${fact} directly — that is rungAllows's job`);
  }
  // The reason keys and the words must be in lockstep: an unknown key renders an empty
  // label, i.e. a dimmed row that explains nothing.
  const why = appjs.match(/const RUNG_WHY = \{[^}]*\}/);
  assert.ok(why, "RUNG_WHY must hold the user-facing words");
  // ⭐ TWO keys where there were five. "slow", "hand" and "blocked" left this map when the
  // rungs that sorted by those facts were cut — a fact can no longer hold a row out of a rung,
  // so it can no longer be the reason one is dimmed. They did not leave the APP: see ROW_FACTS,
  // which shows them on every row at every rung.
  for (const k of ["extension", "app"]) {
    assert.match(why[0], new RegExp(`\\b${k}:`), `RUNG_WHY needs a word for "${k}"`);
  }
  // And the facts' words are still said in the USER's terms, not the mechanism's — that
  // discipline moved to ROW_FACTS with them.
  const facts = appjs.match(/const ROW_FACTS = \{[\s\S]*?\n\}/);
  assert.ok(facts, "ROW_FACTS must hold the per-row fact words");
  assert.match(facts[0], /asks for your password/, "the uac case must be in the user's terms");
  assert.match(facts[0], /uac: \{/, "…keyed by the wire field, which is where `uac` may appear");
  for (const k of ["uac", "forbidden", "slow"]) {
    assert.match(facts[0], new RegExp(`${k}: \\{`), `ROW_FACTS needs an entry for "${k}"`);
  }
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

test("ladder: the control OWNS its zone — full width, framed in the selection blue", () => {
  // C: "je voudrais que le curseur prenne la totalité en large, parce que c'est quand même la
  // partie la plus importante pour les gens… elle devrait être encadrée dans une zone bleue".
  // The reasoning holds: this one control decides what every row below does, so it must
  // out-rank them visually, and BLUE is the only hue free to mean "this is what you ADJUST" —
  // green ("would add"), red ("would remove") and grey ("untouched") are spoken for by the
  // diff, so a control tinted with a verdict colour would read as a verdict about a package.
  const range = html.match(/#ladder-range \{[\s\S]*?\}/);
  assert.ok(range, "#ladder-range needs a rule");
  assert.match(range[0], /width:100%/);
  assert.ok(!/max-width/.test(range[0]),
    "no max-width: the width is what says 'this is the important part'");
  const zone = html.match(/#ladder \{[\s\S]*?\}/);
  assert.ok(zone, "#ladder needs a rule");
  assert.match(zone[0], /border:1px solid #3d59a1/, "a real edge, in the app's selection blue");
  assert.match(zone[0], /background:rgba\(61,89,161,/, "and a wash of the same blue");
  // Not a NEW colour: #3d59a1 is already what a checked switch is painted with, so the
  // palette gains nothing and the blue already means "chosen / adjustable" in this app.
  assert.match(html, /input:checked \+ \.slider \{[^}]*#3d59a1/,
    "#3d59a1 must still be the switch's blue — if that moves, the frame's reasoning moves");
  // The diff's verdict colours must stay OUT of the frame.
  for (const verdict of ["#9ece6a", "#f7768e"]) {
    assert.ok(!zone[0].includes(verdict),
      `the zone must not be tinted ${verdict} — that is a verdict about a package`);
  }
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

test("ladder: the scale carries all five words, built from RUNGS", () => {
  // C, on seeing the full-width zone: "avec une telle largeur on peut mettre les
  // différents mots sur l'échelle". The gap it closes is real — before this, the four
  // rungs you were NOT on were invisible, so choosing meant dragging to discover.
  //
  // Built from RUNGS rather than written into the HTML, so the scale cannot drift from
  // the rule it labels. A hand-written list would be a second source of truth for the
  // words, and the words are a contract: three of the five promises were LIES until they
  // were checked against rungAllows.
  const at = appjs.indexOf("function renderLadderScale()");
  assert.notEqual(at, -1, "renderLadderScale() must exist");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /L\.RUNGS\.entries\(\)/,
    "the labels come from RUNGS — never a literal list, which would drift");
  assert.match(body, /classList\.toggle\("on", i === rung\)/,
    "exactly one label is marked, and it is the chosen rung");
  // Built once, then only re-marked: rebuilding mid-drag would drop the click targets and
  // re-layout five nodes at pointer rate.
  assert.match(body, /if \(!host\.childElementCount\)/,
    "the nodes are created once, not on every move");
  // Clicking a label must go through the INPUT's event, so there is one path that changes
  // the rung — a second would be a second place to forget the row repaint.
  assert.match(body, /dispatchEvent\(new Event\("input"/,
    "a label click reuses the input's own event path");
  assert.match(body, /if \(!r \|\| r\.disabled\) return/,
    "…and it stays frozen during a run or an incomplete scan, like the slider");
  // renderLadder must actually call it, or the scale never marks anything.
  const rl = appjs.slice(appjs.indexOf("function renderLadder()"));
  assert.ok(rl.slice(0, rl.indexOf("\n}\n")).includes("renderLadderScale()"),
    "renderLadder must repaint the scale");
});

test("ladder: Apply refuses an EMPTY rung, and says the rung is why", () => {
  // C, at the second sighting: "le apply reste actif même s'il semble ne plus rien y avoir
  // à exécuter — est-ce normal?". It was not. A button that promises an action with nothing
  // to do is the dishonesty this app already paid for once (the 403 retry card that offered
  // a download and did nothing), and clicking it costs a full server round-trip — re-scan,
  // full-window veil — to be told `done {nothing:true}`.
  const at = appjs.indexOf("const g = document.getElementById(\"install-all\")");
  assert.notEqual(at, -1, "the Apply button's liveness block must exist");
  const body = appjs.slice(at, at + 1400);
  // The emptiness must come from rungPlan, not from a second rule: the button, the count
  // under the thumb and the server's own filter have to agree about what "nothing" means.
  assert.match(body, /M\.rungPlan\(model, rung\)\.length === 0/,
    "emptiness is asked of rungPlan — a second rule would drift from the count");
  assert.match(body, /g\.disabled = busy \|\| rungEmpty/,
    "Apply is refused when the rung has no work, not only while busy");
  assert.match(body, /toggle\("live", .*!rungEmpty/,
    "…and it must not glow `live` either, or a dead button still invites the click");
  // And the reason is shown ONLY when a wider rung would help. With nothing to do at all,
  // the empty panel is already the message.
  assert.match(body, /rungEmpty && ids\.some\(isActionable\)/,
    "the note appears only when the RUNG is what emptied the plan");
  assert.match(body, /Nothing to do at this level/,
    "the note names the state, and the slider as the fix");
});

test("ladder: the ESTIMATE comes from the local secs of the same rung, unknowns included", () => {
  // Four ways a number this small could mislead, all invisible to the pure tests because they
  // pin `estimate`/`formatEstimate` and cannot see what the widget FEEDS them:
  //
  //   1. the wrong rung  → "☕ Unattended · 3 items · ~40 min" (the minutes of 🏗️)
  //   2. the SHARED `slow` instead of the LOCAL `secs` → minutes invented from a boolean
  //   3. the unknowns dropped → a confident "~4 min" for a plan mostly unmeasured
  //   4. a second traversal of model.pkgs → minutes for rows the count does not include
  const at = appjs.indexOf("function renderLadder()");
  assert.notEqual(at, -1, "renderLadder() must exist");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /L\.formatEstimate\(L\.estimate\(M\.rungSeconds\(model, rung\)\)\)/,
    "the estimate reads the chosen rung's LOCAL durations — any other argument is a wrong number");
  // The phrase must never be assembled here: `formatEstimate` is where the wording and its
  // limits live (no "at most", "<1 min" not "~0 min", the unknowns named), and a second
  // formatter in the view would be a second wording to keep true. Comments stripped first —
  // the comments quote the phrase on purpose, to say what the code must NOT do, and a guard
  // that forbids explaining itself is a guard that gets the explanation deleted.
  const code = body.replace(/\/\/.*$/gm, "");
  assert.ok(!/min for|unknown`|\bat most\b/.test(code),
    "app.js must not phrase the estimate itself — ladder.js owns the words and their limits");
  // `p.slow` is a fleet-wide max reduced to a boolean; minutes from it could only be an
  // invented constant, and a row is legitimately `slow: true, secs: 1` on a warm machine.
  const modeljs = readFileSync(new URL("../public/model.js", import.meta.url), "utf8");
  const rs = modeljs.indexOf("export function rungSeconds(");
  assert.notEqual(rs, -1, "rungSeconds must live in model.js, beside rungPlan");
  const rsBody = modeljs.slice(rs, modeljs.indexOf("\n}\n", rs));
  assert.match(rsBody, /rungPlan\(model, rung\)/,
    "…and it must walk rungPlan, so the minutes and the count describe the same rows");
  assert.match(rsBody, /\.secs \|\| 0/, "the LOCAL last-seen duration");
  assert.ok(!/\.slow\b/.test(rsBody.replace(/\/\/.*$/gm, "")),
    "never the shared classification — that is a boolean and a different question");
});

test("ladder: an ABSENT estimate takes its separator with it", () => {
  // At ⚡ on a machine whose config is already applied, the rung is empty: there is no
  // duration and no unknown to report, and `#ladder-blocked` already says "Nothing to do at
  // this level" two lines below. So the estimate goes quiet — but a "·" left behind would
  // read as a value that failed to load, which is the "greyed out teaches nothing" failure
  // in miniature. The separator is a ::before on the element, so hiding one hides both.
  const rule = html.match(/\.ladder-est::before \{[^}]*\}/);
  assert.ok(rule, ".ladder-est::before must carry the separator");
  assert.match(rule[0], /content:"· "/, "…so that hiding the element removes it too");
  const at = appjs.indexOf("function renderLadder()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  // The separator lives in ONE place. A "· " added in the JS as well would render "· · 3
  // unknown" — a MEASURED mutation survivor (`estEl.textContent = est ? "· " + est : ""`
  // passed every other test), and one that reads as a broken template rather than as a value.
  // So the estimate's text is exactly what formatEstimate returned, unpunctuated.
  assert.match(body, /estEl\.textContent = est;/,
    "the estimate's text is the formatted phrase itself — no punctuation added here");
  assert.ok(!body.includes('"· "') && !body.includes("'· '"),
    "the '·' belongs to the ::before only; a second one would print twice");
  assert.match(body, /estEl\.hidden = !est/, "an empty estimate hides the element");
  // And the estimate must be its own element: folded into #ladder-count it could not be
  // hidden without hiding the row count, which is never absent.
  assert.match(html, /id="ladder-est"/, "the estimate needs its own node");
  assert.match(html, /id="ladder-est"[^>]*hidden/,
    "…starting hidden, so the first paint before any plan shows no stray separator");
});

test("ladder: the estimate is ANNOUNCED, not visual-only", () => {
  // "How long will this take me" is the question the whole telemetry chain exists to answer,
  // and a user who cannot see the widget has it too. `aria-valuetext` is already the reading
  // for this control (a range would otherwise announce "4"), so the estimate belongs in it —
  // in the same order as on screen, punctuated for speech.
  const at = appjs.indexOf("function renderLadder()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /aria-valuetext",\s*est \? `\$\{spec\.name\}, \$\{count\}, \$\{est\}`/,
    "the announcement carries the word, the count AND the estimate, in that order");
  assert.match(body, /: `\$\{spec\.name\}, \$\{count\}`\)/,
    "…and falls back to the word and count alone when there is no estimate to announce");
});

test("ladder: the slider's max and detents match RUNGS — the one thing that cannot derive", () => {
  // ⭐ `max` and the <option> detents are HTML ATTRIBUTES, so unlike every other position in
  // this feature they cannot be computed from RUNGS. Adding a rung and forgetting them caps
  // the slider one short: the last rung becomes unreachable, and the failure is silent — the
  // widget just never offers Everything, which is the WORSE direction of error (doing less
  // than the user asked). So it is pinned here rather than trusted.
  const top = RUNGS.length - 1;
  const max = html.match(/id="ladder-range"[^>]*max="(\d+)"/);
  assert.ok(max, "#ladder-range must declare a max");
  assert.equal(
    Number(max[1]),
    top,
    `max="${max[1]}" but RUNGS has ${RUNGS.length} rungs — the top rung is unreachable`,
  );
  // One detent per rung, so the thumb snaps to every one of them.
  const detents = html.match(/id="ladder-detents"[\s\S]*?<\/datalist>/);
  assert.ok(detents, "the detent list must exist");
  const values = [...detents[0].matchAll(/<option value="(\d+)">/g)].map((m) => Number(m[1]));
  assert.deepEqual(
    values,
    RUNGS.map((_, i) => i),
    "one detent per rung, in order",
  );
});
