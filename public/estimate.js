// estimate.js — how LONG an Apply would take here, PURE: no DOM, no model, no globals.
//
// ⭐ This module was ladder.js until 2026-10-10. The ladder (three cumulative rungs —
// ⚡ config only, 🧩 extensions, 📦 apps — choosing which actions one Apply runs) left: applying
// PART of a desire built states nothing reasons about, such as a fresh config over a stale app,
// and nobody ever moved the slider. What stays is what never depended on a rung: the slow threshold
// and the estimate, which the Apply button now shows for the whole plan.
//
// Named twins, so a reader can find the other half: `isSlow` ↔ `plan_facts::is_slow`,
// `SLOW_SECS` ↔ `plan_facts::SLOW_SECS`.

// The threshold the SERVER applies. Exported for documentation and for the test that says
// so — the front never compares seconds, it is told `slow` as a boolean, which is what
// stops the two sides from drifting on a number.
export const SLOW_SECS = 60;

// Is this package classified slow? The verdict arrives on the wire as a boolean, computed
// by the server from the SHARED slow_secs (a max over the fleet, so the label means the
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
// No twin in plan_facts.rs, deliberately: the server holds the durations but never phrases
// them, and the estimate is a reading of a plan, which only the front shows.
export function estimate(secsPerRow) {
  let secs = 0, known = 0, unknown = 0;
  for (const s of secsPerRow) {
    if (s > 0) { secs += s; known++; } else { unknown++; }
  }
  return { secs, known, unknown };
}

// C's shape: "~N min for X, Y never timed here". Until 2026-10-27 it read "X and Y unknown",
// which on the old one-line button sat after the row counts and read as "Y unknown rows";
// in the bar's line it is said for what it is — rows with no duration on THIS machine.
//
// ⚠️ NOT "at most ~N min". The spec's worst-case framing was right for a total summed from
// the SHARED slow_secs, which is a max over the fleet. These minutes come from the LOCAL
// last-seen cache instead, which is neither a worst case nor an average — it is what it
// took HERE last time. Claiming "at most" would be a false claim about a number that can
// genuinely be exceeded, and the user is the one who would find out.
//
// The four readings, and why each is worded the way it is:
//
//   known>0, unknown>0 → "~4 min for 6, 3 never timed here". The minutes cover the 6 ONLY;
//                        the 3 are named so the number is never mistaken for a total.
//   known>0, unknown=0 → "~4 min for all 4". "all" is said EXPLICITLY rather than dropping
//                        the clause: a reader must never have to wonder whether an unknown
//                        count was omitted or was zero. It is mildly redundant beside the
//                        row count and that redundancy is the price of the guarantee.
//   known=0, unknown>0 → "3 never timed here". NO invented minutes — this is the ordinary
//                        reading on a machine that has not applied anything yet, and the honest answer
//                        is that we know what we would do and not how long it takes.
//   nothing at all      → "" (see below).
//
// ⚠️ An empty plan returns an EMPTY STRING, not "nothing to do": the list's own empty state
// (`#steps-empty`) already says so, and the same phrase twice is noise. "" also cannot be
// wrong: with no rows there is no duration and no unknown to report.
export function formatEstimate({ secs, known, unknown }) {
  if (known === 0 && unknown === 0) return "";
  if (known === 0) return `${unknown} never timed here`;
  return `${duration(secs)} for ${unknown === 0 ? `all ${known}` : `${known}, ${unknown} never timed here`}`;
}

// Seconds → the coarsest reading that is still true.
//
// ⚠️ "<1 min", never "~0 min": a 30s total rounds to 1 minute and a 29s total to ZERO, and
// "~0 min" reads as "instant" for something that is not. The threshold is on the SECONDS,
// before any rounding, so everything under a minute reads the same way.
//
// ⚠️ And hours are said in hours. "~127 min" is arithmetically fine and humanly useless —
// the question an estimate answers is "do I have time RIGHT NOW?", and nobody converts 127
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
