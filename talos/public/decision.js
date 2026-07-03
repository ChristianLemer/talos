// decision.js — the PURE decision logic of Talos, with NO DOM and NO globals.
//
// This module is the single source of truth for two questions, shared by BOTH
// the browser (app.js, for pills + button lighting) AND the server (applyDiff,
// for actual execution). Keeping it here — pure, importable, testable — kills
// the front/server duplication that used to let the two drift apart.
//
// Everything is a pure function of its arguments: same inputs, same output, no
// side effects. That is what makes decision.test.mjs possible.

// The four postures an author may declare (chezmoi convention).
export const POSTURES = ["mandatory", "opt-out", "opt-in", "forbidden"];

// Resolve the posture that applies to a package: its own if valid, else the
// bundle's default if valid, else "mandatory" (the safe, non-negotiable default).
export function resolvePosture(pkgPosture, bundlePosture) {
  if (POSTURES.includes(pkgPosture)) return pkgPosture;
  if (POSTURES.includes(bundlePosture)) return bundlePosture;
  return "mandatory";
}

// The DESIRED state (present|absent) of a package, resolving posture + the
// user's effective override ("auto" | "on" | "off").
//   mandatory → always present, forbidden → always absent (author wins).
//   opt-out   → present unless the user says off.
//   opt-in    → absent  unless the user says on.
//   "auto"    → follow the posture's own default.
export function desiredState(posture, override) {
  if (posture === "mandatory") return "present";
  if (posture === "forbidden") return "absent";
  if (override === "on") return "present";
  if (override === "off") return "absent";
  return posture === "opt-out" ? "present" : "absent";   // auto → posture default
}

// Given a DESIRED state and the machine reality, the action a plain Apply would
// take — or null if nothing to do. THIS is the rule the server executes and the
// front previews; they must agree, so they call the same function.
//   desired present & absent           → install
//   desired present & present & stale  → upgrade
//   desired absent  & present          → uninstall (only if removable)
export function actionFor(desired, { present, outdated, canUninstall }) {
  if (desired === "present" && !present) return "install";
  if (desired === "present" && present && outdated) return "upgrade";
  if (desired === "absent" && present && canUninstall) return "uninstall";
  return null;
}

// True when a package is locked by its posture — the author decided, the user
// cannot override (no cyclable pill).
export function isLockedPosture(posture) {
  return posture === "mandatory" || posture === "forbidden";
}
