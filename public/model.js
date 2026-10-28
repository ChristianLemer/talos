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
import { scopeOf as scopeRule, scopeReason as scopeReasonRule } from "./scope.js";
import { bareVersion, compareToPin, compareVersions } from "./version.js";

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
      // On at a first launch — `default: on`, or needed by a card that is. The ENGINE
      // computes it (profiles.rs default_active); the model only returns to it on Reset.
      defaultOn: p.defaultOn === true,
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
      // Can the Apply take this row DOWN to its pin? Sent by the server (`step_json`): only a
      // route that builds a downgrade (an uninstall + a pinned reinstall) can. False when absent.
      canDowngrade: !!s.canDowngrade,
      // `version: self` with its own `update:` command: the Apply runs that command when the
      // tool is behind what the manager announces (`step_json`). False for every other row.
      selfUpdate: !!s.selfUpdate,
      // `update-on: apply`: that update runs at each Apply while the row is wanted, until it succeeds.
      updateEachApply: !!s.updateEachApply,
      // Done for this Talos session — its update exited ok, or the Apply installed it (the
      // server's `updatedThisSession` on the plan, then its `self-updated`), until a Refresh.
      updatedThisSession: !!s.updatedThisSession,
      // The last version change that exited ok and moved nothing — { target, reason } from the
      // server's `unmoved` message — or null. Held while that target is in view.
      unmoved: null,
      isConfig: !!s.isConfig,
      // Derived on the SERVER from the route (`bundles::is_extension_route`), never declared
      // in YAML — carried here for the 🧩 glyph on the row.
      isExtension: !!s.isExtension,
      // Installed outside our manager (Xcode-CLT Git, say). Arrives with the
      // per-row `state` message, NOT with the plan — so it starts false and the
      // scan flips it. A switch that pre-guessed would be the bug.
      external: false,
      requires: s.requires ?? [], // package names this one needs (transitive pull, §8)
      replaces: s.replaces ?? [], // package names this one stands in for (`replacedBy`)
      // The three behaviour facts the row labels show, resolved SERVER-side (fleet observation with the
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
      // What the manager itself records as installed, from the same scan — "" when it is not
      // outdated. Behind what runs when the tool updated itself, or another copy comes first.
      current: "",
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
// WHY a row's `requires:` does not hold, or null — the front's half of
// `deps::requires_blocked`, word for word so the row reads the same whether the
// verdict came from the scan (here) or from the Apply (server.rs).
//
// A requirement holds iff the package EXISTS in the catalogue and is either present
// now or on its way in. Note what is deliberately NOT a veto: a requirement nobody
// wants. `will_be_present` (deps.rs) does veto on wanted-off, which is right for the
// Apply — there `off` is the uninstall instruction — but transposed to a derivation
// at rest it would make every unrequested plugin row shout `requires Nushell` beside
// an installed nushell. See the test that pins this.
//
// First failing requirement wins the message, like the engine. DIRECT requirements
// only, also like the engine: a chain whose middle link is itself blocked reports
// nothing here. That gap is real and shared by both sides — one rule, one hole,
// never two behaviours.
export function requiresReason(model, i) {
  const p = model.pkgs.get(i);
  if (!p) return null;
  const idByName = new Map();
  for (const [j, q] of model.pkgs) idByName.set(q.name, j);
  for (const req of p.requires ?? []) {
    const j = idByName.get(req);
    // A dangling name is NAMED, never silently satisfied: the catalogue is a
    // directory of files and a `requires:` can outlive the package it points at
    // (catalog.rs:164 chose not to fail loudly, so the row has to say it).
    if (j == null) return `requires ${req} (unknown)`;
    const q = model.pkgs.get(j);
    // Presence FIRST and unconditionally — the day `cmd /c ver` answers, the row
    // behaves like any other. Without this ordering the fix would break Windows.
    if (q?.present === true) continue;
    // Otherwise: is it on its way in? Wanting it is NOT enough. `wantedNames` closes
    // over `requires:`, so wanting a row pulls its own requirements in — a desire-only
    // clause would make every pulled row satisfy itself and this whole rule vacuous.
    // What counts is REACHABILITY: a package with no practicable route is never laid
    // down, whatever anyone wants. `canUninstall` is the existing proxy for "Talos
    // acts on this row" (scope.js's `no-route`), used here for that meaning.
    if (desiredOf(model, j) === "present" && q?.canUninstall) continue;
    return `requires ${req}`;
  }
  return null;
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
    // The GLOBAL fact, pre-computed: scope.js is a pure function of one row, and this
    // verdict reads another package's state. The walk belongs to whoever holds the model.
    requiresReason: requiresReason(model, i),
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
// --- fit: what THIS machine can get out of the content ------------------------
// The advanced-mode summary line above the list (app.js paintFit). It counts the WHOLE
// content, not the active bundles: the question it answers is "does this kit fit this
// machine?", asked by whoever composes or checks a kit, and that question is about every
// row the kit ships. It is also the set the Catalog tab lists under it, so the line can
// be checked against the rows by eye there (the Bundles tab filters its list; the line
// says "of 19" either way).
//
// It is not independent of the choices: a requirement holds when it is present OR wanted
// and reachable (requiresReason), so wanting Visual Studio Code takes its extension off
// `requires Visual Studio Code` — the row moves the same way, and the line with it.
//
// NO new categories: a row is out of reach for exactly the reasons the rows already
// show, in scopeReason's order — a requirement that does not hold (the verbatim
// `requires X`), else no practicable route (`no-route`, the row's `—`).
export function fitReason(model, i) {
  const p = model.pkgs.get(i);
  if (!p) return null;
  return requiresReason(model, i) ??
    (p.present !== true && !p.canUninstall && !p.isConfig ? "no-route" : null);
}
// Three departures from scopeReason, all because fit is a fact of the MACHINE, not of the
// user's scope:
//   - a manual "out" or an `external` provenance does not take a row out of reach;
//   - a manual "in" does not bring a blocked row INTO reach: its row label goes back to
//     the presence word, but the Apply still drops it with the same `requires X`
//     (server.rs, the requirement gate runs whatever the scope), so the line keeps it;
//     likewise a row with no route forced in drops its `—` but still has no route;
//   - a PRESENT row with no route is within reach — the OS package (detect-only) is
//     simply there on its own OS. A present row whose requirement fails still counts
//     under that requirement, as its row shows.
export function fitSummary(model) {
  const groups = new Map(); // reason → count, insertion order = catalogue order
  let reachable = 0;
  for (const i of [...model.pkgs.keys()].sort((a, b) => a - b)) {
    const why = fitReason(model, i);
    if (why == null) reachable++;
    else groups.set(why, (groups.get(why) ?? 0) + 1);
  }
  // Largest requirement first (ties keep catalogue order: sort is stable); `no-route`
  // last, because it is the only reason that names nothing a machine could acquire.
  const list = [...groups].map(([reason, count]) => ({ reason, count }));
  list.sort((a, b) => (a.reason === "no-route") - (b.reason === "no-route") || b.count - a.count);
  return { total: model.pkgs.size, reachable, groups: list };
}
// The line itself: `12 of 19 within reach here · 3 require Windows · 1 has no route here`.
// A requirement keeps the row's words, only conjugated (`requires X` → `require X` for
// more than one row). No advice: the reasons say what is missing, the line only counts.
// Empty content → "" (the caller hides the line).
export function fitLine({ total, reachable, groups }) {
  if (!total) return "";
  const parts = [`${reachable} of ${total} within reach here`];
  for (const { reason, count } of groups) {
    const words = reason === "no-route"
      ? (count === 1 ? "has no route here" : "have no route here")
      : count === 1 ? reason : reason.replace(/^requires /, "require ");
    parts.push(`${count} ${words}`);
  }
  return parts.join(" · ");
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
  return pull(model).wanted;
}
// The pull with the OVERRIDE (`replaces:`) applied: `wanted` (names), and `replaced` (replaced
// name → the name of what replaces it). What a wanted package stands in for is not wanted,
// whatever its own intention says. Derived, never stored: the replaced row keeps its intention
// and gets it back the moment its replacement is not wanted. One level only (`--check` refuses
// a replacer that is itself replaced), so the replacers are read from the pull WITHOUT the
// override, and the pull is then taken again without what they replace — a replaced package
// pulls none of its requirements.
function pull(model) {
  // The common case pays nothing: a catalogue with no `replaces:` takes the pull once.
  if (![...model.pkgs.values()].some((p) => p.replaces?.length)) {
    return { wanted: pulledNames(model, new Set()), replaced: new Map() };
  }
  const nameToId = new Map();
  for (const [i, p] of model.pkgs) nameToId.set(p.name, i);
  const first = pulledNames(model, new Set());
  const replaced = new Map();
  for (const [j, p] of model.pkgs) {
    if (!p.replaces?.length || !first.has(p.name) || !landsHere(model, j, first, nameToId)) continue;
    for (const r of p.replaces ?? []) {
      const k = nameToId.get(r);
      // Only a row that would be wanted without the override is replaced: one the user refused,
      // or nothing pulls, goes on its own account. A row out of scope is never touched, nor
      // are its requirements pulled out under it.
      if (r === p.name || k == null || replaced.has(r) || !first.has(r)) continue;
      if (!inScopeAtRest(model, k)) continue;
      replaced.set(r, p.name);
    }
  }
  if (replaced.size === 0) return { wanted: first, replaced };
  return { wanted: pulledNames(model, new Set(replaced.keys())), replaced };
}
// Will row j really be there after the Apply? A replacement that cannot land replaces nothing,
// or the user would be left with neither. Read from facts that need no desire — scope.js's rule
// without its requirement half, then the requirements by presence or by the first pull — so it
// never recurses through requiresReason → desiredOf → pull.
function landsHere(model, j, first, nameToId) {
  const p = model.pkgs.get(j);
  if (!p || !inScopeAtRest(model, j)) return false;
  if (p.present === true) return true;
  if (!p.canUninstall) return false; // no route here
  if (p.hold) return false; // the server holds its install (a brew pin it cannot serve)
  return (p.requires ?? []).every((r) => {
    const k = nameToId.get(r);
    const q = k == null ? null : model.pkgs.get(k);
    return !!q && (q.present === true || (first.has(r) && !!q.canUninstall && inScopeAtRest(model, k)));
  });
}
function inScopeAtRest(model, i) {
  const m = manualScope(model, i);
  if (m) return m === "in";
  const p = model.pkgs.get(i);
  return !!p?.canUninstall && p.external !== true;
}
// The name of the wanted package that stands in for row i, or null.
export function replacedBy(model, i) {
  const name = model.pkgs.get(i)?.name;
  return name == null ? null : pull(model).replaced.get(name) ?? null;
}
// The row's word for the override, or null: `replaced by Beta`. Verbatim on the right of the
// row, like a requirement reason — the toggle keeps saying what the user chose.
export function replacedReason(model, i) {
  const by = replacedBy(model, i);
  return by == null ? null : `replaced by ${by}`;
}
// The pull: seeds, then the closure over `requires`, never through a name in `skip`.
function pulledNames(model, skip) {
  const nameToId = new Map();
  for (const [i, p] of model.pkgs) nameToId.set(p.name, i);
  const wanted = new Set();
  for (const [i, p] of model.pkgs) {
    if (refuses(model, i) || skip.has(p.name)) continue;
    if (manualToggle(model, i) === "in" || inActiveProfile(model, i)) wanted.add(p.name);
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const name of [...wanted]) {
      const p = model.pkgs.get(nameToId.get(name));
      for (const req of p?.requires ?? []) {
        const j = nameToId.get(req);
        if (j == null || refuses(model, j) || skip.has(req) || wanted.has(req)) continue;
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
// A replaced row is wanted absent even when its toggle says "in": the override wins over the
// intention without writing it (`replacedBy`). Manual rows are out of scope, so never touched.
export function desiredOf(model, i) {
  const name = model.pkgs.get(i)?.name;
  const { wanted, replaced } = pull(model);
  if (replaced.has(name)) return "absent";
  return desiredState(postureOf(model, i), manualToggle(model, i), name != null && wanted.has(name));
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
    available: p.available,
    canSelfUpdate: p.selfUpdate,
    updateEachApply: p.updateEachApply,
    updatedThisSession: p.updatedThisSession,
  });
}
// Will the next Apply run this row's own `update:` (`update-on: apply`)? Only for a row
// it will act on: an auto row is never touched, an unwanted or out-of-scope one is not updated.
export function isUpdatedEachApply(model, i) {
  const p = model.pkgs.get(i);
  return !!(p?.selfUpdate && p?.updateEachApply) && isSelfUpdating(model, i)
    && actionOf(model, i) === "upgrade";
}
// The server's `self-updated`: done for the session (its update exited ok, or the Apply installed
// it), or — `done` false — forgotten by a removal that exited ok.
export function setUpdatedThisSession(model, i, done = true) {
  const p = model.pkgs.get(i);
  if (p) p.updatedThisSession = !!done;
}
// Is this row done for the session — its update ran ok, and the Apply leaves it until a
// Refresh? The row says so instead of looking as if nothing had happened.
export function isUpdatedThisSession(model, i) {
  const p = model.pkgs.get(i);
  return !!(p?.selfUpdate && p?.updateEachApply && p?.updatedThisSession) && p.present === true
    && isSelfUpdating(model, i) && actionOf(model, i) === null;
}
// Does this row declare `version: self` — the application updates itself?
export function isSelfUpdating(model, i) {
  return String(model.pkgs.get(i)?.pin || "").toLowerCase() === "self";
}
// The version a change of this row would run toward: an exact pin owns it, otherwise what the
// manager announced. Twin of the server's `step_target`.
function targetOf(p) {
  const pin = String(p.pin || "");
  if (/^\d/.test(pin.trim())) return pin.trim();
  return p.available || "";
}
// The reason this row's change is held because it already ran, from the version installed
// now, toward the same target and moved nothing — or null. The machine moving or a new target
// lifts it. Twin of the server's `held_unmoved`.
function unmovedHold(model, i) {
  const p = model.pkgs.get(i);
  const u = p?.unmoved;
  const installed = bareVersion(p?.installedVersion || "");
  if (!u || !installed) return null;
  const a = actionOf(model, i);
  if (a !== "upgrade" && a !== "downgrade") return null;
  // Never judged by its version, so never held by it (the server's `judges_version`).
  if (isUpdatedEachApply(model, i)) return null;
  const same = installed === bareVersion(u.found) && bareVersion(targetOf(p)) === bareVersion(u.target);
  return same ? u.reason : null;
}
// A version change that exited ok and moved nothing (the server's `unmoved` message:
// `found`, `target`, `reason`). Off an exact pin, the row is outdated again toward what it ran
// for, shown greyed: the row says what it did not reach.
export function setUnmoved(model, i, { found, target, reason, ahead } = {}) {
  const p = model.pkgs.get(i);
  if (!p) return;
  // `ahead`: not a failure — the tool is past what its manager offers, and forcing the manager
  // would take it back (the server's `ahead_of_manager`).
  p.unmoved = { found: found || "", target: target || "", reason: reason || "", ahead: !!ahead };
  if (targetOf({ pin: p.pin, available: "" }) === "") {
    p.outdated = true;
    p.available = target || "";
  }
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
  const v = summary(model, i);
  // A change the Apply leaves (moved nothing last time, or brew cannot serve the pin): shown,
  // not pushed.
  return v.to !== null && holdReason(model, i) !== null ? { ...v, muted: true } : v;
}
function summary(model, i) {
  const none = { from: "", to: null, pinned: null, muted: false, held: false };
  const p = model.pkgs.get(i);
  if (!p) return none;
  const inst = p.present === true ? (p.installedVersion || "") : "";
  if (!inst) return none;
  if (p.pin) {
    const held = String(p.pin).toLowerCase() === "pending";
    const chasing = String(p.pin).toLowerCase() === "latest";
    // `self`: one number — the application keeps itself current — unless the row has its own
    // `update:` and the Apply will run it (`actionOf` decides, with the server's rule).
    if (String(p.pin).toLowerCase() === "self") {
      // Run by the Apply, behind or not: no target to show, the command decides.
      if (actionOf(model, i) === "upgrade" && !isUpdatedEachApply(model, i)) {
        return { from: inst, to: p.available || null, pinned: null, muted: false, held: false };
      }
      return { from: inst, to: null, pinned: null, muted: false, held: false };
    }
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
    // Same display as declaring nothing.
    if (chasing) return unpinned(p, inst);
    const cmp = compareToPin(inst, p.pin);
    if (cmp !== 0) {
      // Off the pin → migration TO the pin, in either direction: BELOW (cmp<0) is an
      // upgrade, ABOVE (cmp>0) a downgrade (an uninstall + reinstall) — both batched by the
      // Apply since 2026-10-10, so both are pushes. The one exception is a route that cannot
      // go down: muted, and `holdReason` says why.
      return { from: inst, to: p.pin, pinned: "to", muted: cmp > 0 && !p.canDowngrade, held: false };
    }
    // At the pin: normally just the pinned number. But if the machine-wide scan
    // found something newer, SHOW it greyed (muted) — visible to test, not pushed.
    if (p.outdated && p.available && compareVersions(p.available, p.pin) > 0) {
      return { from: inst, to: p.available, pinned: "from", muted: true, held: false };
    }
    return { from: inst, to: null, pinned: "from", muted: false, held: false };
  }
  return unpinned(p, inst);
}
// Unpinned (or `latest`): show the machine-wide available only when actually outdated — and
// only when it is AHEAD of what runs. At or past it, the manager's own record is what lags
// (the tool updated itself, or another copy comes first): the Apply's upgrade realigns that
// record, so the row shows the one version there is and the record behind it, never a push
// from a version to itself.
function unpinned(p, inst) {
  if (p.outdated && p.available && compareVersions(inst, p.available) >= 0) {
    return { from: inst, to: null, pinned: null, muted: false, held: false, record: p.current || "" };
  }
  if (p.outdated && p.available) {
    return { from: inst, to: p.available, pinned: null, muted: false, held: false };
  }
  return { from: inst, to: null, pinned: null, muted: false, held: false };
}
// Would a PLAIN APPLY act here? Mirrors the server's `batch_action` exactly: every action the
// rule returns is batched, the downgrade included (until 2026-10-10 it was the per-row button's
// alone) — except a downgrade on a route that builds no command for it, which is HELD and says
// why (`holdReason`). Keeping this in lockstep with the server is what stops the preview from
// lighting for a change the Apply won't make. See talos-version-pin.
const AUTO_ACTS = ["install", "uninstall", "upgrade", "downgrade"];
export function isActionable(model, i) {
  // One predicate for every hold (`holdReason`), so the preview and the row's words agree.
  if (holdReason(model, i) !== null) return false;
  return AUTO_ACTS.includes(actionOf(model, i));
}
// What the Apply would do, one action per row, in plan order — what the bar counts (bar.js).
// The actions stay apart, not summed: a downgrade is an uninstall first and an uninstall is a
// removal, and the spec wants both said before the click.
export function planActions(model) {
  return applyPlan(model).map((i) => actionOf(model, i));
}
// The rows whose removal the button announces — an uninstall, or a reinstall at the pin, which
// removes first. The Apply sends them, and the server runs no removal outside them
// (`server::drop_unannounced`): the label and the act cannot part on a destructive step.
export function announcedRemovals(model) {
  return applyPlan(model).filter((i) => ["uninstall", "downgrade"].includes(actionOf(model, i)));
}
// The settled status a row's presence reads as — what a `state` paints, for a message that
// settles a row without a verdict of its own (a diff's `self`).
export function presenceStatus(model, i) {
  const present = model.pkgs.get(i)?.present;
  return present === true ? "ok" : present === false ? "waiting" : "unknown";
}
// What each row the Apply would run took HERE last time, in plan order; 0 = never measured.
// The button's estimate sums them, and counts the zeros as unknown instead of free.
export function planSeconds(model) {
  return applyPlan(model).map((i) => model.pkgs.get(i)?.secs ?? 0);
}
// WHY the Apply leaves a row it would otherwise act on, or null. Three cases: a change that
// already ran and moved nothing (`unmovedHold`); a brew pin brew cannot serve, which the server
// says at Apply (`held`); and a pinned row above its pin whose route cannot go down, in the
// server's `NO_DOWNGRADE_ROUTE` words, word for word, so the row and the log say the same thing.
//
// The brew hold is kept on the row until the next plan rebuilds the model, and it explains only
// a change the Apply would still make — once the row reaches its pin, there is nothing to hold.
const HELD_ACTIONS = ["install", "upgrade", "downgrade"];
export function holdReason(model, i) {
  const unmoved = unmovedHold(model, i);
  if (unmoved) return unmoved;
  const a = actionOf(model, i);
  const p = model.pkgs.get(i);
  if (p?.hold && HELD_ACTIONS.includes(a)) return p.hold;
  // A removal kept because its replacement is not there (`server::send_kept`): only while the
  // override is what removes the row — never against the user's own refusal — and only until
  // the replacement is installed.
  if (a === "uninstall" && p?.holdRemoval && replacedBy(model, i) != null && !replacementHere(model, p.name)) {
    return p.holdRemoval;
  }
  if (a !== "downgrade") return null;
  return p?.canDowngrade ? null : "above its pin — this route cannot go down to it";
}
// The server held this row at Apply, with these words (`server::send_held`).
export function setHold(model, i, reason) {
  const p = model.pkgs.get(i);
  if (p) p.hold = reason || "";
}
// The server kept a row the override would have removed (`server::send_kept`). Apart from
// `hold`: a removal's hold must never stop an install, nor an install's hold a removal.
export function setRemovalHold(model, i, reason) {
  const p = model.pkgs.get(i);
  if (p) p.holdRemoval = reason || "";
}
function replacementHere(model, name) {
  for (const q of model.pkgs.values()) {
    if (q.present === true && (q.replaces ?? []).includes(name)) return true;
  }
  return false;
}
// The rows the Apply removes through the override, not by the user's own refusal: the server
// makes their removal wait for the replacement (`server::replacement_missing`).
export function replacedIndices(model) {
  return applyPlan(model).filter((i) => actionOf(model, i) === "uninstall" && replacedBy(model, i) != null);
}
// The one word a held row shows beside its reason: `unchanged` (it ran and moved nothing),
// `ahead` (the tool is past what its manager offers), `current` (at it, with a manager record
// the upgrade would not move), `held` (a pin this route cannot reach, or a winget copy that
// could not be located),
// or "" when nothing holds the row.
export function holdWord(model, i) {
  const why = holdReason(model, i);
  if (!why) return "";
  const u = model.pkgs.get(i)?.unmoved;
  // `ahead` is a success: past the target, or AT it with a record that will not move.
  if (u && u.reason === why) {
    if (!u.ahead) return "unchanged";
    return compareVersions(u.found, u.target) > 0 ? "ahead" : "current";
  }
  return "held";
}
// The rows the Apply leaves at a gap it cannot close, in index order. The empty state asks,
// so it never says "already matches" while one is held.
export function heldRows(model) {
  return [...model.pkgs.keys()].filter((i) => holdReason(model, i) !== null).sort((a, b) => a - b);
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
// The rows an Apply would act on — the WHOLE desire, in catalogue order. Until 2026-10-10 this
// was `rungPlan(model, rung)` and kept only the actions a rung of the ladder admitted; the
// ladder left because applying part of a desire built states nothing reasons about (a fresh
// config over a stale app). The server builds its own batch with the same rule (`action_for`);
// this is the preview the Apply button counts.
export function applyPlan(model) {
  const items = [];
  for (const i of [...model.pkgs.keys()].sort((a, b) => a - b)) {
    if (isActionable(model, i)) items.push(i);
  }
  return items;
}
// The one gesture a row offers outside a run: INSPECT a config-atom the user turned off and
// owns. Until 2026-10-10 this was `buttonAction`, which also installed, removed, updated and
// downgraded one row on its own — a second path that built the same partial states the
// ladder did. The Apply is now the only path that acts.
export function rowGesture(model, i) {
  const p = model.pkgs.get(i);
  if (p?.isConfig && desiredOf(model, i) === "absent") return { verb: "diff", type: "diff" };
  return null;
}

// --- touched: a gesture must leave a trace ----------------------------------
//
// A row the user acted on stays VISIBLE for the rest of the session, even when the
// action turns out to be a no-op. C's report, paraphrased: a row that vanishes under the
// user's eyes right after they act on it is odd and annoying, worse when the act was a
// mistake. Exactly so — the Changes tab shows only `.will-change` rows, so
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
// Reset: back to a first launch — clears BOTH manual toggles AND active profiles
// (profiles are additive intention; a true reset drops them too), then puts back the
// author's default cards, which is what a machine with no saved selection shows.
export function clearAllDecisions(model) {
  model.decision.clear();
  model.activeProfiles.clear();
  for (const name of defaultCards(model)) model.activeProfiles.add(name);
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
  // The row runs again: what it did not reach last time is no longer the news. If it moves
  // nothing again, the server says so again.
  if (["installing", "upgrading", "reinstalling", "uninstalling"].includes(status)) p.unmoved = null;
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
export function setOutdated(model, i, on, available = "", current = "") {
  const p = model.pkgs.get(i);
  if (!p) return;
  p.outdated = !!on;
  p.available = on ? (available || "") : "";
  p.current = on ? (current || "") : "";
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
// The author cards on at a first launch, as the engine marked them.
function defaultCards(model) {
  return new Set([...model.profiles.values()].filter((p) => p.defaultOn).map((p) => p.name));
}
export function isProfileActive(model, name) {
  return model.activeProfiles.has(name);
}
// Would a Reset do anything? True if the user moved a package off its author
// default OR the active cards are not the default ones — clearAllDecisions undoes both. The
// domain answer to "is Reset meaningful right now?", kept out of the view.
export function canReset(model) {
  // The active author cards differ from the default ones (the always-on personal bundle
  // aside): a card beyond them, or a default card unchecked, is a change.
  const defaults = defaultCards(model);
  for (const name of model.activeProfiles) {
    if (name !== PERSONAL_BUNDLE && !defaults.has(name)) return true;
  }
  for (const name of defaults) if (!model.activeProfiles.has(name)) return true;
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
    // A replaced member is the bundle's own choice, standing in for it: not "pulled out".
    return i != null && (toggleOf(model, i) === "in" || replacedBy(model, i) != null);
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
    // ⚠️ AND in scope. This counter never consulted the second axis, so it counted rows
    // Apply provably will not touch: `Base` read 10/12 on a Mac for two winget-only
    // rows. Not a requirements bug — an `external` or no-route member already inflated
    // the denominator before requirements entered the picture. The goal is what Apply
    // WILL put down, and out of scope means it will not (see actionOf, same gate).
    if (scopeOf(model, i) === "out") continue;
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

// --- the Doctor's dropdown, as data ---------------------------------------------------
// `agents` come from the engine: every catalogue package declaring `doctor:`, with its
// presence NOW and whether a clean launch is declared. Present ones are offered in
// catalogue order; the OS shell — the floor — is offered last and always, so the list is
// never empty even on a machine where no agent ever got installed. `remembered` is the
// last choice (ui prefs) and wins if that candidate is still present. Absent candidates
// are listed apart: the Doctor names them and points at the Catalog, it never installs.
export function doctorChoices(agents, shell, remembered) {
  const list = Array.isArray(agents) ? agents : [];
  const options = list
    .filter((a) => a && a.present)
    .map((a) => ({ value: a.name, label: a.name, clean: !!a.clean }));
  if (shell) {
    options.push({ value: "shell", label: `${shell} — OS shell, no profile`, clean: false });
  }
  const absent = list.filter((a) => a && !a.present).map((a) => a.name);
  const selected = options.some((o) => o.value === remembered)
    ? remembered
    : (options[0]?.value ?? "");
  return { options, absent, selected };
}

// --- Doctor sessions, the pure part -------------------------------------------------
// The label a session tab wears: its number, what runs, and whether it runs clean. The
// number is the session id — monotonic for the life of the tab strip — so two sessions
// of the same agent never wear the same name, and a closed "3" is never reused.
export function doctorSessionLabel(agent, clean, shellName, n) {
  const base = agent === "shell" ? (shellName || "shell") : agent;
  const what = clean ? `${base} · clean` : base;
  return n ? `${n} · ${what}` : what;
}
// Which tab takes the front after `closed` goes: the one that was active if it is not the
// closed one; else its right-hand neighbour, else its left, else nothing. Ids in tab
// order.
export function doctorNextActive(ids, activeId, closed) {
  const rest = ids.filter((i) => i !== closed);
  if (activeId !== closed && rest.includes(activeId)) return activeId;
  const at = ids.indexOf(closed);
  if (at < 0) return rest[0] ?? null;
  return ids[at + 1] ?? rest[at - 1] ?? rest[0] ?? null;
}

// ── the content's name ──────────────────────────────────────────────────────────────────
// `distribution` is the first message's `distribution` (name, version, label) or null.
// ONE rule, mirrored by the engine's `distribution::window_title` for the native title:
// the distribution leads — it is what a colleague recognises — and the engine's tag
// follows, for support. Without a distribution, byte for byte the title of every release.

export function windowTitle(distribution, tag) {
  return distribution && distribution.label
    ? `${distribution.label} · Talos ${tag}`
    : `Talos · ${tag}`;
}

/** The footer: the distribution, then the exact snapshot (tag · sha — builtAt). */
export function buildLine(distribution, build) {
  const stamp = `${build.tag} · ${build.sha} — ${build.builtAt}`;
  return distribution && distribution.label ? `${distribution.label} · ${stamp}` : stamp;
}
