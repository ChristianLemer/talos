// model.js — the Talos panel STATE, with ZERO DOM. The single source of truth
// the view (app.js) projects from. Pure and importable, so it is unit-tested by
// tested under node --test just like decision.js. Rules live in decision.js; this module holds
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
import { RUNGS, rungAllows } from "./ladder.js";
import { scopeOf as scopeRule, scopeReason as scopeReasonRule } from "./scope.js";
import { compareVersions } from "./version.js";

// The user's own editable bundle — always active, promotes what the user ADDED
// from the Catalog. "Wanting" lives here (force is retired, spec §17): a package
// is wanted if an active bundle (author's or this one) promotes it and it isn't
// vetoed. Its members persist locally (selection.personal).
export const PERSONAL_BUNDLE = "My setup";

export function createModel() {
  return {
    pkgs: new Map(), // i -> PkgRecord
    decision: new Map(), // i -> "in" | "out"  (absence = auto)
    scope: new Map(), // i -> "in" | "out"  (absence = follow the derivation)
    // Rows the user acted on THIS SESSION. Never persisted (interface history, not
    // intent) — it exists so a gesture leaves a trace even when it changes nothing.
    touched: new Set(),
    profiles: new Map(), // name -> ProfileRecord {name, emoji, description, packages}
    activeProfiles: new Set(), // names of profiles the user has applied
    detectedAt: null, // ms of the last full presence scan (future TTL home)
  };
}

