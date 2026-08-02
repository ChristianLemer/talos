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
import {
  RUNGS, rungAllows, rungFromInput, isSlow, SLOW_SECS, estimate, formatEstimate,
} from "../public/ladder.js";
// `rungReason` lives in model.js, beside `rungPlan`, and NOT in ladder.js — ladder.js is a
// declared TWIN of src/ladder.rs, and an extra export with no Rust half would make that
// claim false for the next reader who diffs them. It is tested here anyway, because it is
// the same rule read from the other side: rungPlan answers "which rows", rungReason answers
// "and why not the others", and the two must never be able to disagree.
import { rungReason } from "../public/model.js";

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

test("a row excluded by the rung says WHY, in the user's terms", () => {
  // The reason is the whole point: "greyed out" alone teaches nothing, and this is the
  // first place the collected facts reach the user's eye. The wording is about THEM
  // ("needs your hand"), not about the mechanism ("uac").
  const slowUp = { isConfig: false, uac: false, forbidden: false, slow: true };
  const uacUp = { isConfig: false, uac: true, forbidden: false, slow: false };
  const blocked = { isConfig: false, uac: false, forbidden: true, slow: false };
  const quiet = { isConfig: false, uac: false, forbidden: false, slow: false };

  // At ☕ Unattended, an upgrade that drags is out — because it is slow.
  assert.equal(rungReason(2, "upgrade", slowUp), "slow");
  // …one that will ask for elevation is out for a different reason.
  assert.equal(rungReason(2, "upgrade", uacUp), "hand");
  assert.equal(rungReason(2, "upgrade", blocked), "blocked");
  // A quiet upgrade is IN at ☕, so there is no reason to give.
  assert.equal(rungReason(2, "upgrade", quiet), null);
  // At 👀 the hand and the block are admitted; only slow remains out.
  assert.equal(rungReason(3, "upgrade", uacUp), null);
  assert.equal(rungReason(3, "upgrade", blocked), null);
  assert.equal(rungReason(3, "upgrade", slowUp), "slow");
  // At 🏗️ nothing is out, ever.
  for (const f of [slowUp, uacUp, blocked, quiet]) {
    assert.equal(rungReason(4, "upgrade", f), null);
  }
  // An UPGRADE excluded at rung 1 is not excluded by a fact at all — it is excluded by
  // being an upgrade. That distinction must survive, or a quiet upgrade at 📦 would
  // claim to be "slow".
  assert.equal(rungReason(1, "upgrade", quiet), "update");
  assert.equal(rungReason(1, "upgrade", slowUp), "update",
    "the ACTION is the reason here, not the fact — the fact is not why it is out");
  // An install is out at ⚡ for being an install, whatever its facts.
  assert.equal(rungReason(0, "install", quiet), "install");
  // ⚠️ And uninstall is NEVER out — the veto is honoured at every rung.
  for (let r = 0; r <= 4; r++) assert.equal(rungReason(r, "uninstall", slowUp), null);
});

test("a row the LADDER does not govern gets no reason at all", () => {
  // Beyond the plan's table, and the case that decides what the panel looks like on this
  // machine. `downgrade` and a null action (out of scope, or nothing to do) are out at
  // EVERY rung — so a reason would dim them at every position, never move when the slider
  // moves, and blame the ladder for a row it does not govern. On this Mac that is most of
  // the list, so it would read as "the ladder excluded 25 packages", which is false.
  // A dimmed row must be a row the slider can UN-dim by going right; nothing else.
  const quiet = { isConfig: false, uac: false, forbidden: false, slow: false };
  for (let r = 0; r <= 4; r++) {
    assert.equal(rungReason(r, "downgrade", quiet), null, "downgrade is Apply's business, not the rung's");
    assert.equal(rungReason(r, null, quiet), null, "no action → nothing for a rung to exclude");
  }
});

