// model.js — the Talos panel STATE, with ZERO DOM. The single source of truth
// the view (app.js) projects from. Pure and importable, so it is unit-tested by
// deno test just like decision.js. Rules live in decision.js; this module holds
// the state and applies those rules to it.
import {
  actionFor,
  desiredState,
  effectiveToggle,
  isDeviation,
  isLockedPosture,
  profileState,
  toggleState,
} from "./decision.js";
import { compareVersions } from "./version.js";

export function createModel() {
  return {
    pkgs: new Map(), // i -> PkgRecord
    bundles: new Map(), // name -> BundleRecord
    decision: new Map(), // i -> "in" | "out"  (absence = auto)
    profiles: new Map(), // name -> ProfileRecord {name, emoji, description, packages}
    activeProfiles: new Set(), // names of profiles the user has applied
    detectedAt: null, // ms of the last full presence scan (future TTL home)
  };
}

// Build the model from the server `plan` message (bundles + flat steps + profiles).
export function loadPlan(model, bundles, steps, profiles = []) {
  model.pkgs.clear();
  model.bundles.clear();
  model.profiles.clear();
  model.activeProfiles.clear();
  for (const p of profiles) {
    model.profiles.set(p.name, {
      name: p.name,
      emoji: p.emoji || "🎯",
      description: p.description || "",
      usage: p.usage || p.description || "",
      highlights: p.highlights ?? [],
      packages: p.packages ?? [],
    });
  }
  for (const b of bundles) {
    model.bundles.set(b.name, {
      name: b.name,
      emoji: b.emoji || "📦",
      description: b.description || "",
      posture: b.posture || "mandatory",
      selectable: b.selectable !== false,
      pkgIds: [],
    });
  }
  for (const s of steps) {
    model.pkgs.set(s.i, {
      i: s.i,
      name: s.name,
      description: s.description || "",
      bundle: s.bundle,
      posture: s.posture || "mandatory",
      canUninstall: !!s.canUninstall,
      isConfig: !!s.isConfig,
      present: null,
      outdated: false,
      // Version pinning: `pin` is the exact reference declared in the YAML (null
      // when unpinned); `installedVersion` is what the machine reports live (set
      // by the `state` message). actionOf compares the two. See talos-version-pin.
      pin: s.pin || null,
      installedVersion: "",
      available: "", // the newer version the machine-wide scan found (unpinned only)
      status: "waiting",
      log: "",
      // Bundle-driven: steps come from the flat catalog (bundle=""), so identity
      // is the package name (unique per catalog file), not `${bundle}::${name}`.
      // Old selection.json keyed the old way → won't match → clean reset (only
      // intent is lost; machine reality is re-scanned). Spec Consolidation §6.
      key: s.name,
    });
    const be = model.bundles.get(s.bundle);
    if (be) be.pkgIds.push(s.i);
  }
}

