// The ladder rule, as the FRONT holds it. Pure — no DOM, no server, no model: a function
// of facts, exactly like scope.js's. Run: node --test test/ladder.test.mjs   (from talos/)
//
// ⚠️ THIS TABLE IS THE TWIN OF src/ladder.rs's `the_five_rungs_are_cumulative_…`. They must
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
import { RUNGS, rungAllows, rungFromInput, isSlow, SLOW_SECS } from "../public/ladder.js";

const quiet = { uac: false, forbidden: false, slow: false };
const elevates = { uac: true, forbidden: false, slow: false };
const blocked = { uac: false, forbidden: true, slow: false };
const slow = { uac: false, forbidden: false, slow: true };

test("the five rungs are cumulative, and the table shows it", () => {
  // Columns: Config only · Add missing · Unattended · Stay nearby · Everything
  const cases = [
    ["config atom", "install", true, quiet, [true, true, true, true, true]],
    ["install", "install", false, quiet, [false, true, true, true, true]],
    ["install, elevates", "install", false, elevates, [false, true, true, true, true]],
    ["upgrade, quiet", "upgrade", false, quiet, [false, false, true, true, true]],
    ["upgrade, uac", "upgrade", false, elevates, [false, false, false, true, true]],
    ["upgrade, 403", "upgrade", false, blocked, [false, false, false, true, true]],
    ["upgrade, slow", "upgrade", false, slow, [false, false, false, false, true]],
    ["upgrade, slow+uac", "upgrade", false, { uac: true, forbidden: false, slow: true },
      [false, false, false, false, true]],
    // uninstall is NOT on the ladder — it honours the user's ✕ at every rung.
    ["uninstall", "uninstall", false, quiet, [true, true, true, true, true]],
    ["uninstall, everything bad", "uninstall", false, { uac: true, forbidden: true, slow: true },
      [true, true, true, true, true]],
    // downgrade is never batched by Apply (mirrors AUTO_ACTS).
    ["downgrade", "downgrade", false, quiet, [false, false, false, false, false]],
    // A null action (out of scope, or nothing to do) is never in any rung.
    ["no action", null, false, quiet, [false, false, false, false, false]],
  ];
  for (const [what, action, isConfig, facts, expected] of cases) {
    expected.forEach((want, rung) => {
      assert.equal(rungAllows(rung, action, isConfig, facts), want, `${what} at rung ${rung}`);
    });
    let seenTrue = false;
    for (const v of expected) {
      if (seenTrue) assert.ok(v, `${what}: a rung withdrew what a lower one admitted`);
      seenTrue ||= v;
    }
  }
});

test("RUNGS carries five rungs, each with an emoji, a name and a promise", () => {
  assert.equal(RUNGS.length, 5);
  for (const r of RUNGS) {
    assert.ok(r.emoji && r.name && r.promise, `${r.name} needs all three`);
  }
  // The emoji ramp answers "do I have time?", NOT "is this better?". Pinned as bytes
  // because a satisfaction ramp (🙁→😀) is the tempting wrong answer and would assert
  // that Everything is the good end — rung 0 is the only one that cannot fail on the
  // network, so that assertion would be false.
  assert.deepEqual(RUNGS.map((r) => r.emoji), ["⚡", "📦", "☕", "👀", "🏗️"]);
  // And the KEYS are the names ladder.rs::Rung::as_str logs, index for index — so a log
  // line and a widget position can be compared by a human reading both.
  assert.deepEqual(
    RUNGS.map((r) => r.key),
    ["config-only", "add-missing", "unattended", "stay-nearby", "everything"],
  );
});