// Build the model from the server `plan` message (flat steps + profiles).
export function loadPlan(model, steps, profiles = []) {
  model.pkgs.clear();
  model.profiles.clear();
  model.activeProfiles.clear();
  model.scope.clear();
  // A new plan is a new session's worth of rows: the traces of the old one are moot.
  model.touched.clear();
  for (const p of profiles) {
    model.profiles.set(p.name, {
      name: p.name,
      emoji: p.emoji || "🎯",
      description: p.description || "",
      usage: p.usage || p.description || "",
      highlights: p.highlights ?? [],
      packages: p.packages ?? [],
      needs: p.needs ?? [], // other bundles this one depends on (cascade, §23)
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
      // Derived on the SERVER from the route (`bundles::is_extension_route`), never declared
      // in YAML — carried here so the ladder's live counts match what the server will filter.
      isExtension: !!s.isExtension,
      // Installed outside our manager (Xcode-CLT Git, say). Arrives with the
      // per-row `state` message, NOT with the plan — so it starts false and the
      // scan flips it. A switch that pre-guessed would be the bug.
      external: false,
      requires: s.requires ?? [], // package names this one needs (transitive pull, §8)
      // The ladder's three discriminants, resolved SERVER-side (fleet observation with the
      // catalogue's declaration laid over it) and sent with the plan. The front does not
      // compute them and does not hold the durations behind them — it is told the verdict.
      uac: !!s.uac,
      forbidden: !!s.forbidden,
      slow: !!s.slow,
      // How long this took HERE last time, in whole seconds. 0 = never measured, which the
      // estimate must COUNT and SHOW as unknown rather than treat as free (server.rs floors
      // a measurement at 1s precisely so this sentinel stays unambiguous).
      secs: Number(s.secs) || 0,
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
  }
  // Seed the always-on personal bundle (spec §14-17). Its members are restored
  // separately from the local store (applySavedPersonal), not from the server.
  if (!model.profiles.has(PERSONAL_BUNDLE)) {
    model.profiles.set(PERSONAL_BUNDLE, {
      name: PERSONAL_BUNDLE,
      emoji: "⭐",
      usage: "The packages you picked yourself.",
      highlights: [],
      description: "Your own selection.",
      packages: [],
    });
  }
  model.activeProfiles.add(PERSONAL_BUNDLE);
}

// --- personal bundle (the user's own editable, always-active bundle) ---------
export function addToPersonal(model, pkgName) {
  const p = model.profiles.get(PERSONAL_BUNDLE);
  if (p && !p.packages.includes(pkgName)) p.packages.push(pkgName);
}
export function removeFromPersonal(model, pkgName) {
  const p = model.profiles.get(PERSONAL_BUNDLE);
  if (p) p.packages = p.packages.filter((n) => n !== pkgName);
}
export function isInPersonal(model, pkgName) {
  return !!model.profiles.get(PERSONAL_BUNDLE)?.packages.includes(pkgName);
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
  return effectiveToggle(manualToggle(model, i), isPulled(model, i));
}
export function isLocked(model, i) {
  return isLockedPosture(postureOf(model, i));
}

// --- scope: the SECOND axis — "is this mine to manage?" ---------------------
//
// Rule lives in scope.js; this composes it with the model's facts. Two layers:
// the DERIVATION (no way back, or installed elsewhere) and the user's manual
// override, which is sparse — absence means "follow the derivation".

// The raw manual override, or null. Same defensive shape as manualToggle.
export function manualScope(model, i) {
  const s = model.scope.get(i);
  return s === "in" || s === "out" ? s : null;
}
// The facts scope.js needs, gathered in one place so the rule stays pure.
function scopeFacts(model, i) {
  const p = model.pkgs.get(i);
  return {
    canUninstall: !!p?.canUninstall,
    external: p?.external === true,
    isConfig: !!p?.isConfig,
    isExtension: !!p?.isExtension,
    pulled: isPulled(model, i),
    manual: manualScope(model, i),
  };
}
// "in" | "out" — the effective scope of row i.
export function scopeOf(model, i) {
  return scopeRule(scopeFacts(model, i));
}
// WHY it is out of scope, for the row's right-hand label; null when in scope.
export function scopeReason(model, i) {
  return scopeReasonRule(scopeFacts(model, i));
}
// Set (or clear, with null) the user's override. Unlike setDecision there is no
// posture to refuse it: scope is the user's question, always theirs to answer.
export function setScope(model, i, state, { trace = true } = {}) {
  if (state === "in" || state === "out") model.scope.set(i, state);
  else model.scope.delete(i);
  // A scope gesture is a gesture: it leaves its trace too — unless it is a RESTORE. Same
  // distinction as `setDecision`: `touched` is interface history, `scope` is intent, and replaying
  // intent is not doing something.
  if (trace) markTouched(model, i);
}
// Would pulling row i INTO scope be the risky direction? True only for a row we
// did not install: managing it means Talos may remove a binary it never put
// there. This is the one gesture that asks for confirmation (app.js).
export function scopeInNeedsConfirm(model, i) {
  return model.pkgs.get(i)?.external === true;
}
// Every out-of-scope index, in catalogue order — exactly what the `apply` message
// must carry so the SERVER can refuse them too (a front-only guard is cosmetic).
export function unmanagedIndices(model) {
  const out = [];
  for (const i of [...model.pkgs.keys()].sort((a, b) => a - b)) {
    if (scopeOf(model, i) === "out") out.push(i);
  }
  return out;
}
// A package REFUSES to be wanted if the author forbade it or the user vetoed it by
// hand. Such a package never installs, so it also can't propagate its `requires`.
function refuses(model, i) {
  return isLocked(model, i) || manualToggle(model, i) === "out";
}
// The set of package NAMES that end up wanted "in" — the fixpoint of: SEED with
// what a manual "in" or an active bundle pulls, then CLOSE over `requires` (a
// wanted package pulls its requirements "in" too, transitively). A manual "out"
// or `forbidden` STOPS propagation (§8: a package that won't install can't pull
// its deps). Cycle-safe: the set only grows, so the walk terminates. This is the
// live transitive pull, computed in the model (deps.rs only ORDERS the install).
export function wantedNames(model) {
  const nameToId = new Map();
  for (const [i, p] of model.pkgs) nameToId.set(p.name, i);
  const wanted = new Set();
  for (const [i, p] of model.pkgs) {
    if (refuses(model, i)) continue;
    if (manualToggle(model, i) === "in" || inActiveProfile(model, i)) wanted.add(p.name);
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const name of [...wanted]) {
      const p = model.pkgs.get(nameToId.get(name));
      for (const req of p?.requires ?? []) {
        const j = nameToId.get(req);
        if (j == null || refuses(model, j) || wanted.has(req)) continue;
        wanted.add(req);
        changed = true;
      }
    }
  }
  return wanted;
}
// Is this package pulled "in" — by an active bundle OR by a wanted package that
// requires it (transitively)? Feeds the `pulled` argument of the decision rules.
// (When the user forced it "in" by hand, the manual toggle wins upstream anyway,
// so reporting it as pulled too is harmless — toggleState checks manual first.)
export function isPulled(model, i) {
  const name = model.pkgs.get(i)?.name;
  return name != null && wantedNames(model).has(name);
}
// Bundle-driven: the decision fns take (posture, manualToggle, pulled) — pulled =
// "an active bundle OR a wanted dependant wants this package". A manual toggle
// wins; else the pull decides; else out. See isPulled + wantedNames (§8).
export function toggleOf(model, i) {
  return toggleState(postureOf(model, i), manualToggle(model, i), isPulled(model, i));
}
export function desiredOf(model, i) {
  return desiredState(postureOf(model, i), manualToggle(model, i), isPulled(model, i));
}
export function isDeviated(model, i) {
  return isDeviation(postureOf(model, i), manualToggle(model, i));
}
// The action a plain Apply would take here, or null. present:null → treated as
// "not known present" (false), the safe direction for install.
//
// SCOPE IS A GATE IN FRONT OF THE RULE (spec 2026-07-30 §1). An out-of-scope row
// yields no action, whatever the desire and whatever the machine — one line here,
// and every downstream consumer goes quiet for free: actClass, isActionable,
// .will-change, .will-remove, the plan count. Note this gates ACTION only; the row
// is still probed and still reports presence, version and outdated.
export function actionOf(model, i) {
  const p = model.pkgs.get(i);
  if (!p) return null;
  if (scopeOf(model, i) === "out") return null;
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
  const none = { from: "", to: null, pinned: null, muted: false, held: false };
  const p = model.pkgs.get(i);
  if (!p) return none;
  const inst = p.present === true ? (p.installedVersion || "") : "";
  if (!inst) return none;
  if (p.pin) {
    const held = String(p.pin).toLowerCase() === "pending";
    const chasing = String(p.pin).toLowerCase() === "latest";
    // ⭐ `pending` and `latest` are POLICY, not versions — comparing them would read them as
    // 0.0.0 (measured) and claim the machine is "above the pin".
    if (held) {
      // Show the gap when there is one and it is FORWARD, GREYED: seen, quantified, not
      // pushed. That is what makes this a hold rather than a blindfold — and it is what
      // the arbiter reads. BACKWARDS (available < installed) is drift from receipt lag;
      // calling it arbitration would assert a decision exists when the machine is ahead.
      if (p.outdated && p.available && compareVersions(p.available, inst) > 0) {
        return { from: inst, to: p.available, pinned: null, muted: true, held: true };
      }
      return { from: inst, to: null, pinned: null, muted: false, held: false };
    }
    if (chasing) {
      // Same display as declaring nothing: an ordinary push when something is newer.
      if (p.outdated && p.available) {
        return { from: inst, to: p.available, pinned: null, muted: false, held: false };
      }
      return { from: inst, to: null, pinned: null, muted: false, held: false };
    }
    const cmp = compareVersions(inst, p.pin);
    if (cmp !== 0) {
      // Off the pin → migration TO the pin. BELOW (cmp<0) is an upgrade Apply
      // runs → not muted (a push). ABOVE (cmp>0) is a downgrade — manual only,
      // never batched by Apply → muted, same "not pushed" grey as an upgrade
      // past a pin. Mirrors AUTO_ACTS excluding downgrade. See talos-version-pin.
      return { from: inst, to: p.pin, pinned: "to", muted: cmp > 0, held: false };
    }
    // At the pin: normally just the pinned number. But if the machine-wide scan
    // found something newer, SHOW it greyed (muted) — visible to test, not pushed.
    if (p.outdated && p.available && compareVersions(p.available, p.pin) > 0) {
      return { from: inst, to: p.available, pinned: "from", muted: true, held: false };
    }
    return { from: inst, to: null, pinned: "from", muted: false, held: false };
  }
  // Unpinned: show the machine-wide available only when actually outdated.
  if (p.outdated && p.available) {
    return { from: inst, to: p.available, pinned: null, muted: false, held: false };
  }
  return { from: inst, to: null, pinned: null, muted: false, held: false };
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
// Is this row's version HELD, with something to decide?
//
// ⚠️ The count this feeds is "rows with something to arbitrate", NOT "packages carrying the
// keyword". 25 packages may declare `pending` while only a handful have a newer version
// available — a tab reading `Arbitration (25)` would send someone to look at nineteen rows
// with nothing on them.
//
// Absent is excluded deliberately: there is no held version of a thing that is not there, and
// `action_for` returns `install` for that row (it is ordinary work, not a decision).
export function isPending(model, i) {
  const p = model.pkgs.get(i);
  if (!p || p.present !== true) return false;
  if (String(p.pin || "").toLowerCase() !== "pending") return false;
  return !!(p.outdated && p.available);
}
// Which rows a given rung would touch, in catalogue order. The FRONT needs this only to
// show the choice; the SERVER is what filters (see ladder.js's header).
//
// Returns indices rather than a number because Task 6's estimate needs the rows
// themselves (each carries its own `secs`), and one traversal should answer both.
//
// ⚠️ The AUTO_ACTS gate is REDUNDANT today and kept as a COUPLING guard, not as a live
// filter — measured, not assumed: deleting the line kills nothing, because `rungAllows`
// independently refuses `downgrade` and a null action, which are the only two things
// AUTO_ACTS excludes. Its job is to keep the two lists tied: if a verb is ever added to
// `rungAllows` without being added to AUTO_ACTS, the count would promise a batch action
// Apply does not take, and this line is what stops it. Stated plainly because an
// "obviously necessary" filter that in fact filters nothing is the kind of line a later
// reader trusts too much.
export function rungPlan(model, rung) {
  const items = [];
  for (const i of [...model.pkgs.keys()].sort((a, b) => a - b)) {
    const a = actionOf(model, i);
    if (!AUTO_ACTS.includes(a)) continue;
    const p = model.pkgs.get(i);
    if (!rungAllows(rung, a, p.isConfig, p.isExtension === true)) continue;
    items.push(i);
  }
  return items;
}
// The local durations, in seconds, for the rows a rung would touch — ladder.js's `estimate`
// turns them into "~4 min for 6 and 3 unknown". `0` = never measured HERE, which the estimate
// COUNTS and SHOWS as unknown rather than treating as free.
//
// ⚠️ `p.secs`, the LOCAL last-seen duration, and NOT the shared classification. `p.slow` is
// the other file's verdict — a max over the whole fleet, which exists so that a rung means
// the same thing on every machine — and it is a BOOLEAN here, so deriving minutes from it
// could only mean inventing a constant. A row can legitimately be `slow: true, secs: 1`
// (measured: a brew bottle already cached); the rung puts it last for everyone and the
// estimate still says "<1 min", because those are two different questions.
//
// Built on rungPlan rather than walking the packages again, so the count under the thumb and
// the minutes beside it can never describe different sets of rows.
export function rungSeconds(model, rung) {
  return rungPlan(model, rung).map((i) => model.pkgs.get(i).secs || 0);
}
// WHY a row is out of this rung's reach, or null when it is in. A KEY, not a sentence:
// this module is the rule layer and holds no copy — the words live in app.js beside the
// other user-facing strings (`RUNG_WHY`), so they can be read and reworded in one place.
// Returns "slow" | "hand" | "blocked" | "update" | "install" | null.
//
// ⚠️ DERIVED from `rungAllows`, never a second rule. Every branch below asks the rule and
// only then explains, so the two cannot disagree — a copied table would eventually put a
// reason on a row that IS in the plan, which is the one failure mode that would make the
// dimming a lie rather than a hint.
//
// The order mirrors rung_allows's: the ACTION excludes before any fact does. A quiet
// upgrade at 📦 is out for being an update, not for any behaviour — and so is a SLOW one,
// which is the case that makes the order load-bearing: labelling it "slow" at 📦 would be
// a lie the user could check by stepping one rung right and seeing it stay out.
export function rungReason(rung, action, p) {
  const isConfig = p?.isConfig === true;
  const isExt = p?.isExtension === true;
  const top = RUNGS.length - 1; // read from RUNGS so another rung cannot leave this behind
  // Ask the rule FIRST. In reach → there is nothing to explain.
  if (rungAllows(rung, action, isConfig, isExt)) return null;
  // Out at EVERY rung — a null action (out of scope, nothing to do) or a `downgrade`, which
  // Apply never batches. The RUNG is not what excludes these, so they get no rung reason and
  // the panel leaves them alone: dimming them would blame the slider for a row it does not
  // govern, and on this machine that is most of the list.
  if (!rungAllows(top, action, isConfig, isExt)) return null;
  // ⭐ WHAT IS LEFT is the KIND, and only the kind. Three reasons became two, and the two that
  // went ("slow", "hand", "blocked") are the point of the reshape: a fact can no longer hold a
  // row out of a rung, so it can no longer be the reason one is dimmed. Those words did not
  // disappear — they moved from "why this row is out of reach" to "what this row will do to
  // you", shown on every row at every rung.
  //
  // ⚠️ And the walk that derived the fact is gone with them. It existed because `slow`
  // dominated `uac`/`403` and the dominance had to be ASKED rather than assumed; with the
  // facts out of the rule there is nothing left to ask.
  // A config-atom is admitted at rung 0, so it can never be out of reach and never reaches
  // here. What remains is: an EXTENSION held out at rung 0 ("move to 🧩"), or an app held out
  // at 0 or 1 ("move to 📦"). The key names WHERE it enters, which is the only actionable
  // thing left to say.
  return isExt ? "extension" : "app";
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

// --- touched: a gesture must leave a trace ----------------------------------
//
// A row the user acted on stays VISIBLE for the rest of the session, even when the
// action turns out to be a no-op. C's report: "quand un utilisateur agit sur une
// entrée et qu'elle disparaît sous ses yeux c'est bizarre et ennuyeux… surtout si
// c'est une erreur". Exactly so — the Changes tab shows only `.will-change` rows, so
// UNDOING a mistake made the row vanish, which means correcting an error destroyed
// the evidence that you had corrected it. There was no way to check your own work.
//
// SESSION-ONLY, never persisted. This is interface history, not intent: writing it to
// selection.json would resurrect rows on the next launch with no visible cause, and
// `pkgs`/`scope` are sparse precisely so that only real intent is stored.
//
// The codebase already had this intuition for ONE case (a bundle member you turned
// out stays visible, struck through, so the bundle reads incomplete — index.html:523).
// This generalises it: that was not a special case, it was the rule.
export function markTouched(model, i) {
  model.touched.add(i);
}
export function isTouched(model, i) {
  return model.touched.has(i);
}

// --- mutations --------------------------------------------------------------
export function setDecision(model, i, state, { trace = true } = {}) {
  if (isLocked(model, i)) return false; // author's posture wins
  if (state === "in" || state === "out") model.decision.set(i, state);
  else model.decision.delete(i); // anything else clears to auto
  // Returning to auto IS a gesture — it must leave its trace.
  //
  // ⚠️ EXCEPT when RESTORING a persisted decision, which is why `trace` exists. C, on the running
  // app: "j'ai fait refresh et elle ne part pas". `applySavedSelection` replays every stored
  // choice through here, so a Refresh cleared `touched` (loadPlan does) and then immediately
  // re-marked every row that had ever been decided — the trace looked permanent and the Changes
  // tab kept showing rows where nothing would happen.
  //
  // The distinction is the point: `touched` is INTERFACE HISTORY ("you did this, just now"), while
  // `decision` is INTENT ("you want this, still"). Replaying intent is not doing something.
  if (trace) markTouched(model, i);
  return true;
}
// The scan's verdict on provenance: present, but installed outside our manager.
// Set by the `state` handler, per row, as each probe lands.
export function setExternal(model, i, external) {
  const p = model.pkgs.get(i);
  if (p) p.external = external === true;
}
// Reset: back to posture defaults — clears BOTH manual toggles AND active
// profiles (profiles are additive intention; a true reset drops them too).
export function clearAllDecisions(model) {
  model.decision.clear();
  model.activeProfiles.clear();
  // Reset clears the TRACES too. Everywhere else a gesture must leave one, but Reset
  // is the gesture that says "forget my choices" — keeping witnesses to choices that
  // no longer exist would fill the view with rows the user just asked to be rid of.
  model.touched.clear();
  // The personal bundle is always-on and emptied by a reset (not removed): clear
  // its members, then re-activate it so "My setup" stays present but blank.
  const personal = model.profiles.get(PERSONAL_BUNDLE);
  if (personal) {
    personal.packages = [];
    model.activeProfiles.add(PERSONAL_BUNDLE);
  }
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

// --- profiles ---------------------------------------------------------------
// Apply a profile: mark it active (it then pulls its packages "in" via
// userToggle). Additive — it never writes decisions and never forces anything
// out, so a manual "out" still wins (that turns the profile hollow).
export function applyProfile(model, name) {
  // Activate this bundle AND, transitively, every bundle it `needs:` — for real,
  // "as if clicked" (spec §23). Cycle-guarded via the seen set. Deactivating does
  // NOT reverse-cascade (removeProfile drops only the one bundle) — the dependency
  // pulls but never pushes back, so order never matters.
  const seen = new Set();
  const activate = (n) => {
    if (seen.has(n) || !model.profiles.has(n)) return;
    seen.add(n);
    model.activeProfiles.add(n);
    for (const dep of model.profiles.get(n).needs ?? []) activate(dep);
  };
  activate(name);
}
// Remove a profile from the active set. REVERSE-CASCADE: any active bundle that
// (transitively) `needs:` this one goes too — you can't leave a dependent alive
// without its socle (the `needs` invariant: an active dependency is always
// satisfied). A package shared with another still-active bundle or a manual "in"
// survives (userToggle recomputes from what remains). Cycle-guarded via `seen`.
export function removeProfile(model, name) {
  const seen = new Set();
  const deactivate = (n) => {
    if (seen.has(n) || !model.activeProfiles.has(n)) return;
    seen.add(n);
    model.activeProfiles.delete(n);
    // Drop every active bundle that needs `n` (directly or through the chain).
    for (const other of [...model.activeProfiles]) {
      if ((model.profiles.get(other)?.needs ?? []).includes(n)) deactivate(other);
    }
  };
  deactivate(name);
}
export function isProfileActive(model, name) {
  return model.activeProfiles.has(name);
}
// Would a Reset do anything? True if the user moved a package off its author
// default OR has an active profile — both are cleared by clearAllDecisions. The
// domain answer to "is Reset meaningful right now?", kept out of the view.
export function canReset(model) {
  // Any ACTIVE author bundle (not the always-on personal one) is a change.
  for (const name of model.activeProfiles) {
    if (name !== PERSONAL_BUNDLE) return true;
  }
  // A non-empty personal bundle (the user added packages) is a change.
  if ((model.profiles.get(PERSONAL_BUNDLE)?.packages.length ?? 0) > 0) return true;
  // A manual override (veto) on any package is a change.
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

// Progress-count for a bundle card — "installed / wanted", i.e. how far the
// machine has come toward what this bundle WILL put down. `total` = the bundle's
// packages that are desired-present (the future: what Apply will install/keep),
// `present` = those already installed now. So present ≤ total, and it reads as
// progress toward the goal, tracking BOTH the scan (present rises) and toggles
// (total shifts). An inactive bundle wants nothing → 0/0 (the view blanks it).
export function profileProgress(model, name) {
  const prof = model.profiles.get(name);
  if (!prof) return { present: 0, total: 0 };
  const idByName = new Map();
  for (const [i, p] of model.pkgs) idByName.set(p.name, i);
  let present = 0, total = 0;
  for (const pkgName of prof.packages) {
    const i = idByName.get(pkgName);
    if (i == null) continue;
    if (desiredOf(model, i) !== "present") continue; // not wanted → not in the goal
    total++;
    if (model.pkgs.get(i)?.present === true) present++;
  }
  return { present, total };
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
      // ⚠️ `trace: false` — restoring a stored choice is NOT a gesture of this session. Without it
      // a Refresh re-marked every previously decided row and the trace looked permanent.
      setDecision(model, i, state, { trace: false });
    }
  }
}
// Scope overrides — sparse, keyed by NAME like pkgs, so only rows the user
// actually moved are written and the file survives catalogue reordering.
export function persistableScope(model) {
  const scope = {};
  for (const [i, state] of model.scope) {
    const p = model.pkgs.get(i);
    if ((state === "in" || state === "out") && p) scope[p.key] = state;
  }
  return scope;
}
export function applySavedScope(model, sel) {
  if (!sel) return;
  const byKey = new Map();
  for (const [i, p] of model.pkgs) byKey.set(p.key, i);
  for (const [key, state] of Object.entries(sel.scope || {})) {
    const i = byKey.get(key);
    // `trace: false` — see `applySavedSelection`: a restore is not a gesture of this session.
    if (i != null && (state === "in" || state === "out")) setScope(model, i, state, { trace: false });
  }
}
// Personal bundle members — the package NAMES the user added from the Catalog.
export function persistablePersonal(model) {
  return [...(model.profiles.get(PERSONAL_BUNDLE)?.packages ?? [])];
}
export function applySavedPersonal(model, list) {
  if (!Array.isArray(list)) return;
  const p = model.profiles.get(PERSONAL_BUNDLE);
  if (p) p.packages = [...list];
}
// Active bundles — which top cards the user activated (excludes the always-on
// personal bundle, re-added by loadPlan). Restored so the cascade + card state
// survive a restart. Only names that still exist as profiles are re-activated.
export function persistableActiveBundles(model) {
  return [...model.activeProfiles].filter((n) => n !== PERSONAL_BUNDLE);
}
export function applySavedActiveBundles(model, list) {
  if (!Array.isArray(list)) return;
  for (const name of list) if (model.profiles.has(name)) applyProfile(model, name);
}
