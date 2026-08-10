// The ladder rule, as the FRONT holds it. Pure — no DOM, no server, no model: a function
// of facts, exactly like scope.js's. Run: node --test test/ladder.test.mjs   (from talos/)
//
// ⚠️ THIS TABLE IS THE TWIN OF src/ladder.rs's `the_three_rungs_are_cumulative_…`. They must
// stay in lockstep — the same relation model.js's AUTO_ACTS already has with the server's
// Install|Uninstall|Upgrade filter. The front needs the rule ONLY to show per-rung counts;
// the filter that governs the plan is the Rust one. So a drift costs a wrong count, never a
// wrong action — the duplication runs in the safe direction, which is the reason it is
// tolerated at all.
//
// The rows are the same rows, in the same order, with one addition Rust CANNOT express: a
// `null` action. `decision::Action` is a total enum there, so "no action at all" is not a
// case the Rust table can carry; here `actionOf` returns null for an out-of-scope row or a
// row with nothing to do, so it must be answered.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  RUNGS, rungAllows, rungFromInput, isSlow, SLOW_SECS, estimate, formatEstimate,
} from "../public/ladder.js";
// `rungReason` lives in model.js, beside `rungPlan`, and NOT in ladder.js — ladder.js is a
// declared TWIN of src/ladder.rs, and an extra export with no Rust half would make that
// claim false for the next reader who diffs them. It is tested here anyway, because it is
// the same rule read from the other side: rungPlan answers "which rows", rungReason answers
// "and why not the others", and the two must never be able to disagree.
import { rungReason } from "../public/model.js";

// ⚠️ ONE facts fixture left, and only one test uses it: the rungs no longer read facts at all,
// so the four that were here (`quiet` / `elevates` / `blocked` / `slow`) had nothing left to
// discriminate. What remains checks that a fact CANNOT change a verdict — the reshape asserted
// directly rather than assumed.
const anyFacts = { uac: true, forbidden: true, slow: true };

test("the three rungs are cumulative, and the table shows it", () => {
  // Columns: ⚡ Config only · 🧩 Extensions · 📦 Apps
  // ⚠️ THIS TABLE IS THE TWIN of src/ladder.rs's. Change one, change the other — a drift
  // costs a wrong COUNT under the slider while the server does something else.
  //
  // ⭐ NO `facts` COLUMN, and its absence is the reshape: `rungAllows` no longer takes facts
  // at all, because the rungs that sorted by `uac`/`403`/`slow` are gone and those facts are
  // written on the row instead. A fact can no longer change a verdict, so a fixture carrying
  // one here would imply otherwise.
  const cases = [
    ["config atom install", "install", true, false, [true, true, true]],
    // ⭐ A config-atom's UPGRADE is admitted at 0 too — the six-rung version sent every
    // upgrade to the facts ladder regardless of kind, which was the bug this fixes.
    ["config atom upgrade", "upgrade", true, false, [true, true, true]],
    // 🧩 An extension, install OR upgrade: rung 1. Above config-atoms (it downloads), below
    // apps (it never writes outside your profile and never elevates).
    ["extension install", "install", false, true, [false, true, true]],
    ["extension upgrade", "upgrade", false, true, [false, true, true]],
    // 📦 An app: rung 2, and NOTHING is excluded there — no fact holds it back any more.
    ["install", "install", false, false, [false, false, true]],
    ["upgrade", "upgrade", false, false, [false, false, true]],
    // uninstall is NOT on the ladder — it honours the user's ✕ at every rung.
    ["uninstall", "uninstall", false, false, [true, true, true]],
    // downgrade is never batched by Apply (mirrors AUTO_ACTS).
    ["downgrade", "downgrade", false, false, [false, false, false]],
    // A null action (out of scope, or nothing to do) is never in any rung.
    ["no action", null, false, false, [false, false, false]],
  ];
  for (const [what, action, isConfig, isExt, expected] of cases) {
    expected.forEach((want, rung) => {
      assert.equal(rungAllows(rung, action, isConfig, isExt), want, `${what} at rung ${rung}`);
    });
    let seenTrue = false;
    for (const v of expected) {
      if (seenTrue) assert.ok(v, `${what}: a rung withdrew what a lower one admitted`);
      seenTrue ||= v;
    }
  }
});

