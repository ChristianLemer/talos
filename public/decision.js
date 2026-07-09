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

// A posture is a two-level thing: WHERE its toggle starts (the author's default,
// "in" | "out") and WHETHER the user may move it. Guillaume's model: the only
// user choice is a binary in/out toggle; the posture just decides the default
// side and whether it's locked.
//   mandatory → default in,  locked   (always present)
//   opt-out   → default in,  free      (present unless you flip out)
//   opt-in    → default out, free      (absent unless you flip in)
//   forbidden → default out, locked    (always absent)
export function postureDefault(posture) {
  return (posture === "mandatory" || posture === "opt-out") ? "in" : "out";
}
export function isLockedPosture(posture) {
  return posture === "mandatory" || posture === "forbidden";
}

// The user's toggle for a package is "in" | "out" | null (untouched → follow the
// default). Locked postures ignore the toggle entirely (author wins). This is the
// effective in/out.
export function toggleState(posture, userToggle) {
  if (isLockedPosture(posture)) return postureDefault(posture);
  return userToggle === "in" || userToggle === "out" ? userToggle : postureDefault(posture);
}

// The DESIRED state (present|absent), from posture + the user's toggle.
export function desiredState(posture, userToggle) {
  return toggleState(posture, userToggle) === "in" ? "present" : "absent";
}

// Has the user DEVIATED from the author's default? (drives the "vivid when moved,
// neutral when at default" colouring). Locked → never a deviation.
export function isDeviation(posture, userToggle) {
  if (isLockedPosture(posture)) return false;
  return toggleState(posture, userToggle) !== postureDefault(posture);
}

// --- Profiles: named, additive package sets ---------------------------------
//
// A profile is a bundle-independent SELECTION: a named list of package keys the
// user applies as a group (like gus's distributions, but at package granularity
// and purely additive). Applying a profile pulls its packages "in"; a manual
// "out" always wins over that pull. A profile carries NO machine state — which
// profiles are ACTIVE is intention (persisted like the toggles, legitimate under
// detect-don't-remember), but a profile's VISUAL state is DERIVED live from the
// toggles. Three states:
//   off    → not active (user hasn't applied it)
//   full   → active AND every one of its packages is effectively "in"
//   hollow → active BUT the user has manually pulled at least one package "out"
export const PROFILE_STATES = ["off", "full", "hollow"];

// The effective user toggle for a package, accounting for active profiles. A
// manual toggle always wins (the user's explicit in/out). Otherwise, if any
// active profile lists the package, it is pulled "in". Otherwise untouched
// (null → the package follows its posture default). This is what lets a profile
// be ADDITIVE (it only ever pulls in, never forces out) while a manual "out"
// still overrides it — the exact rule that makes a profile go "hollow".
export function effectiveToggle(userToggle, inActiveProfile) {
  if (userToggle === "in" || userToggle === "out") return userToggle;
  return inActiveProfile ? "in" : null;
}

// A profile's derived state. `active` = is it in the active set; `isIn(pkgKey)`
// = is that package effectively "in" right now (caller composes posture + toggle
// + profiles). Pure: same inputs, same output.
export function profileState(profile, active, isIn) {
  if (!active) return "off";
  const pkgs = profile.packages ?? [];
  return pkgs.every((key) => isIn(key)) ? "full" : "hollow";
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
