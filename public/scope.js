// scope.js — the SECOND axis, pure: "is this row mine to manage?"
//
// The desire toggle (decision.js) answers ONE question — what do I want: ✕ out /
// ● auto / ✓ in. Two situations refuse to sit on that axis: a package with no way
// back, and a package we did not install. They answer a DIFFERENT question, so
// they get a different control. This module is that question's rule, and nothing
// else: no DOM, no model, no globals — a function of facts, like decision.js.
//
// TWO LAYERS, the same convention manualToggle/posture already use:
//   derived — !canUninstall (no way back) or external (installed elsewhere) → out
//   manual  — the user's switch: "in" | "out" | absent = follow the derivation
//
// WHY !canUninstall and NOT isConfig: isConfig says what a package IS; scope asks
// whether Talos can take responsibility for it. The real cleavage is whether the
// frontier is inscribed in the file — bun-path.nu writes between sentinels and
// `strip-managed` already exists (a way back, merely unwired), while starship.nu
// upserts into a record where our key is indistinguishable from the user's own (no
// way back, ever). `canUninstall` already encodes exactly that bit, so scope needs
// no knowledge of config-atoms at all. Wiring runUninstall into bun-path later
// pulls that row back into scope with ZERO lines here touched — which is the test
// of whether this derivation is the right one.

// Would Talos derive this row out of scope on its own, ignoring the user?
export function derivedOut({ canUninstall, external }) {
  return !canUninstall || external === true;
}

// The effective scope: "in" | "out". A manual "in"/"out" wins; anything else
// (null, undefined, garbage from a hand-edited selection.json) → the derivation.
export function scopeOf(facts) {
  const m = facts.manual;
  if (m === "in" || m === "out") return m;
  return derivedOut(facts) ? "out" : "in";
}

// WHY a row is out of scope — the word the row shows on the RIGHT (observation
// side). Returns null when the row is in scope, so the caller keeps its normal
// label. A DERIVED reason outranks the manual one: "external" tells the operator
// why, "not managed" only tells them what they already did.
//
// `isConfig` survives ONLY here — as a label discriminator, never in the rule.
// And it cannot separate our two atoms on its own (Starship config and Bun PATH
// are BOTH config-atoms): what separates them is whether a bundle pulls the row.
export function scopeReason(facts) {
  if (scopeOf(facts) === "in") return null;
  if (facts.external === true) return "external";
  if (facts.isConfig) return facts.pulled ? "yours" : "available";
  if (!facts.canUninstall) return "no-route"; // no route on this platform
  return "not-managed"; // manageable, but the user pushed it out
}