// --- per-package queries (delegate rules to decision.js) --------------------
export function postureOf(model, i) {
  return model.pkgs.get(i)?.posture ?? "mandatory";
}
// The RAW manual toggle: only what the user set by hand ("in"/"out"/null).
export function manualToggle(model, i) {
  const d = model.decision.get(i);
  return d === "in" || d === "out" ? d : null;
}
// Is this package pulled "in" by any ACTIVE profile? Profiles reference packages
// by name; a package is pulled if an active profile lists its name.
export function inActiveProfile(model, i) {
  const p = model.pkgs.get(i);
  if (!p || model.activeProfiles.size === 0) return false;
  for (const name of model.activeProfiles) {
    const prof = model.profiles.get(name);
    if (prof && prof.packages.includes(p.name)) return true;
  }
  return false;
}
// The EFFECTIVE toggle used everywhere downstream: a manual toggle wins; else an
// active profile pulls it "in"; else null (follow posture). This is what makes a
// profile additive yet overridable — the rule lives in decision.effectiveToggle.
export function userToggle(model, i) {
  return effectiveToggle(manualToggle(model, i), inActiveProfile(model, i));
}
export function isLocked(model, i) {
  return isLockedPosture(postureOf(model, i));
}
// Bundle-driven: the decision fns take (posture, manualToggle, pulled) — pulled =
// "an active bundle wants this package". A manual toggle wins; else the pull
// decides; else out. (profiles ARE the bundles now, so inActiveProfile = pulled.)
export function toggleOf(model, i) {
  return toggleState(postureOf(model, i), manualToggle(model, i), inActiveProfile(model, i));
}
export function desiredOf(model, i) {
  return desiredState(postureOf(model, i), manualToggle(model, i), inActiveProfile(model, i));
}
export function isDeviated(model, i) {
  return isDeviation(postureOf(model, i), manualToggle(model, i));
}
// The action a plain Apply would take here, or null. present:null → treated as
// "not known present" (false), the safe direction for install.
export function actionOf(model, i) {
  const p = model.pkgs.get(i);
  if (!p) return null;
  return actionFor(desiredOf(model, i), {
    present: p.present === true,
    outdated: p.outdated,
    canUninstall: p.canUninstall,
    pin: p.pin,
    installedVersion: p.installedVersion,
  });
}
// The row's version display — the SINGLE source of truth for what numbers show,
// so nothing can double up (the old bug: statusLabel showed the installed version
// AND the delta showed cur→avail, repeating the current one). Returns at most two
// numbers, marks the pin exactly once, and says which target is a "push" vs a
// muted "for info":
//   { from, to, pinned, muted }
//     from   — the installed version (left number), "" when absent
//     to      — the target (right number), or null when there's nothing to show
//     pinned  — which number IS the pin: "from" (at the pin), "to" (migrating to
//               the pin), or null (unpinned). The view badges that number 📌.
//     muted   — true when `to` is shown FOR INFO ONLY, not as a push: an upgrade
//               that exists past a pin you're already sitting on. The view greys it.
// Cases:
//   absent                          → { from:"",  to:null,  pinned:null, muted:false }
//   unpinned, up to date             → { from:inst, to:null,  pinned:null, muted:false }
//   unpinned, outdated               → { from:inst, to:avail, pinned:null, muted:false }
//   pinned, installed == pin, no upg → { from:inst, to:null,  pinned:"from", muted:false }
//   pinned, installed == pin, upg    → { from:inst, to:avail, pinned:"from", muted:true }
//   pinned, installed != pin         → { from:inst, to:pin,   pinned:"to",  muted:false }
// A pin OWNS the direction: when you're AT the pin, a newer version isn't a
// migration target (we don't push past your pin) — it's shown greyed so you can
// still choose to test it. See talos-version-pin.
export function versionSummary(model, i) {
  const none = { from: "", to: null, pinned: null, muted: false };
  const p = model.pkgs.get(i);
  if (!p) return none;
  const inst = p.present === true ? (p.installedVersion || "") : "";
  if (!inst) return none;
  if (p.pin) {
    const cmp = compareVersions(inst, p.pin);
    if (cmp !== 0) {
      // Off the pin → migration TO the pin. BELOW (cmp<0) is an upgrade Apply
      // runs → not muted (a push). ABOVE (cmp>0) is a downgrade — manual only,
      // never batched by Apply → muted, same "not pushed" grey as an upgrade
      // past a pin. Mirrors AUTO_ACTS excluding downgrade. See talos-version-pin.
      return { from: inst, to: p.pin, pinned: "to", muted: cmp > 0 };
    }
    // At the pin: normally just the pinned number. But if the machine-wide scan
    // found something newer, SHOW it greyed (muted) — visible to test, not pushed.
    if (p.outdated && p.available && compareVersions(p.available, p.pin) > 0) {
      return { from: inst, to: p.available, pinned: "from", muted: true };
    }
    return { from: inst, to: null, pinned: "from", muted: false };
  }
  // Unpinned: show the machine-wide available only when actually outdated.
  if (p.outdated && p.available) {
    return { from: inst, to: p.available, pinned: null, muted: false };
  }
  return { from: inst, to: null, pinned: null, muted: false };
}
// Would a PLAIN APPLY act here? Mirrors the server's AUTO_ACTS filter exactly:
// install/uninstall/upgrade are batched by Apply; "downgrade" is NOT (it's the
// destructive path, a manual per-row button only). Keeping this in lockstep with
// the server is what stops the button-preview from lighting for a change Apply
// won't make. See talos-version-pin.
const AUTO_ACTS = ["install", "uninstall", "upgrade"];
export function isActionable(model, i) {
  return AUTO_ACTS.includes(actionOf(model, i));
}
// The manual invert action a row button performs (label + direction + msg type).
// ALWAYS inverts current machine state: absent→install, present→uninstall,
// present-but-stale→update. null only when present and un-uninstallable.
export function buttonAction(model, i) {
  const p = model.pkgs.get(i);
  if (!p) return null;
  // A config-atom the user turned OFF is self-managed: the button inspects
  // (diff) rather than installs — it has no install path, and re-applying would
  // clobber the file the user chose to own.
  if (p.isConfig && desiredOf(model, i) === "absent") {
    return { verb: "diff", dir: "", type: "diff" };
  }
  // A pinned & present package: the action is the direction to the pin. This wins
  // over the outdated branch below (a pin is the reference, not "latest"). The
  // "downgrade" button is the ONLY way to run a downgrade — Apply never does it.
  if (p.present && p.pin) {
    const a = actionOf(model, i);
    if (a === "downgrade") {
      return { verb: "downgrade", dir: "remove", type: "downgrade" };
    }
    if (a === "upgrade") return { verb: "update", dir: "add", type: "upgrade" };
    // at the pin → satisfied: fall through to the present/uninstall logic below.
  }
  // Desired-ABSENT wins over "outdated → update": you don't update something you
  // asked to remove. So a present, uninstallable package the user turned out reads
  // uninstall even when a newer version exists (otherwise the button said "update"
  // while the plan said uninstall — button/text contradiction). Only offer update
  // for a package that's staying (desired present).
  if (p.present && p.canUninstall && desiredOf(model, i) === "absent") {
    return { verb: "uninstall", dir: "remove", type: "uninstall" };
  }
  if (p.present && p.outdated) {
    return { verb: "update", dir: "add", type: "upgrade" };
  }
  if (p.present) {
    return p.canUninstall
      ? { verb: "uninstall", dir: "remove", type: "uninstall" }
      : null;
  }
  return { verb: "install", dir: "add", type: "install" };
}