test("RUNGS carries three rungs, each with an emoji, a name and a promise", () => {
  assert.equal(RUNGS.length, 3);
  for (const r of RUNGS) {
    assert.ok(r.emoji && r.name && r.promise, `${r.name} needs all three`);
  }
  // ⭐ There were six. The three that went (☕ 👀 🏗️) sorted UPGRADES by uac/403/slow — facts
  // the row now carries in words instead. Pinned as bytes because the ramp is a claim: it
  // answers "where does this land?" (files → profile → machine), NOT "is this better?".
  // 🧩 is a puzzle piece: something that slots INTO a thing already there.
  assert.deepEqual(RUNGS.map((r) => r.emoji), ["⚡", "🧩", "📦"]);
  // And the KEYS are the names ladder.rs::Rung::as_str logs, index for index — so a log
  // line and a widget position can be compared by a human reading both.
  assert.deepEqual(RUNGS.map((r) => r.key), ["config-only", "extensions", "apps"]);
});

test("every promise is TRUE of what the rule does at that rung", () => {
  // ⚠️ The promise is the only sentence the user reads before committing, so it is the one
  // place an overstatement becomes a lie the code then tells. Three of the original five were
  // caught being one, all in the SAME direction — promising more calm than the rung gives.
  // Two of those three rungs no longer exist; the discipline outlived them.
  //
  // Shaped as "the promise says X, so X must hold" rather than as string matches, so rewording
  // is free and lying is not.
  const [r0, rExt, rApps] = RUNGS;

  // ⚡ rung 0: "adds config only", and removals still run.
  assert.equal(rungAllows(0, "install", true, false), true, "a config-atom is added");
  assert.equal(rungAllows(0, "upgrade", true, false), true, "…and updated");
  assert.equal(rungAllows(0, "install", false, false), false, "…and nothing else is");
  assert.equal(rungAllows(0, "uninstall", false, false), true, "removals DO run at rung 0");
  assert.match(r0.promise, /[Rr]emovals/, "so the promise must say so, not deny it");
  assert.ok(!/\bInstant\b/.test(r0.promise),
    "and must not claim instant: a removal shells out to brew/winget");

  // 🧩 rung 1: extensions enter, apps do not.
  assert.equal(rungAllows(1, "install", false, true), true, "an extension is added");
  assert.equal(rungAllows(1, "upgrade", false, true), true, "…and updated");
  assert.equal(rungAllows(1, "install", false, false), false, "…an app is NOT");
  // The words: no speed claim (a clone can be slow on a bad line), and it must say WHERE it
  // writes, which is the part that is actually guaranteed.
  assert.ok(!/\b[Ff]ast\b|\b[Ii]nstant\b/.test(rExt.promise),
    "no speed claim: nothing has ever measured these routes");
  assert.match(rExt.promise, /profile/i, "it must say where it writes");

  // 📦 rung 2: everything, and the promise must NOT claim you can walk away.
  for (const a of ["install", "upgrade", "uninstall"]) {
    assert.equal(rungAllows(2, a, false, false), true, `${a} runs at 📦`);
  }
  assert.ok(!/walk away|uninterrupted|nothing will/i.test(rApps.promise),
    "the ladder no longer separates 'I can leave' from 'it will ask' — the promise must not");
  assert.match(rApps.promise, /hand|password|ask/i,
    "…and it must say so out loud, since the rung admits the ones that prompt");
});