test("every promise is TRUE of what the rule does at that rung", () => {
  // ⚠️ The promise is the only sentence the user reads before committing, so it is the one
  // place an overstatement becomes a lie the code then tells. Three of the plan's five were
  // caught being one, all in the SAME direction — promising more calm than the rung gives:
  //
  //   rung 0: "Instant, and the only rung that cannot fail on the network" — FALSE.
  //           `uninstall` is on EVERY rung, and `brew uninstall` / `winget uninstall
  //           --source winget` is neither instant nor offline.
  //   rung 2: "Start it and walk away" — FALSE. The uac/403 gate governs UPGRADES only; an
  //           INSTALL flagged `uac` is admitted from rung 1 (ladder.rs's table says so),
  //           so rung 1 already broke this promise before rung 2 made it.
  //   rung 3: "the ones that need your hand" — ambiguous, read as covering the slow ones
  //           too. `slow` DOMINATES: rung 3 excludes them full stop.
  //
  // These assertions are shaped as "the promise says X, so X must hold", not as string
  // matches, so rewording is free and lying is not.
  const [r0, r1, r2, r3, r4] = RUNGS;

  // rung 0 says "adds config only" and "removals still run".
  assert.equal(rungAllows(0, "install", true, quiet), true, "a config-atom is added");
  assert.equal(rungAllows(0, "install", false, quiet), false, "…and nothing else is");
  assert.equal(rungAllows(0, "upgrade", false, quiet), false);
  assert.equal(rungAllows(0, "uninstall", false, quiet), true, "removals DO run at rung 0");
  assert.match(r0.promise, /[Rr]emovals/, "so the promise must say so, not deny it");
  assert.ok(!/\bInstant\b/.test(r0.promise),
    "and must not claim instant: a removal shells out to brew/winget");
  assert.ok(!/cannot fail on the network/.test(r0.promise),
    "nor offline: `winget uninstall --source winget` reaches a source");

  // rung 1 says "installs what is absent. No updates" — both halves.
  assert.equal(rungAllows(1, "install", false, quiet), true);
  for (const f of [quiet, elevates, blocked, slow]) {
    assert.equal(rungAllows(1, "upgrade", false, f), false, "no updates means NO updates");
  }
  assert.match(r1.promise, /[Nn]o updates/);

  // rung 2 adds quiet upgrades and no others.
  assert.equal(rungAllows(2, "upgrade", false, quiet), true);
  assert.equal(rungAllows(2, "upgrade", false, elevates), false);
  assert.equal(rungAllows(2, "upgrade", false, blocked), false);
  assert.equal(rungAllows(2, "upgrade", false, slow), false);
  // …but an elevating INSTALL is already through, from rung 1. So the promise must scope
  // itself to updates rather than promise an uninterrupted run.
  assert.equal(rungAllows(2, "install", false, elevates), true, "the fact that forbids the words");
  assert.match(r2.promise, /updates/,
    "the promise must be about UPDATES, since installs are not gated on uac at all");
  assert.ok(!/walk away/.test(r2.promise),
    "an elevating install runs here, so 'walk away' would be a promise the rung breaks");

  // rung 3 adds uac/403 upgrades — and NOT the slow ones.
  assert.equal(rungAllows(3, "upgrade", false, elevates), true);
  assert.equal(rungAllows(3, "upgrade", false, blocked), true);
  assert.equal(rungAllows(3, "upgrade", false, slow), false, "slow DOMINATES");
  assert.ok(!/slow/i.test(r3.promise), "so rung 3 must not hint at the slow ones");

  // rung 4 adds the slow ones, and excludes nothing.
  assert.equal(rungAllows(4, "upgrade", false, slow), true);
  assert.match(r4.promise, /slow/i);
  for (const [a, cfg, f] of [["install", true, quiet], ["install", false, slow],
    ["upgrade", false, elevates], ["upgrade", false, blocked], ["uninstall", false, slow]]) {
    assert.equal(rungAllows(4, a, cfg, f), true, `rung 4 excludes nothing: ${a}`);
  }

  // And each rung 1..4 says "Also …": they ADD to the one before, which is the cumulative
  // property stated in words rather than only in the table above.
  for (const r of [r2, r3, r4]) {
    assert.match(r.promise, /^Also /, `${r.name} adds to the rung below, and says so`);
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

test("rungFromInput yields an INTEGER NUMBER — a string on the wire would mean Everything", () => {
  // ⚠️ The one way this widget can silently lie. A range input's `.value` is a STRING, and
  // server.rs's `rung_from_wire` reads it with serde's `as_u64`, which rejects a JSON
  // string outright — its None arm then lands on `Rung::Everything`. So sending "2"
  // would run rung 4 while the widget said "Unattended, start it and walk away". Hence a
  // parse, and hence this test: the assertion is on the WIRE BYTES, not on the JS value,
  // because that is what serde actually sees.
  assert.equal(rungFromInput("2"), 2);
  assert.equal(typeof rungFromInput("2"), "number");
  assert.equal(JSON.stringify({ rung: rungFromInput("2") }), '{"rung":2}');
  assert.ok(Number.isInteger(rungFromInput("2")), "as_u64 rejects a float too");
  // A FRACTIONAL input, which `parseFloat` would let through: `as_u64` rejects `2.5`
  // exactly as it rejects `"2"`, so it would ALSO mean Everything. No range input emits a
  // fraction with `step="1"`, but a keyboard, a restored preference or a future step of .5
  // could — and the failure is silent, so it is pinned rather than reasoned about. (This
  // was a live mutation survivor: swapping parseInt→parseFloat passed the whole suite.)
  assert.equal(rungFromInput("2.5"), 2, "truncated toward the SMALLER rung, not floated");
  assert.ok(Number.isInteger(rungFromInput("2.5")));
  assert.equal(JSON.stringify({ rung: rungFromInput("2.9") }), '{"rung":2}');
  assert.equal(JSON.stringify({ rung: rungFromInput("4.0") }), '{"rung":4}');
  // Clamped the SAME way Rung::from_wire clamps, and toward the same end: out of range or
  // unreadable → Everything, so what the widget shows and what the server does agree even
  // when the input is nonsense. (Doing silently LESS than asked is the worse error — the
  // reasoning is on ladder.rs's `impl Default for Rung`.)
  assert.equal(rungFromInput("0"), 0);
  assert.equal(rungFromInput("4"), 4);
  assert.equal(rungFromInput("9"), 4, "out of range → Everything");
  assert.equal(rungFromInput("-1"), 0, "below range → the first rung, not a crash");
  assert.equal(rungFromInput(""), 4, "unreadable → Everything, like an absent field");
  assert.equal(rungFromInput(undefined), 4);
  // And every clamped result must be a valid index into RUNGS — renderLadder reads
  // RUNGS[rung].emoji with no guard of its own.
  for (const v of ["-5", "0", "2", "4", "99", "", "x", null]) {
    assert.ok(RUNGS[rungFromInput(v)], `RUNGS[rungFromInput(${JSON.stringify(v)})] must exist`);
  }
});