// --- mutations --------------------------------------------------------------
export function setDecision(model, i, state) {
  if (isLocked(model, i)) return false; // author's posture wins
  if (state === "in" || state === "out") model.decision.set(i, state);
  else model.decision.delete(i); // anything else clears to auto
  return true;
}
// Reset: back to posture defaults — clears BOTH manual toggles AND active
// profiles (profiles are additive intention; a true reset drops them too).
export function clearAllDecisions(model) {
  model.decision.clear();
  model.activeProfiles.clear();
}
export function setPresence(model, i, present) {
  const p = model.pkgs.get(i);
  if (!p) return;
  p.present = present;
  if (present !== true) {
    p.outdated = false;
    p.available = "";
    p.installedVersion = ""; // no longer present → no installed version to compare
  }
}
// The version the machine reports for a package right now (from the `state`
// probe). Feeds the pin comparison in actionOf. Empty string = unknown.
export function setInstalledVersion(model, i, version) {
  const p = model.pkgs.get(i);
  if (p) p.installedVersion = version || "";
}
// Settle a package's status; ok/absent/waiting also update presence.
export function setStatusData(model, i, status) {
  const p = model.pkgs.get(i);
  if (!p) return;
  p.status = status;
  if (status === "ok") {
    p.present = true;
    p.outdated = false;
    p.available = "";
  } else if (status === "absent" || status === "waiting") {
    p.present = false;
    p.outdated = false;
    p.available = "";
    p.installedVersion = "";
  }
}
// Mark a present package stale, carrying the version the scan found available so
// the display can show cur→avail without a second event. `available` optional
// (kept for older callers / the boolean-only case).
export function setOutdated(model, i, on, available = "") {
  const p = model.pkgs.get(i);
  if (!p) return;
  p.outdated = !!on;
  p.available = on ? (available || "") : "";
}
export function appendLog(model, i, text) {
  const p = model.pkgs.get(i);
  if (p) p.log += text;
}
export function resetLog(model, i) {
  const p = model.pkgs.get(i);
  if (p) p.log = "";
}