test("the FACTS no longer move a verdict — only the kind does", () => {
  // ⭐ THE reshape, asserted directly: `uac`, `403` and `slow` used to decide which rung
  // admitted an upgrade. They no longer reach the rule at all. Checked over the whole fact
  // cross-product rather than at a point, because the failure mode is a fact sneaking back in
  // as a special case for one combination.
  for (const uac of [false, true]) {
    for (const forbidden of [false, true]) {
      for (const slow of [false, true]) {
        const what = `uac=${uac} 403=${forbidden} slow=${slow}`;
        for (let r = 0; r < RUNGS.length; r++) {
          for (const a of ["install", "upgrade", "uninstall", "downgrade"]) {
            // ⚠️ A 5th argument is passed on purpose: an old caller that still hands facts
            // over must get the SAME answer, or the twin's signature change would silently
            // alter behaviour somewhere I did not grep.
            assert.equal(
              rungAllows(r, a, false, false, anyFacts),
              rungAllows(r, a, false, false),
              `${a} @${r} (${what}): a fact changed the verdict`,
            );
          }
        }
      }
    }
  }
});

test("rungFromInput yields an INTEGER NUMBER — a string on the wire would mean the top rung", () => {
  // A range input's `.value` is a STRING. `server::rung_from_wire` reads the field with serde's
  // `as_u64`, which rejects a JSON string AND a JSON float, and its None arm lands on the top
  // rung. So sending `"1"` would run 📦 Apps while the widget said 🧩 Extensions — the widget
  // lying about the plan, silently, in the direction of doing MORE.
  assert.equal(rungFromInput("1"), 1);
  assert.equal(typeof rungFromInput("1"), "number");
  assert.equal(JSON.stringify({ rung: rungFromInput("1") }), '{"rung":1}');
  // A FRACTIONAL input, which `parseFloat` would let through: `as_u64` rejects `1.5` exactly as
  // it rejects `"1"`. No range input emits a fraction with `step="1"`, but a keyboard or a
  // future half-step could — and the failure is silent. (A live mutation survivor: swapping
  // parseInt→parseFloat passed the whole suite.)
  assert.equal(rungFromInput("1.5"), 1, "truncated toward the SMALLER rung, not floated");
  assert.ok(Number.isInteger(rungFromInput("1.5")));
  // Clamped the SAME way Rung::from_wire clamps, and toward the same end: out of range or
  // unreadable → the top, so widget and server agree even on nonsense. Doing silently LESS
  // than asked is the worse error (ladder.rs's `impl Default for Rung`).
  //
  // ⚠️ Read from RUNGS, never written as a literal: the ceiling has moved twice (4 → 5 → 2) and
  // every hardcoded copy had to be hand-edited each time.
  const top = RUNGS.length - 1;
  assert.equal(rungFromInput("0"), 0);
  assert.equal(rungFromInput(String(top)), top);
  assert.equal(rungFromInput(String(top + 5)), top, "out of range → the top");
  assert.equal(rungFromInput("-1"), 0, "below range → the first rung, not a crash");
  assert.equal(rungFromInput(""), top, "unreadable → the top, like an absent field");
  assert.equal(rungFromInput(undefined), top);
  // And every clamped result must index into RUNGS — renderLadder reads RUNGS[rung].emoji with
  // no guard of its own.
  for (const v of ["-5", "0", "1", "2", "99", "", "x", null]) {
    assert.ok(RUNGS[rungFromInput(v)], `RUNGS[rungFromInput(${JSON.stringify(v)})] must exist`);
  }
});

test("isSlow reads the wire boolean, not a duration", () => {
  // The SERVER classifies (it holds the shared slow_secs and the threshold); the front is
  // told the verdict. Keeping the number out of the front is what stops the two from
  // drifting on a threshold.
  assert.equal(isSlow({ slow: true }), true);
  assert.equal(isSlow({ slow: false }), false);
  assert.equal(isSlow({}), false, "absent → not slow, never undefined");
  assert.equal(typeof SLOW_SECS, "number", "exported for the docs/tests, not for the rule");
});

