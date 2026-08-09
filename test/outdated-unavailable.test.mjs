// Structural guards on the "upgrade check unavailable" banner. Run: node --test
//
// No DOM library on purpose: this repo carries zero JS dependencies, and what
// these tests pin is STRUCTURAL — which element exists, which code path shows it,
// which one clears it — so reading the source as text catches it exactly.
//
// The defect: the machine-wide outdated scan returned an empty map for THREE
// different reasons — nothing outdated, the command failed, the output was
// unreadable — and the UI rendered all three identically. A user whose package
// source hiccupped saw a screen of reassuring green rows and had no way to know
// the upgrade check never happened. The server now says which case it is; these
// tests keep the front honest about showing it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

test("the banner element exists, and sits OUTSIDE #steps", () => {
  assert.match(html, /id="outdated-warn"/, "#outdated-warn must exist");

  // It must not live inside #steps: that container is wiped and re-rendered on
  // every plan, and it is also what the re-scan veil covers — a warning about the
  // whole machine belongs above the list, not inside it.
  const warnAt = html.indexOf('id="outdated-warn"');
  const stepsAt = html.indexOf('<div id="steps"');
  assert.notEqual(stepsAt, -1, "#steps must exist");
  assert.ok(
    warnAt < stepsAt,
    "#outdated-warn must precede #steps, not be nested in it",
  );
});

test("the banner is hidden until something shows it", () => {
  // `display:none` by default, revealed by a class — the same shape as .fb-banner.
  assert.match(
    html,
    /\.warn-banner\s*\{[^}]*display:\s*none/,
    ".warn-banner must default to display:none",
  );
  assert.match(
    html,
    /\.warn-banner\.show\s*\{[^}]*display:\s*block/,
    ".warn-banner.show must reveal it",
  );
});

test("the server's outdated-unavailable message is handled", () => {
  assert.match(
    appjs,
    /case "outdated-unavailable"/,
    "app.js must handle the message the server sends when the scan is unreadable",
  );
});

test("the reason is injected as TEXT, never as HTML", () => {
  // The reason quotes raw package-manager output. Interpolating it into innerHTML
  // would let a package name or an error string carrying markup reach the DOM.
  const block = handlerFor("outdated-unavailable");
  assert.match(
    block,
    /textContent\s*=\s*msg\.reason/,
    "msg.reason must be assigned via textContent",
  );
  assert.doesNotMatch(
    block,
    /innerHTML\s*=[^;]*msg\.reason/,
    "msg.reason must never be interpolated into innerHTML",
  );
});

test("the banner says presence is still trustworthy", () => {
  // The failure is narrow: presence comes from a separate per-package probe and
  // is unaffected. A banner that read as a general failure would send the user
  // hunting the wrong problem.
  const block = handlerFor("outdated-unavailable");
  assert.match(
    block,
    /presence/i,
    "the copy must say package presence is still accurate",
  );
});

test("a fresh plan clears a stale warning", () => {
  // A new scan re-answers the question. Leaving last run's caution up would be
  // its own kind of lie.
  const block = handlerFor("plan");
  assert.match(
    block,
    /outdated-warn[^]*?remove\("show"\)/,
    'case "plan" must clear the banner',
  );
});

test("a row reporting a real upgrade clears the warning", () => {
  // If any row carries an available version, the scan demonstrably worked — the
  // banner must not linger and contradict the row right below it.
  const block = handlerFor("outdated");
  assert.match(
    block,
    /outdated-warn[^]*?remove\("show"\)/,
    'case "outdated" must clear the banner',
  );
});

// The body of one `case "<name>":` arm, up to the next `case ` at the same level.
// Crude on purpose: precise enough to assert what a given arm does, and it fails
// loudly if the arm disappears.
function handlerFor(name) {
  const start = appjs.indexOf(`case "${name}":`);
  assert.notEqual(start, -1, `case "${name}" must exist in app.js`);
  const next = appjs.indexOf("\n    case ", start + 1);
  return appjs.slice(start, next === -1 ? appjs.length : next);
}