// --- bundle queries ---------------------------------------------------------
export function bundleLocked(model, name) {
  const be = model.bundles.get(name);
  if (!be) return true;
  if (!be.selectable) return true;
  return be.pkgIds.length > 0 && be.pkgIds.every((i) => isLocked(model, i));
}
// Three-state toggle for a bundle: all in → "on-in", all out → "on-out",
// a real mix → "mixed". Empty free-list → "on-in" (nothing to mix).
// Returns tokens WITHOUT a leading space; the view adds the space when it
// concatenates the className (so don't reintroduce a " mixed" comparison).
export function bundleToggleState(model, name) {
  const be = model.bundles.get(name);
  if (!be) return "on-in";
  const free = be.pkgIds.filter((i) => !isLocked(model, i));
  if (free.length === 0) return "on-in"; // all locked → default in
  const allIn = free.every((i) => toggleOf(model, i) === "in");
  const allOut = free.every((i) => toggleOf(model, i) === "out");
  if (allIn || allOut) return (allOut && !allIn) ? "on-out" : "on-in";
  return "mixed";
}
export function bundleAnyActionable(model, name) {
  const be = model.bundles.get(name);
  return !!be && be.pkgIds.some((i) => isActionable(model, i));
}
// Aggregate colour direction for a bundle: "add" if any pkg would install/update,
// else "remove" if any would remove, else "".
export function bundleAct(model, name) {
  const be = model.bundles.get(name);
  if (!be) return "";
  let removes = false;
  for (const i of be.pkgIds) {
    const a = actionOf(model, i);
    if (a === "install" || a === "upgrade") return "add";
    if (a === "uninstall") removes = true;
  }
  return removes ? "remove" : "";
}

// --- profiles ---------------------------------------------------------------
// Apply a profile: mark it active (it then pulls its packages "in" via
// userToggle). Additive — it never writes decisions and never forces anything
// out, so a manual "out" still wins (that turns the profile hollow).
export function applyProfile(model, name) {
  if (model.profiles.has(name)) model.activeProfiles.add(name);
}
// Remove a profile from the active set: its pull vanishes, but a package it
// shared with another active profile or a manual "in" survives (userToggle
// recomputes from what remains). This is the clean per-profile removal that
// spares the user from redoing everything by hand.
export function removeProfile(model, name) {
  model.activeProfiles.delete(name);
}
export function isProfileActive(model, name) {
  return model.activeProfiles.has(name);
}
// Would a Reset do anything? True if the user moved a package off its author
// default OR has an active profile — both are cleared by clearAllDecisions. The
// domain answer to "is Reset meaningful right now?", kept out of the view.
export function canReset(model) {
  if (model.activeProfiles.size > 0) return true;
  for (const i of model.pkgs.keys()) if (isDeviated(model, i)) return true;
  return false;
}
// Which profiles list this package? Returns [{name, emoji}] — used to show, next
// to a package, the emojis of the profiles it belongs to. Order follows the
// profile declaration order (Map preserves insertion).
export function profilesForPkg(model, i) {
  const p = model.pkgs.get(i);
  if (!p) return [];
  const out = [];
  for (const prof of model.profiles.values()) {
    if (prof.packages.includes(p.name)) {
      out.push({ name: prof.name, emoji: prof.emoji });
    }
  }
  return out;
}
// off | full | hollow — derived live from the toggles (never stored). full when
// every package of an active profile is effectively "in"; hollow when active but
// the user pulled one out; off when not active. Delegates to decision.profileState.
export function profileStateOf(model, name) {
  const prof = model.profiles.get(name);
  if (!prof) return "off";
  const byName = new Map();
  for (const [i, p] of model.pkgs) byName.set(p.name, i);
  return profileState(prof, model.activeProfiles.has(name), (pkgName) => {
    const i = byName.get(pkgName);
    return i != null && toggleOf(model, i) === "in";
  });
}

// Present-count for a profile — MACHINE TRUTH, live-derived (not intent). Of the
// profile's packages, how many are present on the machine right now. Distinct
// from profileStateOf (off/full/hollow = intention). A profile can be "full"
// (intended) yet 0/N present (nothing installed yet) — both are correct.
export function profileProgress(model, name) {
  const prof = model.profiles.get(name);
  if (!prof) return { present: 0, total: 0 };
  const byName = new Map();
  for (const p of model.pkgs.values()) byName.set(p.name, p);
  let present = 0;
  for (const pkgName of prof.packages) {
    if (byName.get(pkgName)?.present === true) present++;
  }
  return { present, total: prof.packages.length };
}

// --- persistence (by stable key, survives reordering) -----------------------
export function persistablePkgs(model) {
  const pkgs = {};
  for (const [i, state] of model.decision) {
    const p = model.pkgs.get(i);
    if ((state === "in" || state === "out") && p) pkgs[p.key] = state;
  }
  return pkgs;
}
export function applySavedSelection(model, sel) {
  if (!sel) return;
  const byKey = new Map();
  for (const [i, p] of model.pkgs) byKey.set(p.key, i);
  for (const [key, state] of Object.entries(sel.pkgs || {})) {
    const i = byKey.get(key);
    if (i != null && (state === "in" || state === "out")) {
      setDecision(model, i, state);
    }
  }
}
