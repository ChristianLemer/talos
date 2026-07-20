// decision.js — the PURE decision logic of Talos, with NO DOM and NO globals.
//
// This module is the single source of truth for two questions, shared by BOTH
// the browser (app.js, for pills + button lighting) AND the server (applyDiff,
// for actual execution). Keeping it here — pure, importable, testable — kills
// the front/server duplication that used to let the two drift apart.
//
// Everything is a pure function of its arguments: same inputs, same output, no
// side effects. That is what makes decision.test.mjs possible.
import { compareVersions } from "./version.js";

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
// Only `forbidden` locks (locked OUT — an integrator bans a package). `mandatory`
// no longer locks: nothing is indispensable (spec §4), so any package a bundle
// pulls can still be manually toggled out (the bundle then reads hollow).
export function isLockedPosture(posture) {
  return posture === "forbidden";
}

// The user's toggle for a package is "in" | "out" | null (untouched → follow the
// default). Locked postures ignore the toggle entirely (author wins). This is the
// effective in/out.
// Bundle-driven model: a package is "in" only if a bundle pulls it OR the user
// set it in by hand. Untouched + unpulled → out (nothing by default). posture no
// longer sets a default SIDE; only `forbidden` matters (locked out — an integrator
// can ban a package). `mandatory` is no longer "always in" (nothing is
// indispensable — plumbing is pulled via `requires`). See spec Consolidation §3-4.
export function toggleState(posture, userToggle, pulled) {
  if (posture === "forbidden") return "out"; // locked out, always
  if (userToggle === "in" || userToggle === "out") return userToggle; // manual wins
  return pulled ? "in" : "out";
}

// The DESIRED state (present|absent), from posture + the user's toggle + pull.
export function desiredState(posture, userToggle, pulled) {
  return toggleState(posture, userToggle, pulled) === "in" ? "present" : "absent";
}

// Deviation now = the user set a manual toggle (the only "off the default" signal
// in a bundle-driven model, where the default is simply "what the bundles pull").
export function isDeviation(posture, userToggle) {
  if (posture === "forbidden") return false;
  return userToggle === "in" || userToggle === "out";
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

// A bundle's derived state — computed PURELY from its packages' wanted-ness, not
// from whether the card was clicked. `isIn(pkgKey)` = is that package effectively
// "in" right now (caller composes posture + toggle + bundle pull). So a bundle
// whose packages are all wanted (even via manual picks that happen to cover it)
// reads `full`; some wanted → `hollow`; none → `off`. The card never lies about
// its members. `active` is kept in the signature for callers but no longer gates
// the visual (an empty bundle with no packages is trivially off).
export function profileState(profile, _active, isIn) {
  const pkgs = profile.packages ?? [];
  if (pkgs.length === 0) return "off";
  if (pkgs.every((key) => isIn(key))) return "full";
  if (pkgs.some((key) => isIn(key))) return "hollow";
  return "off";
}

// Given a DESIRED state and the machine reality, the action a plain Apply would
// take — or null if nothing to do. THIS is the rule the server executes and the
// front previews; they must agree, so they call the same function.
//   desired present & absent                     → install
//   desired present & present & below pin        → upgrade   (to the pin)
//   desired present & present & above pin         → downgrade (to the pin) — MANUAL
//   desired present & present & stale (no pin)    → upgrade   (to latest)
//   desired absent  & present                     → uninstall (only if removable)
//
// A `pin` (exact version declared in the YAML) REFRAMES "outdated": the reference
// is no longer "latest" but the pin, so the machine-wide outdated flag is ignored
// when a pin is present. compareVersions(installed, pin) decides the direction:
//   installed <  pin → upgrade    (Apply runs it — install/upgrade only)
//   installed == pin → null       (satisfied — the whole point of an exact pin)
//   installed >  pin → downgrade  (a distinct action Apply FILTERS OUT: it's the
//                                   only destructive path — uninstall+install — so
//                                   it's a manual per-row button, never batched)
export function actionFor(
  desired,
  { present, outdated, canUninstall, pin, installedVersion },
) {
  if (desired === "present" && !present) return "install";
  if (desired === "present" && present) {
    if (pin) {
      const cmp = compareVersions(installedVersion || "", pin);
      if (cmp < 0) return "upgrade";
      if (cmp > 0) return "downgrade";
      return null; // at the pin → satisfied
    }
    if (outdated) return "upgrade";
  }
  if (desired === "absent" && present && canUninstall) return "uninstall";
  return null;
}