test("a row out of reach says WHERE it enters, in the user's terms", () => {
  // ⭐ Two reason keys where there were five. "slow", "hand" and "blocked" are gone from HERE
  // — not from the app: they moved from "why this row is out of reach" to "what this row will
  // do to you", shown on every row at every rung. A fact can no longer hold a row out of a
  // rung, so it can no longer be the reason one is dimmed.
  const ext = { isConfig: false, isExtension: true };
  const app = { isConfig: false, isExtension: false };
  const cfg = { isConfig: true, isExtension: false };

  // An extension at ⚡ is out, and the reason names the rung that admits it.
  assert.equal(rungReason(0, "install", ext), "extension");
  assert.equal(rungReason(1, "install", ext), null, "in reach → nothing to explain");
  // An app is out at both lower rungs, for the same reason both times.
  assert.equal(rungReason(0, "install", app), "app");
  assert.equal(rungReason(1, "install", app), "app");
  assert.equal(rungReason(2, "install", app), null);
  // A config-atom is admitted everywhere, so it can never carry a reason.
  for (let r = 0; r < RUNGS.length; r++) {
    assert.equal(rungReason(r, "install", cfg), null, `config atom @${r}`);
  }
});

test("a row the LADDER does not govern gets no reason at all", () => {
  // A null action (out of scope, or nothing to do) and a `downgrade` are out at EVERY rung, so
  // the RUNG is not what excludes them. Dimming them would blame the slider for a row it does
  // not govern — and on a converged machine that is most of the list.
  for (let r = 0; r < RUNGS.length; r++) {
    assert.equal(rungReason(r, null, { isExtension: false }), null, `null action @${r}`);
    assert.equal(rungReason(r, "downgrade", { isExtension: false }), null, `downgrade @${r}`);
    // An uninstall runs at every rung, so it is never out of reach either.
    assert.equal(rungReason(r, "uninstall", { isExtension: false }), null, `uninstall @${r}`);
  }
});

test("rungReason and rungAllows cannot disagree — a reason means OUT, exhaustively", () => {
  // The invariant that makes the dimming trustworthy, over the whole cross product rather than
  // at chosen points: a reason is present exactly when the rule refuses, EXCEPT for the rows no
  // rung admits (pinned above). And every reason must be a key app.js has words for — an
  // unknown key renders an empty span, i.e. a dimmed row with no explanation.
  const known = new Set(["extension", "app"]);
  for (const action of ["install", "uninstall", "upgrade", "downgrade", null]) {
    for (const isConfig of [false, true]) {
      for (const isExtension of [false, true]) {
        const p = { isConfig, isExtension };
        const governed = rungAllows(RUNGS.length - 1, action, isConfig, isExtension);
        for (let r = 0; r < RUNGS.length; r++) {
          const why = rungReason(r, action, p);
          const allowed = rungAllows(r, action, isConfig, isExtension);
          const what = `${action} isConfig=${isConfig} isExt=${isExtension} @${r}`;
          if (allowed) {
            assert.equal(why, null, `${what}: IN the plan, so no reason may be given`);
          } else if (!governed) {
            assert.equal(why, null, `${what}: no rung admits it, so the rung is not why`);
          } else {
            assert.ok(why, `${what}: out AND governed by the rung → it must say why`);
            assert.ok(known.has(why), `${what}: "${why}" has no word in app.js's RUNG_WHY`);
          }
        }
      }
    }
  }
});

