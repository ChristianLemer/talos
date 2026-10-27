// The estimate and the slow threshold, as the FRONT holds them. Pure — no DOM, no server, no
// model. Run: node --test test/estimate.test.mjs   (from talos/)
//
// ⭐ This file was ladder.test.mjs until 2026-10-10, when the ladder left; the rung tables went
// with it. What stays never depended on a rung.
import { test } from "node:test";
import assert from "node:assert/strict";
import { isSlow, SLOW_SECS, estimate, formatEstimate } from "../public/estimate.js";

test("isSlow reads the wire boolean, not a duration", () => {
  // The SERVER classifies (it holds the shared slow_secs and the threshold); the front is
  // told the verdict. Keeping the number out of the front is what stops the two from
  // drifting on a threshold.
  assert.equal(isSlow({ slow: true }), true);
  assert.equal(isSlow({ slow: false }), false);
  assert.equal(isSlow({}), false, "absent → not slow, never undefined");
  assert.equal(typeof SLOW_SECS, "number", "exported for the docs/tests, not for the rule");
});

// --- the estimate: minutes, and the unknowns beside them ----------------------

test("the estimate counts the unknowns instead of hiding them", () => {
  // A row with no local timing contributes NOTHING to the total, so an early estimate
  // under-reports. Showing a bare "~4 min" would therefore teach people to distrust it —
  // the same discipline that made the scan narrate "checking X, i of n" instead of a
  // frozen phrase.
  assert.deepEqual(estimate([120, 120, 0, 0, 0]), { secs: 240, known: 2, unknown: 3 });
  assert.deepEqual(estimate([]), { secs: 0, known: 0, unknown: 0 });
  assert.deepEqual(estimate([0, 0]), { secs: 0, known: 0, unknown: 2 });
  assert.deepEqual(estimate([30]), { secs: 30, known: 1, unknown: 0 });
  // ⚠️ `0` is the "never measured HERE" sentinel, never "it was instant": server.rs floors a
  // real measurement at 1s (timings.rs::note_duration) precisely so this test can rely on it.
  // A 1s row is therefore KNOWN, and must land in `known` — treating `<= 0` as unknown would
  // be right, treating `< 2` as unknown would silently reclassify every warm-cache row.
  assert.deepEqual(estimate([1]), { secs: 1, known: 1, unknown: 0 });
});

test("formatEstimate: C's shape, and never a bare ~N min", () => {
  // C's wording: "~N min for X, Y never timed here" (was "X and Y unknown" until 2026-10-27). The spec's "at most ~N min" belonged to a
  // total summed from the SHARED slow_secs, which is a max over the fleet; these minutes come
  // from a LOCAL last-seen cache, which is neither a worst case nor an average — it is what it
  // took here last time. So "at most" would be a false claim and is deliberately absent.
  assert.equal(formatEstimate({ secs: 240, known: 6, unknown: 3 }), "~4 min for 6, 3 never timed here");
  // Every item measured: say so explicitly rather than dropping the clause, so the reader
  // never has to wonder whether an unknown count was omitted or was zero.
  assert.equal(formatEstimate({ secs: 240, known: 4, unknown: 0 }), "~4 min for all 4");
  // Nothing measured yet: no invented minutes. THE ORDINARY STATE on a fresh machine, and
  // the one this whole shape exists for — the row count is still shown beside it, so the
  // reading is "19 to install · 19 never timed here": we know what we would do, not how long it takes.
  assert.equal(formatEstimate({ secs: 0, known: 0, unknown: 3 }), "3 never timed here");
  // Sub-minute must not round to "~0 min" — a total of 30s with `Math.round(30/60)` is 1,
  // but 45s is 1 too and 29s would be 0, i.e. "instant" for something that is not.
  assert.equal(formatEstimate({ secs: 30, known: 1, unknown: 0 }), "<1 min for all 1");
  assert.equal(formatEstimate({ secs: 59, known: 2, unknown: 0 }), "<1 min for all 2");
  assert.equal(formatEstimate({ secs: 60, known: 2, unknown: 0 }), "~1 min for all 2");
  assert.equal(formatEstimate({ secs: 89, known: 2, unknown: 0 }), "~1 min for all 2");
  // An HOUR reads as an hour. "~127 min" is arithmetically fine and humanly useless — the
  // question an estimate answers is "do I have time right now?", and nobody converts 127
  // minutes in their head to answer it. a fresh corporate machine with Xcode CLT in the plan is
  // exactly this case, so it is not hypothetical.
  assert.equal(formatEstimate({ secs: 3600, known: 3, unknown: 0 }), "~1 h for all 3");
  assert.equal(formatEstimate({ secs: 7620, known: 8, unknown: 2 }), "~2 h 7 min for 8, 2 never timed here");
  // …and the minutes must never round UP into a 60th minute: 7199s is 120 whole minutes.
  assert.equal(formatEstimate({ secs: 7199, known: 3, unknown: 0 }), "~2 h for all 3");
  // Nothing at all: the estimate says NOTHING. The list's own empty state already says there is
  // nothing to send, and an empty string is the only answer that cannot be wrong: with no rows
  // there is no duration and no unknown to report.
  assert.equal(formatEstimate({ secs: 0, known: 0, unknown: 0 }), "");
});
