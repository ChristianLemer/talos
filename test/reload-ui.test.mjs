// Structural guards on the catalogue RELOAD. Run: node --test (from talos/)
//
// No DOM library on purpose: this repo carries zero JS dependencies, and what these pin is
// STRUCTURAL — whether `render` clears before it appends — which reading the source as text
// catches exactly. Same technique as veil.test.mjs and server.rs's own text-level guards.
//
// THE DEFECT THIS EXISTS FOR. Until Refresh started re-reading catalog/, `render` ran
// exactly ONCE per session: one `plan` message, at connect. So it appended into an empty
// list and filled an empty registry, and nothing ever needed clearing. Sending a second
// `plan` without a wipe would have:
//   - appended a whole SECOND copy of the catalogue below the first, and
//   - left `rows` (keyed by index) holding the OLD elements, so every later `state` message
//     would paint a row nobody was looking at — a scan whose verdicts land invisibly.
//
// Neither is caught by model.test.mjs: the model replaces its maps correctly on its own
// (`loadPlan` clears them), which is precisely what makes the DOM half easy to miss.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

// The body of `render`, from its signature to the next top-level function.
function renderBody() {
  const start = appjs.indexOf("function render(steps");
  assert.notEqual(start, -1, "render moved — re-point this test");
  const end = appjs.indexOf("\nfunction ", start + 10);
  return appjs.slice(start, end === -1 ? undefined : end);
}

test("render CLEARS the previous rows before appending", () => {
  const body = renderBody();
  assert.ok(
    /stepsEl\.(replaceChildren\(\)|innerHTML\s*=\s*"")/.test(body),
    "render must empty #steps first, or a reload appends a second copy of the catalogue",
  );
});

test("render empties the row registry, and does not merely overwrite it", () => {
  const body = renderBody();
  // ⚠️ Overwriting per index is NOT enough: a catalogue that SHRANK leaves entries above
  // the new length, and those indices no longer designate anything. A removed package
  // would linger in the registry with the verdict it had when it existed.
  assert.ok(
    /delete rows\[/.test(body) || /rows\s*=\s*\{\}/.test(body),
    "render must drop every `rows` entry, not just reassign the ones the new plan covers",
  );
});

test("the clear happens BEFORE the first append", () => {
  // Order is the whole property: clearing after building would throw the new rows away.
  const body = renderBody();
  const clearAt = body.search(/stepsEl\.(replaceChildren|innerHTML)/);
  const appendAt = body.search(/stepsEl\.append/);
  assert.notEqual(clearAt, -1, "a clear must exist");
  assert.notEqual(appendAt, -1, "an append must exist");
  assert.ok(
    clearAt < appendAt,
    `the clear must precede the first append (clear@${clearAt}, append@${appendAt})`,
  );
});