test("every reason key is REACHABLE, and its tooltip names a rung that really admits it", () => {
  // 1. Reachability. A key with no (rung, action, kind) that produces it is dead copy, and the
  //    next reader would trust it.
  const reachable = new Set();
  for (const a of ["install", "uninstall", "upgrade", "downgrade", null]) {
    for (const isConfig of [false, true]) {
      for (const isExtension of [false, true]) {
        for (let r = 0; r < RUNGS.length; r++) {
          const k = rungReason(r, a, { isConfig, isExtension });
          if (k) reachable.add(k);
        }
      }
    }
  }
  assert.deepEqual([...reachable].sort(), ["app", "extension"]);

  // 2. The tooltips NAME A RUNG ("Move to 🧩 Extensions to include it"), which is a promise
  //    about what happens next. So the named rung must really admit the row, and the one below
  //    it must really not — checked by RUNNING the rule, not by reading strings.
  const entersAt = (rung, action, isExt) =>
    rungAllows(rung, action, false, isExt) && !rungAllows(rung - 1, action, false, isExt);
  assert.ok(entersAt(1, "install", true), "an extension must enter exactly at 🧩");
  assert.ok(entersAt(2, "install", false), "an app must enter exactly at 📦");
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
  // C's wording: "~N min for X and Y unknown". The spec's "at most ~N min" belonged to a
  // total summed from the SHARED slow_secs, which is a max over the fleet; these minutes come
  // from a LOCAL last-seen cache, which is neither a worst case nor an average — it is what it
  // took here last time. So "at most" would be a false claim and is deliberately absent.
  assert.equal(formatEstimate({ secs: 240, known: 6, unknown: 3 }), "~4 min for 6 and 3 unknown");
  // Every item measured: say so explicitly rather than dropping the clause, so the reader
  // never has to wonder whether an unknown count was omitted or was zero.
  assert.equal(formatEstimate({ secs: 240, known: 4, unknown: 0 }), "~4 min for all 4");
  // Nothing measured yet: no invented minutes. THE ORDINARY STATE on a fresh machine, and
  // the one this whole shape exists for — the row count is still shown beside it, so the
  // reading is "19 items · 19 unknown": we know what we would do, not how long it takes.
  assert.equal(formatEstimate({ secs: 0, known: 0, unknown: 3 }), "3 unknown");
  // Sub-minute must not round to "~0 min" — a total of 30s with `Math.round(30/60)` is 1,
  // but 45s is 1 too and 29s would be 0, i.e. "instant" for something that is not.
  assert.equal(formatEstimate({ secs: 30, known: 1, unknown: 0 }), "<1 min for all 1");
  assert.equal(formatEstimate({ secs: 59, known: 2, unknown: 0 }), "<1 min for all 2");
  assert.equal(formatEstimate({ secs: 60, known: 2, unknown: 0 }), "~1 min for all 2");
  assert.equal(formatEstimate({ secs: 89, known: 2, unknown: 0 }), "~1 min for all 2");
  // An HOUR reads as an hour. "~127 min" is arithmetically fine and humanly useless — the
  // question the widget answers is "do I have time right now?", and nobody converts 127
  // minutes in their head to answer it. a fresh corporate machine with Xcode CLT in the plan is
  // exactly this case, so it is not hypothetical.
  assert.equal(formatEstimate({ secs: 3600, known: 3, unknown: 0 }), "~1 h for all 3");
  assert.equal(formatEstimate({ secs: 7620, known: 8, unknown: 2 }), "~2 h 7 min for 8 and 2 unknown");
  // …and the minutes must never round UP into a 60th minute: 7199s is 120 whole minutes.
  assert.equal(formatEstimate({ secs: 7199, known: 3, unknown: 0 }), "~2 h for all 3");
  // Nothing at all: the estimate says NOTHING, and app.js drops the separator with it.
  //
  // ⚠️ DELIBERATE DEVIATION from the plan, which specified "nothing to do" here. Verified in
  // the real app: at ⚡ on this Mac the rung is empty, and the plan's string would have made
  // the widget read "0 items · nothing to do" directly above `#ladder-blocked`'s "Nothing to
  // do at this level. There is more further right." — the same word three times in two lines.
  // The blocked note already owns this state and names the fix; a third phrase is the noise
  // that note's own reasoning refuses ("with nothing to do at all, the empty panel is the
  // message"). An empty string is also the only answer that cannot be wrong: with no rows
  // there is no duration and no unknown to report.
  assert.equal(formatEstimate({ secs: 0, known: 0, unknown: 0 }), "");
});
