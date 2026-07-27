// Structural guards on the 403 recovery UI, in the same spirit as veil.test.mjs.
//
// Why these exist as SOURCE-TEXT assertions: the defects they pin live in
// `app.js`, which is DOM-bound and imported by no test, and each of them is
// structural — "does this branch send that message", "is this element's `hidden`
// actually honoured by the CSS". A behavioural test would need a DOM library, and
// this repo carries zero JS dependencies on purpose.
//
// And they pin bugs that ALL SHIPPED. `test/decision.test.mjs` asserts the pure
// `forbiddenMessage` mapping, which is necessary but cannot regress to any of these:
// that function was introduced BY the fix, so it never held the defect.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");

/** The body of a top-level `function name(...) { … }`, brace-matched. */
function fnBody(src, name) {
  const at = src.indexOf(`function ${name}(`);
  assert.notEqual(at, -1, `${name}() must exist`);
  const open = src.indexOf("{", at);
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) return src.slice(open, i + 1);
  }
  throw new Error(`unbalanced braces in ${name}()`);
}

test("403: retryStep while paused asks for a RETRY, never a continue", () => {
  // THE original defect: the button said "Retry" and sent `forbidden-continue`, so
  // the Apply moved on to the NEXT package and left this row marked `forbidden` —
  // the user had to hunt it down and click again. The modal's step 3 meanwhile
  // promised "Talos downloads it for you". The word lied.
  //
  // Asserted on the fbPaused BRANCH specifically, because that is where the bug
  // lived: re-wiring it to decideForbidden("continue") must fail this test.
  const body = fnBody(appjs, "retryStep");
  const paused = body.slice(0, body.indexOf("if (applyRunning)"));
  assert.match(paused, /decideForbidden\("retry"\)/, "the paused branch must ask for a retry");
  assert.doesNotMatch(
    paused,
    /decideForbidden\("continue"\)/,
    'Retry must not send "continue" — that skips the row and the label becomes a lie',
  );
});

test("403: giving up or stopping DECIDES before it clears the banner", () => {
  // Both used to call clearForbidden() first. While a retry was running (so nothing
  // was waiting on the server), the banner vanished and NO message was sent: the
  // click looked like it worked and did nothing. A control that appears to act and
  // does not is worse than one that is plainly unavailable.
  for (const [fn, gesture] of [
    ["giveUp", "continue"],
    ["stopApply", "stop"],
  ]) {
    const body = fnBody(appjs, fn);
    const decide = body.indexOf(`decideForbidden("${gesture}")`);
    const clear = body.indexOf("clearForbidden(");
    assert.notEqual(decide, -1, `${fn}() must send ${gesture}`);
    assert.notEqual(clear, -1, `${fn}() must clear the recovery UI`);
    assert.ok(
      decide < clear,
      `${fn}() clears the banner before deciding → on a no-op the click silently lies`,
    );
  }
});

test("403: the whole Apply can actually be stopped from the UI", () => {
  // `forbidden-stop` is implemented server-side and was UNREACHABLE: nothing in the
  // front ever sent it, so "abandon the rest of the plan" was a promise made only in
  // a comment. Three outcomes need three controls.
  assert.match(appjs, /decideForbidden\("stop"\)/);
  assert.match(html, /id="fb-stop"/, "the modal needs a Stop control");
  assert.match(appjs, /data-fb="stop"/, "the row banner needs one too");
});

test("hidden: every element hidden from JS is actually hidden by the CSS", () => {
  // `[hidden] { display:none }` lives in the UA stylesheet, so ANY author `display`
  // rule beats it. That is not theoretical: `#fb-retry` carries
  // `.fb-step { display:flex }`, so setting `.hidden = true` at the retry cap left
  // the "Come back and retry — Talos downloads it for you" card VISIBLE and
  // CLICKABLE while retryStep refused to act. Verified in a real browser before the
  // fix: hidden=true, computed display=flex, clickable.
  //
  // The global rule below forecloses the class. This test guards the rule itself,
  // because deleting it would silently resurrect every instance.
  assert.match(
    html,
    /\[hidden\]\s*\{[^}]*display:\s*none\s*!important/,
    "a global `[hidden] { display:none !important }` must exist — the UA rule alone loses to any author display",
  );
});