test("rungReason and rungAllows cannot disagree — a reason means OUT, exhaustively", () => {
  // The invariant that makes the dimming trustworthy, checked over the whole cross product
  // rather than at chosen points: a reason is present exactly when the rule refuses, EXCEPT
  // for the rows no rung admits (pinned above). And every reason returned must be one of
  // the keys app.js has words for — an unknown key would render an empty <span>, i.e. a
  // dimmed row with no explanation, which is the "greyed out teaches nothing" failure.
  const known = new Set(["slow", "hand", "blocked", "update", "install"]);
  for (const action of ["install", "uninstall", "upgrade", "downgrade", null]) {
    for (const isConfig of [false, true]) {
      for (const uac of [false, true]) {
        for (const forbidden of [false, true]) {
          for (const slow of [false, true]) {
            const p = { isConfig, uac, forbidden, slow };
            const governed = rungAllows(RUNGS.length - 1, action, isConfig, p);
            for (let r = 0; r < RUNGS.length; r++) {
              const why = rungReason(r, action, p);
              const allowed = rungAllows(r, action, isConfig, p);
              const what = `${action} isConfig=${isConfig} uac=${uac} 403=${forbidden} slow=${slow} @${r}`;
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
    }
  }
});

test("the reason names the fact that is ACTUALLY holding the row back", () => {
  // The defect the plan warns about, from the other direction: a word that contradicts the
  // rule. "slow" must mean "one rung right is 🏗️", and "hand"/"blocked" must mean "one rung
  // right is 👀" — verified by asking rungAllows, so the words cannot drift from the table.
  const facts = [
    { uac: false, forbidden: false, slow: true },
    { uac: true, forbidden: false, slow: false },
    { uac: false, forbidden: true, slow: false },
    { uac: true, forbidden: true, slow: true },
  ];
  for (const f of facts) {
    const p = { isConfig: false, ...f };
    for (let r = 0; r < RUNGS.length; r++) {
      const why = rungReason(r, "upgrade", p);
      if (why !== "slow" && why !== "hand" && why !== "blocked") continue;
      // The lowest rung that admits this row — the one the reason is a claim about.
      let first = RUNGS.length - 1;
      while (first > 0 && rungAllows(first - 1, "upgrade", false, p)) first--;
      if (why === "slow") {
        assert.equal(first, 4, `"slow" claims only 🏗️ admits it (facts ${JSON.stringify(f)})`);
        assert.equal(p.slow, true, `…and the row must really BE slow`);
      } else {
        assert.equal(first, 3, `"${why}" claims 👀 admits it (facts ${JSON.stringify(f)})`);
        assert.equal(why === "hand" ? p.uac : p.forbidden, true,
          `…and the row must really carry the fact "${why}" names`);
      }
    }
  }
});

test("every reason key is REACHABLE, and app.js's tooltip names a rung that really admits it", () => {
  // Two contracts a user can check with their own eyes, so both are checked by RUNNING the
  // rule rather than by reading strings.
  //
  // 1. Reachability. A key with no (rung, action, facts) that produces it is dead copy — five
  //    words maintained for a state that cannot occur, and the next reader would trust it.
  const reachable = new Set();
  for (const a of ["install", "uninstall", "upgrade", "downgrade", null]) {
    for (const isConfig of [false, true]) {
      for (const uac of [false, true]) {
        for (const forbidden of [false, true]) {
          for (const slow of [false, true]) {
            for (let r = 0; r < RUNGS.length; r++) {
              const k = rungReason(r, a, { isConfig, uac, forbidden, slow });
              if (k) reachable.add(k);
            }
          }
        }
      }
    }
  }
  assert.deepEqual([...reachable].sort(), ["blocked", "hand", "install", "slow", "update"]);

  // 2. app.js's tooltips NAME A RUNG ("Move to 👀 Stay nearby to include it"), which is a
  //    promise about what happens next — the same class of claim as `RUNGS[].promise`, and
  //    three of those five were caught being false. So the named rung must really admit the
  //    row, and the one below it must really not.
  const F = (o) => ({ isConfig: false, uac: false, forbidden: false, slow: false, ...o });
  const entersAt = (rung, action, p) =>
    rungAllows(rung, action, p.isConfig, p) && !rungAllows(rung - 1, action, p.isConfig, p);
  assert.ok(entersAt(4, "upgrade", F({ slow: true })),
    '"slow" says 🏗️ Everything — so 🏗️ must be exactly where a slow upgrade enters');
  assert.ok(entersAt(3, "upgrade", F({ uac: true })),
    '"needs your hand" says 👀 Stay nearby — so that is where a uac upgrade must enter');
  assert.ok(entersAt(3, "upgrade", F({ forbidden: true })),
    '"blocked here" says 👀 Stay nearby too');
  // "an update": the tooltip says this rung "installs and removes, but runs no updates".
  assert.ok(rungAllows(1, "install", false, F()) && rungAllows(1, "uninstall", false, F()),
    "…installs and removes: both must be true at 📦, or the sentence is wrong");
  assert.ok(!rungAllows(1, "upgrade", false, F()), "…but runs no updates");
  assert.ok(rungAllows(2, "upgrade", false, F()), "…and 'move right' must actually work");
  // "an install": the tooltip says this rung "applies config only".
  assert.ok(rungAllows(0, "install", true, F()), "…a config atom IS applied at ⚡");
  assert.ok(!rungAllows(0, "install", false, F()), "…and a plain install is not");
  assert.ok(rungAllows(1, "install", false, F()), "…'move right' works here too");
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
