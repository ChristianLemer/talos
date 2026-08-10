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

// The six rungs, in order. `promise` is the sentence under the thumb; `key` is the same
// word ladder.rs logs, so a log line and a widget position can be read against each other.
//
// ⚠️ The emojis answer "DO I HAVE TIME RIGHT NOW?", deliberately not "is this better?". A
// satisfaction ramp (🙁→😀) would assert that Everything is the good end and it is not:
// rung 0 is the only rung that CANNOT fail on the network, and on a Tuesday morning the
// right answer is usually rung 3. ☕ vs 👀 carries the actual difference between rungs 3
// and 4 — "you may leave" vs "you must stay" — better than any word would.
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
  // ⭐ Between ⚡ and 📦 because its promise sits between theirs: it DOES touch the network
  // (a clone), and it NEVER touches the machine — nothing enters Program Files, nothing
  // elevates, undoing it deletes a folder in your own profile. Config-only's promise is
  // strictly stronger (no network at all), a binary's strictly weaker (may elevate, may take
  // nine minutes). C's observation: adding a two-second skill is boring when it rides with Git.
  //
  // ⚠️ The promise names what it does NOT do, because that is the part the user is buying.
  // "Fast" would be the wrong word — a clone can be slow on a bad line; what is guaranteed is
  // WHERE it writes. And no duration is claimed: nothing has ever measured these routes
  // (timings.yaml holds five brew entries and nothing else), so the widget will honestly
  // count them as unknown.
  { key: "extensions", emoji: "🧩", name: "Extensions",
    promise: "Also plugins and skills. Downloaded into your profile — never installed on the machine" },
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

// Sum the local durations for a set of rows, and COUNT the rows that have none.
//
// ⚠️ Unknowns are counted and SHOWN, never silently omitted. A row with no local timing
// contributes nothing to the total, so an early estimate under-reports — and a number that
// reads as a prediction while being systematically wrong teaches people to distrust it.
// Same discipline that made the scan narrate "checking X, i of n" rather than freeze on a
// phrase.
//
// ⚠️ `0` is the "never measured HERE" sentinel and NEVER "it was instant" — timings.rs's
// `note_duration` floors a real measurement at one second precisely so this test can be a
// plain `s > 0`. So a warm-cache 1s row is KNOWN, and only a genuinely absent measurement
// is unknown. (Anything stricter — `s > 1`, a "too fast to be real" filter — would move
// every cached row into the unknown column and make the estimate read as if nothing had
// ever been measured on a machine where everything has.)
//
// No twin in ladder.rs, deliberately: the server holds the durations but never phrases
// them, and the estimate is a reading of a chosen rung, which only the front has.
export function estimate(secsPerRow) {
  let secs = 0, known = 0, unknown = 0;
  for (const s of secsPerRow) {
    if (s > 0) { secs += s; known++; } else { unknown++; }
  }
  return { secs, known, unknown };
}

// C's shape: "~N min for X and Y unknown".
//
// ⚠️ NOT "at most ~N min". The spec's worst-case framing was right for a total summed from
// the SHARED slow_secs, which is a max over the fleet. These minutes come from the LOCAL
// last-seen cache instead, which is neither a worst case nor an average — it is what it
// took HERE last time. Claiming "at most" would be a false claim about a number that can
// genuinely be exceeded, and the user is the one who would find out.
//
// The four readings, and why each is worded the way it is:
//
//   known>0, unknown>0 → "~4 min for 6 and 3 unknown". The minutes cover the 6 ONLY; the
//                        3 are named so the number is never mistaken for a total.
//   known>0, unknown=0 → "~4 min for all 4". "all" is said EXPLICITLY rather than dropping
//                        the clause: a reader must never have to wonder whether an unknown
//                        count was omitted or was zero. It is mildly redundant beside the
//                        row count and that redundancy is the price of the guarantee.
//   known=0, unknown>0 → "3 unknown". NO invented minutes — this is the ordinary reading on
//                        a machine that has not applied anything yet, and the honest answer
//                        is that we know what we would do and not how long it takes.
//   nothing at all      → "" (see below).
//
// ⚠️ The empty rung returns an EMPTY STRING, not the plan's "nothing to do". Verified on
// screen: `#ladder-blocked` already says "Nothing to do at this level. There is more further
// right." two lines below, so the plan's wording put the same phrase twice in one widget —
// and that note's own reasoning ("with nothing to do at all, the empty panel is the
// message") is the argument against a third. "" also cannot be wrong: with no rows there is
// no duration and no unknown to report. app.js drops the separator with it.
export function formatEstimate({ secs, known, unknown }) {
  if (known === 0 && unknown === 0) return "";
  if (known === 0) return `${unknown} unknown`;
  return `${duration(secs)} for ${unknown === 0 ? `all ${known}` : `${known} and ${unknown} unknown`}`;
}

// Seconds → the coarsest reading that is still true.
//
// ⚠️ "<1 min", never "~0 min": a 30s total rounds to 1 minute and a 29s total to ZERO, and
// "~0 min" reads as "instant" for something that is not. The threshold is on the SECONDS,
// before any rounding, so everything under a minute reads the same way.
//
// ⚠️ And hours are said in hours. "~127 min" is arithmetically fine and humanly useless —
// the question this widget asks is "do I have time RIGHT NOW?", and nobody converts 127
// minutes in their head to answer it. A fresh machine with Xcode CLT in the plan really is
// this case. The minutes are carried out of the total rather than computed per-part, so a
// 59.7-minute remainder cannot print as "1 h 60 min".
function duration(secs) {
  if (secs < 60) return "<1 min";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `~${mins} min`;
  const h = Math.floor(mins / 60), m = mins % 60;
  return m === 0 ? `~${h} h` : `~${h} h ${m} min`;
}

// May this action run at this rung? THE twin of ladder.rs::rung_allows.
//   rung: 0..4  ·  action: "install"|"uninstall"|"upgrade"|"downgrade"|null
export function rungAllows(rung, action, isConfig, isExtension, facts) {
  if (!action) return false; // no action (out of scope, nothing to do) is in no rung
  // NOT ON THE LADDER, at any rung: removing honours the user's ✕, and the ladder governs
  // how far to GO, not whether to honour a veto.
  if (action === "uninstall") return true;
  // Never batched by Apply (mirrors AUTO_ACTS and the server's Install|Uninstall|Upgrade).
  if (action === "downgrade") return false;
  // An extension installs INTO a host: rung 1, above config-atoms and below binaries. It
  // stays rung 1 even if a fact says it elevates — the ROUTE earns the rung, not the
  // observation (the Rust twin's table says the same, and pins it).
  if (action === "install") return isConfig ? true : isExtension ? rung >= 1 : rung >= 2;
  if (action === "upgrade") {
    // ⚠️ Every threshold moved up by one when Extensions was inserted at 1. slow DOMINATES:
    // the last rung is the only one that includes "the slow ones".
    const needed = isSlow(facts) ? 5 : (facts?.uac || facts?.forbidden) ? 4 : 3;
    return rung >= needed;
  }
  return false;
}
