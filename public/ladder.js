// ladder.js — how FAR an Apply goes, PURE: no DOM, no model, no globals. A function of
// facts, like scope.js and decision.js.
//
// ⚠️ THIS IS A TWIN of src/ladder.rs. The rule that GOVERNS the plan is the Rust one — the
// server filters when it builds `visual_plan`, because `row_action` never sees the wire lists
// and a front-only filter would be cosmetic (the Git-hazard lesson). This copy exists for
// ONE job: showing how many rows each rung would touch, live, before you commit to it.
// So a drift between the two costs a wrong COUNT, never a wrong action. Same relation
// model.js's AUTO_ACTS already has with the server's action filter, and the same
// obligation: change one, change the other, and keep the two test tables identical.
//
// Named twins, so a reader can find the other half: `rungAllows` ↔ `ladder::rung_allows`,
// `isSlow` ↔ `ladder::is_slow`, `SLOW_SECS` ↔ `ladder::SLOW_SECS`, `RUNGS[n].key` ↔
// `ladder::Rung::as_str`, and `rungFromInput` ↔ `Rung::from_wire` (which
// `server::rung_from_wire` calls).

// The threshold the SERVER applies. Exported for documentation and for the test that says
// so — the front never compares seconds, it is told `slow` as a boolean, which is what
// stops the two sides from drifting on a number.
export const SLOW_SECS = 60;

// The five rungs, in order. `promise` is the sentence under the thumb; `key` is the same
// word ladder.rs logs, so a log line and a widget position can be read against each other.
//
// ⚠️ The emojis answer "DO I HAVE TIME RIGHT NOW?", deliberately not "is this better?". A
// satisfaction ramp (🙁→😀) would assert that Everything is the good end and it is not:
// rung 0 is the only rung that CANNOT fail on the network, and on a Tuesday morning the
// right answer is usually rung 2. ☕ vs 👀 carries the actual difference between rungs 2
// and 3 — "you may leave" vs "you must stay" — better than any word would.
// ⚠️ AND EVERY PROMISE MUST BE TRUE OF WHAT THE SERVER DOES AT THAT RUNG — the promise is
// the only thing the user reads before committing, so an overstatement here is a lie the
// code then tells. Rung 0's was caught being one: the plan wrote "Instant, and the only
// rung that cannot fail on the network", which is false, because `uninstall` is on EVERY
// rung (see `rungAllows`) and `brew uninstall` / `winget uninstall --source winget` is
// neither instant nor offline. The promise now speaks about what the rung ADDS, and names
// the removals rather than quietly contradicting them.
export const RUNGS = [
  { key: "config-only", emoji: "⚡", name: "Config only",
    promise: "Adds config only — nothing is downloaded. Removals you asked for still run" },
  { key: "add-missing", emoji: "📦", name: "Add missing",
    promise: "Installs what is absent. No updates" },
  { key: "unattended", emoji: "☕", name: "Unattended",
    // ⚠️ "Updates that…", not "nothing will interrupt you". The uac/403 gate governs
    // UPGRADES only — an INSTALL flagged `uac` is admitted from rung 1 (ladder.rs's table
    // says so in as many words: "an install of something known to elevate is STILL rung 1",
    // because an install was explicitly asked for). So a bare "walk away" would promise
    // something rung 1 already broke. The ☕ still carries the intent; the words stay exact.
    promise: "Also the updates that ask you nothing. Only new installs may still prompt" },
  { key: "stay-nearby", emoji: "👀", name: "Stay nearby",
    promise: "Also updates that need your hand, or a page unblocked" },
  { key: "everything", emoji: "🏗️", name: "Everything",
    promise: "Also the slow ones. Allow time" },
];

// A range input's `.value` is a STRING. This turns it into the integer the wire needs.
//
// ⚠️ NOT cosmetic. `server::rung_from_wire` reads the field with serde's `as_u64`, which
// rejects a JSON string AND a JSON float, and its None arm lands on `Rung::Everything`. So
// sending `"2"` would run EVERYTHING while the widget said "Unattended · start it and walk
// away" — the widget lying about the plan, silently, in the direction of doing more.
// `parseInt` + a clamp is the whole fix, and test/ladder.test.mjs asserts the serialised
// bytes rather than the JS value, because that is what serde sees.
//
// The clamp mirrors `Rung::from_wire`: out of range or unreadable → the LAST rung, because
// doing silently less than the user asked for is the worse direction of error (the
// reasoning is on ladder.rs's `impl Default for Rung`). Below range → the first rung, which
// no real input produces but which must still be a valid index.
export function rungFromInput(value) {
  const n = parseInt(value, 10);
  if (!Number.isFinite(n)) return RUNGS.length - 1;
  if (n < 0) return 0;
  if (n > RUNGS.length - 1) return RUNGS.length - 1;
  return n;
}

// Is this package classified slow? The verdict arrives on the wire as a boolean, computed
// by the server from the SHARED slow_secs (a max over the fleet, so the rung means the
// same thing on every machine). The minutes shown to this user come from a LOCAL last-seen
// cache instead — so a row can be `slow` here and still read "~2 min", because this
// machine's cache is warm. Two questions, two answers.
export function isSlow(facts) {
  return facts?.slow === true;
}

// May this action run at this rung? THE twin of ladder.rs::rung_allows.
//   rung: 0..4  ·  action: "install"|"uninstall"|"upgrade"|"downgrade"|null
export function rungAllows(rung, action, isConfig, facts) {
  if (!action) return false; // no action (out of scope, nothing to do) is in no rung
  // NOT ON THE LADDER, at any rung: removing honours the user's ✕, and the ladder governs
  // how far to GO, not whether to honour a veto.
  if (action === "uninstall") return true;
  // Never batched by Apply (mirrors AUTO_ACTS and the server's Install|Uninstall|Upgrade).
  if (action === "downgrade") return false;
  if (action === "install") return isConfig ? true : rung >= 1;
  if (action === "upgrade") {
    // slow DOMINATES: rung 3 excludes "the slow ones" full stop.
    const needed = isSlow(facts) ? 4 : (facts?.uac || facts?.forbidden) ? 3 : 2;
    return rung >= needed;
  }
  return false;
}
