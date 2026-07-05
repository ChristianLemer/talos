// model.js — the Talos panel STATE, with ZERO DOM. The single source of truth
// the view (app.js) projects from. Pure and importable, so it is unit-tested by
// deno test just like decision.js. Rules live in decision.js; this module holds
// the state and applies those rules to it.
import {
  actionFor,
  desiredState,
  isDeviation,
  isLockedPosture,
  toggleState,
} from "./decision.js";

export function createModel() {
  return {
    pkgs: new Map(), // i -> PkgRecord
    bundles: new Map(), // name -> BundleRecord
    decision: new Map(), // i -> "in" | "out"  (absence = auto)
    detectedAt: null, // ms of the last full presence scan (future TTL home)
  };
}

// Build the model from the server `plan` message (bundles + flat steps).
export function loadPlan(model, bundles, steps) {
  model.pkgs.clear();
  model.bundles.clear();
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
      present: null,
      outdated: false,
      status: "waiting",
      log: "",
      key: `${s.bundle}::${s.name}`,
    });
    const be = model.bundles.get(s.bundle);
    if (be) be.pkgIds.push(s.i);
  }
}

// --- per-package queries (delegate rules to decision.js) --------------------
export function postureOf(model, i) {
  return model.pkgs.get(i)?.posture ?? "mandatory";
}
export function userToggle(model, i) {
  const d = model.decision.get(i);
  return d === "in" || d === "out" ? d : null;
}
export function isLocked(model, i) {
  return isLockedPosture(postureOf(model, i));
}
export function toggleOf(model, i) {
  return toggleState(postureOf(model, i), userToggle(model, i));
}
export function desiredOf(model, i) {
  return desiredState(postureOf(model, i), userToggle(model, i));
}
export function isDeviated(model, i) {
  return isDeviation(postureOf(model, i), userToggle(model, i));
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
  });
}
export function isActionable(model, i) {
  return actionOf(model, i) != null;
}
// The manual invert action a row button performs (label + direction + msg type).
// ALWAYS inverts current machine state: absent→install, present→uninstall,
// present-but-stale→update. null only when present and un-uninstallable.
export function buttonAction(model, i) {
  const p = model.pkgs.get(i);
  if (!p) return null;
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
export function clearAllDecisions(model) {
  model.decision.clear();
}
export function setPresence(model, i, present) {
  const p = model.pkgs.get(i);
  if (!p) return;
  p.present = present;
  if (present !== true) p.outdated = false;
}
// Settle a package's status; ok/absent/waiting also update presence.
export function setStatusData(model, i, status) {
  const p = model.pkgs.get(i);
  if (!p) return;
  p.status = status;
  if (status === "ok") {
    p.present = true;
    p.outdated = false;
  } else if (status === "absent" || status === "waiting") {
    p.present = false;
    p.outdated = false;
  }
}
export function setOutdated(model, i, on) {
  const p = model.pkgs.get(i);
  if (p) p.outdated = !!on;
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
