// Ported from the Deno-era test/model.test.ts to node:test (the frontend tests
// were not carried through the Tauri/Rust port; this restores model.js coverage).
// Run: node --test test/model.test.mjs   (from talos/). Pure — no DOM, no server.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  actionOf,
  addToPersonal,
  applyProfile,
  applySavedActiveBundles,
  applySavedScope,
  applySavedSelection,
  rowGesture,
  canReset,
  clearAllDecisions,
  createModel,
  desiredOf,
  isActionable,
  isDeviated,
  isInPersonal,
  isPending,
  isLocked,
  isProfileActive,
  isTouched,
  applyPlan,
  holdReason,
  heldRows,
  planCounts,
  planSeconds,
  loadPlan,
  PERSONAL_BUNDLE,
  persistableActiveBundles,
  persistablePkgs,
  persistableScope,
  profileProgress,
  profilesForPkg,
  profileStateOf,
  removeFromPersonal,
  removeProfile,
  requiresReason,
  scopeOf,
  scopeReason,
  setDecision,
  setExternal,
  setInstalledVersion,
  setOutdated,
  setPresence,
  setScope,
  setStatusData,
  setUnmoved,
  isSelfUpdating,
  isUpdatedEachApply,
  toggleOf,
  unmanagedIndices,
  versionSummary,
} from "../public/model.js";

test("personal bundle: exists, always active, empty by default", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "rg", bundle: "", posture: "opt-in" }], []);
  assert.equal(isProfileActive(m, PERSONAL_BUNDLE), true); // always on
  assert.deepEqual(m.profiles.get(PERSONAL_BUNDLE).packages, []);
});

test("bundle cascade: activating a bundle activates its needs transitively", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "uv", bundle: "", posture: "opt-in" }], [
    { name: "Base", packages: [], needs: [] },
    { name: "Documents", packages: [], needs: ["Base"] },
    { name: "Data", packages: ["uv"], needs: ["Documents"] },
  ]);
  applyProfile(m, "Data");
  // Data + its chain are active (as if clicked, §23).
  assert.equal(isProfileActive(m, "Data"), true);
  assert.equal(isProfileActive(m, "Documents"), true);
  assert.equal(isProfileActive(m, "Base"), true);
  // Deactivating Data does NOT reverse-cascade — Documents/Base stay.
  removeProfile(m, "Data");
  assert.equal(isProfileActive(m, "Data"), false);
  assert.equal(isProfileActive(m, "Documents"), true);
  assert.equal(isProfileActive(m, "Base"), true);
});

test("active bundles persist + restore (with cascade)", () => {
  const m = createModel();
  const plan = [
    { name: "Base", packages: [], needs: [] },
    { name: "Documents", packages: [], needs: ["Base"] },
    { name: "Terminal", packages: [], needs: [] },
  ];
  loadPlan(m, [], plan);
  applyProfile(m, "Documents"); // cascades Base
  applyProfile(m, "Terminal");
  const saved = persistableActiveBundles(m); // excludes the always-on personal
  assert.deepEqual(saved.sort(), ["Base", "Documents", "Terminal"]);
  // Fresh model, restore
  const m2 = createModel();
  loadPlan(m2, [], plan);
  applySavedActiveBundles(m2, saved);
  assert.equal(isProfileActive(m2, "Documents"), true);
  assert.equal(isProfileActive(m2, "Base"), true);
  assert.equal(isProfileActive(m2, "Terminal"), true);
});

test("deactivating a needed bundle reverse-cascades its dependents", () => {
  const m = createModel();
  loadPlan(m, [], [
    { name: "Base", packages: [], needs: [] },
    { name: "Documents", packages: [], needs: ["Base"] },
    { name: "Development", packages: [], needs: ["Documents"] },
    { name: "Terminal", packages: [], needs: [] }, // independent
  ]);
  applyProfile(m, "Development"); // activates Documents + Base
  applyProfile(m, "Terminal");
  removeProfile(m, "Base"); // pull the socle out
  // Everything that (transitively) needs Base goes too; the invariant holds.
  assert.equal(isProfileActive(m, "Base"), false);
  assert.equal(isProfileActive(m, "Documents"), false);
  assert.equal(isProfileActive(m, "Development"), false);
  // Independent bundle untouched.
  assert.equal(isProfileActive(m, "Terminal"), true);
});

test("bundle cascade is cycle-guarded", () => {
  const m = createModel();
  loadPlan(m, [], [
    { name: "A", packages: [], needs: ["B"] },
    { name: "B", packages: [], needs: ["A"] }, // cycle
  ]);
  applyProfile(m, "A"); // must not infinite-loop
  assert.equal(isProfileActive(m, "A"), true);
  assert.equal(isProfileActive(m, "B"), true);
});

test("addToPersonal / removeFromPersonal grow it and pull the package", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "rg", bundle: "", posture: "opt-in" }], []);
  addToPersonal(m, "rg");
  assert.deepEqual(m.profiles.get(PERSONAL_BUNDLE).packages, ["rg"]);
  assert.equal(isInPersonal(m, "rg"), true);
  assert.equal(toggleOf(m, 0), "in"); // promoted by My setup
  assert.equal(desiredOf(m, 0), "present");
  removeFromPersonal(m, "rg");
  assert.deepEqual(m.profiles.get(PERSONAL_BUNDLE).packages, []);
  assert.equal(toggleOf(m, 0), "out"); // no longer promoted
});

// A plan WITH profiles: reuses the opt-in rg/bat fixture, adds two profiles.
function seedWithProfiles() {
  const m = createModel();
  loadPlan(
    m,
    [
      // ⚠️ `canUninstall: true` is not decoration: without it these rows are derived
      // OUT of scope (no way back → `no-route`), and profileProgress now honours the
      // second axis. Three normal, fully manageable rows is what these tests mean.
      { i: 0, name: "Node", bundle: "Base", posture: "mandatory", canUninstall: true },
      { i: 1, name: "rg", bundle: "Extras", posture: "opt-in", canUninstall: true },
      { i: 2, name: "bat", bundle: "Extras", posture: "opt-in", canUninstall: true },
    ],
    [
      { name: "Search", emoji: "🔎", packages: ["rg"] },
      { name: "All tools", emoji: "🧰", packages: ["rg", "bat"] },
    ],
  );
  return m;
}

// A small fixture plan: one mandatory pkg, one opt-in, one opt-out, in 2 bundles.
function seed() {
  const m = createModel();
  loadPlan(
    m,
    [
      {
        i: 0,
        name: "Node",
        bundle: "Base",
        posture: "mandatory",
        canUninstall: true,
      },
      {
        i: 1,
        name: "rg",
        bundle: "Extras",
        posture: "opt-in",
        canUninstall: true,
      },
      {
        i: 2,
        name: "bat",
        bundle: "Extras",
        posture: "opt-in",
        canUninstall: true,
      },
    ],
  );
  return m;
}

test("loadPlan loads the flat step list keyed by package name", () => {
  const m = seed();
  assert.deepEqual(m.pkgs.size, 3);
  assert.deepEqual(m.pkgs.get(1)?.key, "rg"); // key = package name now (flat catalog)
});

test("bundle-driven: everything is OUT until pulled or set (posture ≠ default side)", () => {
  const m = seed();
  assert.deepEqual(toggleOf(m, 0), "out"); // mandatory no longer "always in"
  assert.deepEqual(toggleOf(m, 1), "out"); // opt-in out
  assert.deepEqual(desiredOf(m, 1), "absent");
});

test("setDecision moves a package in; a manual set makes it wanted", () => {
  const m = seed();
  assert.deepEqual(setDecision(m, 1, "in"), true);
  assert.deepEqual(toggleOf(m, 1), "in");
  assert.deepEqual(isDeviated(m, 1), true);
});

test("actionOf: wanted+absent → install; wanted+present+current → null", () => {
  const m = seed();
  setDecision(m, 1, "in"); // pull it in
  setStatusData(m, 1, "waiting"); // absent
  assert.deepEqual(actionOf(m, 1), "install");
  assert.deepEqual(isActionable(m, 1), true);
  setStatusData(m, 1, "ok"); // now present + current
  assert.deepEqual(actionOf(m, 1), null); // wanted + satisfied → nothing
});

test("actionOf: an OUTDATED package the user turned OUT → uninstall, not update", () => {
  const m = seed(); // pkg 1 = opt-in rg, canUninstall
  setStatusData(m, 1, "ok"); // present
  setOutdated(m, 1, true, "9.9"); // and outdated
  setDecision(m, 1, "out"); // but the user wants it GONE
  // The desired state (absent) must win: you don't "update" something you're removing.
  assert.deepEqual(actionOf(m, 1), "uninstall");
});

// --- version pinning at the model layer -------------------------------------
// A pin plan seeds one pinned package the user WANTS (setDecision in — nothing is
// wanted by default now), so the installed version drives the three-way action.
// See memory talos-version-pin.
function seedPinned(extra = {}) {
  const m = createModel();
  loadPlan(
    m,
    [{
      i: 0,
      name: "jq",
      bundle: "Base",
      posture: "opt-out", // non-locked, so the user can pull it in (mandatory
      // stays locked-from-manual; a pin test just needs a WANTED package)
      canUninstall: true,
      pin: "1.8",
      ...extra,
    }],
  );
  setDecision(m, 0, "in"); // wanted — bundle-driven model has no default-in
  return m;
}

test("actionOf: pinned + installed below pin → upgrade, at pin → null, above → downgrade", () => {
  const m = seedPinned();
  setStatusData(m, 0, "ok"); // present
  setInstalledVersion(m, 0, "1.5");
  assert.deepEqual(actionOf(m, 0), "upgrade");
  setInstalledVersion(m, 0, "1.8");
  assert.deepEqual(actionOf(m, 0), null); // at the pin → satisfied
  setInstalledVersion(m, 0, "2.0");
  assert.deepEqual(actionOf(m, 0), "downgrade");
});

// versionSummary — the SINGLE source of the row's version display. Never more
// than two numbers, never a repeat, and a pin is marked exactly once (`pinned:
// "from" | "to" | null`) so the view can badge it. This replaces the old split
// between the statusLabel (installed) and the delta (cur→avail) that doubled the
// current version on screen. See the UX pass in talos-version-pin.
test("versionSummary: unpinned + up to date → just the installed version, no arrow", () => {
  const m = seed();
  setStatusData(m, 1, "ok");
  setInstalledVersion(m, 1, "26.5.0");
  assert.deepEqual(versionSummary(m, 1), {
    from: "26.5.0",
    to: null,
    pinned: null,
    muted: false,
    held: false,
  });
});

test("versionSummary: unpinned + outdated → cur→avail ONCE, not muted (a real push)", () => {
  const m = seed();
  setStatusData(m, 1, "ok");
  setInstalledVersion(m, 1, "26.4.0");
  setOutdated(m, 1, true, "26.5.0"); // available version now carried in the model
  assert.deepEqual(versionSummary(m, 1), {
    from: "26.4.0",
    to: "26.5.0",
    pinned: null,
    muted: false,
    held: false,
  });
});

test("versionSummary: pinned AT the pin, NO upgrade → single number, marked pin, no arrow", () => {
  const m = seedPinned(); // pin 1.8
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.8");
  assert.deepEqual(versionSummary(m, 0), {
    from: "1.8",
    to: null,
    pinned: "from",
    muted: false,
    held: false,
  });
});

test("versionSummary: pinned AT the pin, upgrade AVAILABLE → 📌pin → avail(muted)", () => {
  const m = seedPinned(); // pin 1.8
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.8");
  // The machine-wide scan found something newer. We're at the pin, so we don't
  // push it — but we SHOW it, greyed: "you're pinned here; a newer one exists to
  // test if you want". The pin (from) stays normal; the avail (to) is muted.
  setOutdated(m, 0, true, "2.5");
  assert.deepEqual(versionSummary(m, 0), {
    from: "1.8",
    to: "2.5",
    pinned: "from",
    muted: true,
    held: false,
  });
});

test("versionSummary: pinned BELOW → installed→pin, pin on target, not muted (real migration)", () => {
  const m = seedPinned(); // pin 1.8
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.5");
  assert.deepEqual(versionSummary(m, 0), {
    from: "1.5",
    to: "1.8",
    pinned: "to",
    muted: false,
    held: false,
  });
});

test("versionSummary: pinned ABOVE with a route down → installed→pin, a real push", () => {
  // Since 2026-10-10 the Apply takes a downgrade (an uninstall + reinstall at the pin), so a
  // route that can go down shows its target like any push — the Apply WILL go there.
  const m = seedPinned({ canDowngrade: true }); // pin 1.8
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.0");
  assert.deepEqual(versionSummary(m, 0), {
    from: "2.0",
    to: "1.8",
    pinned: "to",
    muted: false,
    held: false,
  });
});

test("versionSummary: pinned ABOVE with NO route down → installed→pin, MUTED, and it says why", () => {
  // The route builds no downgrade command (the server sends `canDowngrade: false`), so the
  // Apply cannot go there: the target reads muted, and holdReason names the obstacle.
  const m = seedPinned(); // pin 1.8, canDowngrade absent → false
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.0");
  assert.equal(holdReason(m, 0), "above its pin — this route cannot go down to it");
  setInstalledVersion(m, 0, "1.5");
  assert.equal(holdReason(m, 0), null, "an upgrade has nothing to explain");
  setInstalledVersion(m, 0, "2.0");
  assert.deepEqual(versionSummary(m, 0), {
    from: "2.0",
    to: "1.8",
    pinned: "to",
    muted: true,
    held: false,
  });
});

test("heldRows: the rows the Apply leaves at a gap it cannot close", () => {
  // The empty state reads this: with one held row, "this machine already matches" is false.
  const m = seedPinned(); // canDowngrade absent → no route down
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.8");
  assert.deepEqual(heldRows(m), [], "at the pin: nothing held");
  setInstalledVersion(m, 0, "2.0");
  assert.deepEqual(heldRows(m), [0], "above the pin, no route down: held");
  m.pkgs.get(0).canDowngrade = true;
  assert.deepEqual(heldRows(m), [], "a route down: batched, not held");
});

test("self: one number, never arbitration, and the Apply leaves it unless it has its own update", () => {
  const m = seedPinned({ pin: "self" });
  setDecision(m, 0, "in");
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.141.2");
  setOutdated(m, 0, true, "1.142.0");
  assert.equal(isSelfUpdating(m, 0), true);
  assert.equal(isPending(m, 0), false, "a decision made is not arbitration");
  assert.equal(actionOf(m, 0), null);
  assert.deepEqual(versionSummary(m, 0), { from: "1.141.2", to: null, pinned: null, muted: false, held: false });
  // With `update:` (the server sends `selfUpdate`), a tool behind what is announced is a push.
  m.pkgs.get(0).selfUpdate = true;
  assert.equal(actionOf(m, 0), "upgrade");
  assert.deepEqual(versionSummary(m, 0), { from: "1.141.2", to: "1.142.0", pinned: null, muted: false, held: false });
  setInstalledVersion(m, 0, "1.142.0");
  assert.equal(actionOf(m, 0), null, "the receipt lags; the tool is current");
  assert.deepEqual(versionSummary(m, 0), { from: "1.142.0", to: null, pinned: null, muted: false, held: false });
});

test("update-on apply: the Apply runs the tool's update every time, and the row shows one number", () => {
  const m = seedPinned({ pin: "self" });
  setDecision(m, 0, "in");
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.1.308");
  m.pkgs.get(0).selfUpdate = true;
  m.pkgs.get(0).updateEachApply = true;
  assert.equal(isUpdatedEachApply(m, 0), true);
  assert.equal(actionOf(m, 0), "upgrade", "nothing announced, and still run");
  assert.equal(isActionable(m, 0), true);
  assert.deepEqual(versionSummary(m, 0), { from: "2.1.308", to: null, pinned: null, muted: false, held: false });
  // Never held by a remembered "unchanged": such a row is never judged by its version.
  setUnmoved(m, 0, { found: "2.1.308", target: "", reason: "it moved nothing" });
  assert.equal(isActionable(m, 0), true);
  assert.equal(holdReason(m, 0), null);
  // Out of scope: the Apply leaves it, and the row does not claim otherwise.
  setScope(m, 0, "out", { trace: false });
  assert.equal(isUpdatedEachApply(m, 0), false);
  setScope(m, 0, null, { trace: false });
  // Wanted absent: an uninstall, never an update — and the row no longer says it is updated.
  setDecision(m, 0, "out");
  assert.equal(actionOf(m, 0), "uninstall");
  assert.equal(isUpdatedEachApply(m, 0), false, "only a row the Apply keeps present");
});

test("self: outdated with no version named — a push, not an arrow to nothing", () => {
  const m = seedPinned({ pin: "self" });
  setDecision(m, 0, "in");
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.0");
  setOutdated(m, 0, true, "");
  m.pkgs.get(0).selfUpdate = true;
  assert.equal(actionOf(m, 0), "upgrade");
  assert.equal(versionSummary(m, 0).to, null);
});

test("unmoved: a change that exited ok and moved nothing is held while the same version faces the same target", () => {
  const m = seedPinned({ pin: null });
  setDecision(m, 0, "in");
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.141.2");
  setOutdated(m, 0, true, "1.142.0");
  assert.equal(actionOf(m, 0), "upgrade");
  assert.equal(isActionable(m, 0), true);
  const why = "winget reported success, but 1.141.2 is still installed, not 1.142.0";
  setStatusData(m, 0, "ok"); // the step's own status arrives first, and clears `outdated`
  setUnmoved(m, 0, { found: "1.141.2", target: "1.142.0", reason: why });
  assert.equal(isActionable(m, 0), false, "not run again toward the same target");
  assert.equal(holdReason(m, 0), why);
  assert.deepEqual(heldRows(m), [0], "the empty state must not claim a match");
  assert.deepEqual(versionSummary(m, 0), { from: "1.141.2", to: "1.142.0", pinned: null, muted: true, held: false });
  // The manifest changed: a new target lifts the hold.
  setOutdated(m, 0, true, "1.142.1");
  assert.equal(isActionable(m, 0), true);
  assert.equal(holdReason(m, 0), null);
  // The machine moved — by hand, or by the tool — : the hold no longer describes it.
  setOutdated(m, 0, true, "1.142.0");
  assert.equal(isActionable(m, 0), false);
  setInstalledVersion(m, 0, "1.141.5");
  assert.equal(isActionable(m, 0), true, "detect, don't remember");
  // A change toward no named version is held too, while the version stays.
  const n = seedPinned({ pin: null });
  setDecision(n, 0, "in");
  setStatusData(n, 0, "ok");
  setInstalledVersion(n, 0, "2.50.1");
  setOutdated(n, 0, true, "");
  setUnmoved(n, 0, { found: "2.50.1", target: "", reason: "brew reported success, but 2.50.1 is still installed" });
  assert.equal(isActionable(n, 0), false);
  assert.match(holdReason(n, 0), /still installed/);
  // A pin: held while the pin is the same.
  const p = seedPinned({ canDowngrade: true }); // pin 1.8
  setDecision(p, 0, "in");
  setStatusData(p, 0, "ok");
  setInstalledVersion(p, 0, "2.0");
  setUnmoved(p, 0, { found: "2.0", target: "1.8", reason: "winget reported success, but 2.0 is installed, not 1.8" });
  assert.equal(isActionable(p, 0), false);
  assert.match(holdReason(p, 0), /not 1\.8/);
});

test("versionSummary: absent → empty (nothing installed to show)", () => {
  const m = seedPinned();
  setStatusData(m, 0, "waiting"); // absent
  assert.deepEqual(versionSummary(m, 0), {
    from: "",
    to: null,
    pinned: null,
    muted: false,
    held: false,
  });
});

test("isActionable: a downgrade is batched — the Apply honours a pin both ways", () => {
  // Mirrors the server's `batch_action`: a downgrade runs when the route can go down, and is
  // held (with holdReason) when it cannot.
  const m = seedPinned({ canDowngrade: true });
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.0"); // above pin → downgrade
  assert.deepEqual(isActionable(m, 0), true);
  m.pkgs.get(0).canDowngrade = false;
  assert.deepEqual(isActionable(m, 0), false, "no route down → held, not batched");
  // Below the pin (upgrade) IS in the plan.
  setInstalledVersion(m, 0, "1.5");
  assert.deepEqual(isActionable(m, 0), true);
});

test("persist + restore selection by key", () => {
  const m = seed();
  setDecision(m, 1, "in");
  const saved = { pkgs: persistablePkgs(m) };
  assert.deepEqual(saved.pkgs, { "rg": "in" }); // key = package name now
  const m2 = seed();
  applySavedSelection(m2, saved);
  assert.deepEqual(toggleOf(m2, 1), "in");
});

test("clearAllDecisions returns everything to posture defaults", () => {
  const m = seed();
  setDecision(m, 1, "in");
  clearAllDecisions(m);
  assert.deepEqual(toggleOf(m, 1), "out"); // back to opt-in default
  assert.deepEqual(m.decision.size, 0);
});

// --- profiles: additive pull, hollow on manual out, clean removal ------------
test("applyProfile: pulls its packages in (additive, no decision written)", () => {
  const m = seedWithProfiles();
  assert.deepEqual(toggleOf(m, 1), "out"); // rg opt-in, default out
  applyProfile(m, "Search");
  assert.deepEqual(toggleOf(m, 1), "in"); // pulled in by the profile
  assert.deepEqual(desiredOf(m, 1), "present");
  assert.deepEqual(m.decision.size, 0); // no manual decision written — purely additive
  assert.deepEqual(profileStateOf(m, "Search"), "full");
});

test("profileStateOf: off when not active", () => {
  const m = seedWithProfiles();
  assert.deepEqual(profileStateOf(m, "Search"), "off");
  assert.deepEqual(profileStateOf(m, "All tools"), "off");
});

test("profileStateOf: hollow when a manual out overrides the pull", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools"); // pulls rg + bat in
  assert.deepEqual(profileStateOf(m, "All tools"), "full");
  setDecision(m, 2, "out"); // user pulls bat back out
  assert.deepEqual(toggleOf(m, 2), "out"); // manual out wins over the profile
  assert.deepEqual(profileStateOf(m, "All tools"), "hollow");
});

test("removeProfile: pull vanishes, but a shared package survives", () => {
  const m = seedWithProfiles();
  applyProfile(m, "Search"); // rg
  applyProfile(m, "All tools"); // rg + bat
  assert.deepEqual(toggleOf(m, 1), "in"); // rg pulled by both
  removeProfile(m, "All tools");
  assert.deepEqual(toggleOf(m, 1), "in"); // rg survives — still pulled by Search
  assert.deepEqual(toggleOf(m, 2), "out"); // bat drops — only All tools wanted it
  assert.deepEqual(profileStateOf(m, "Search"), "full");
});

test("canReset: false at rest, true after a manual move or a profile", () => {
  const m = seedWithProfiles();
  assert.deepEqual(canReset(m), false); // pristine
  setDecision(m, 1, "in");
  assert.deepEqual(canReset(m), true); // moved a package
  clearAllDecisions(m);
  assert.deepEqual(canReset(m), false);
  applyProfile(m, "Search");
  assert.deepEqual(canReset(m), true); // active profile alone is enough
});

test("profilesForPkg: lists the profiles a package belongs to", () => {
  const m = seedWithProfiles();
  // rg is in both Search and All tools; bat only in All tools; Node in neither.
  assert.deepEqual(profilesForPkg(m, 1).map((p) => p.name), [
    "Search",
    "All tools",
  ]);
  assert.deepEqual(profilesForPkg(m, 2).map((p) => p.name), ["All tools"]);
  assert.deepEqual(profilesForPkg(m, 0), []);
});

test("profileProgress: installed / WANTED — total is the goal, present the progress", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools"); // pull rg + bat in → both wanted
  // wanted 2, none installed yet
  assert.deepEqual(profileProgress(m, "All tools"), { present: 0, total: 2 });
  setStatusData(m, 1, "ok"); // rg present
  assert.deepEqual(profileProgress(m, "All tools"), { present: 1, total: 2 });
  setStatusData(m, 2, "ok"); // bat present
  assert.deepEqual(profileProgress(m, "All tools"), { present: 2, total: 2 });
});

test("profileProgress: unknown profile → 0/0", () => {
  const m = seedWithProfiles();
  assert.deepEqual(profileProgress(m, "Nope"), { present: 0, total: 0 });
});

test("profileProgress: inactive bundle wants nothing → 0/0", () => {
  const m = seedWithProfiles();
  // Not applied → no package is desired-present → the goal is empty.
  assert.deepEqual(profileProgress(m, "All tools"), { present: 0, total: 0 });
});

test("profileProgress: total shrinks when a member is de-selected (extra out)", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools"); // rg + bat wanted → total 2
  assert.deepEqual(profileProgress(m, "All tools"), { present: 0, total: 2 });
  setDecision(m, 2, "out"); // user drops bat → no longer in the goal
  assert.deepEqual(profileProgress(m, "All tools"), { present: 0, total: 1 });
});

test("clearAllDecisions drops author profiles but keeps personal empty+active", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools");
  addToPersonal(m, "rg"); // user also picked something
  setDecision(m, 1, "out");
  clearAllDecisions(m);
  // Only the always-on personal bundle remains active, now empty.
  assert.deepEqual([...m.activeProfiles], [PERSONAL_BUNDLE]);
  assert.deepEqual(m.profiles.get(PERSONAL_BUNDLE).packages, []);
  assert.deepEqual(m.decision.size, 0);
  assert.deepEqual(toggleOf(m, 1), "out"); // back to opt-in default
  assert.deepEqual(profileStateOf(m, "All tools"), "off");
});

// The author's `default: on` cards. The ENGINE computes the list (needs included) and marks
// each card `defaultOn`; the model only restores it. A Reset goes back to that first-launch
// state — "forget my choices" means the author's proposal, not an empty screen.
function seedWithDefaultCards() {
  const m = createModel();
  loadPlan(
    m,
    [
      { i: 0, name: "Node", canUninstall: true },
      { i: 1, name: "rg", canUninstall: true },
    ],
    [
      { name: "Base", packages: ["Node"], defaultOn: true },
      { name: "Search", packages: ["rg"], needs: ["Base"] },
    ],
  );
  return m;
}

test("clearAllDecisions returns to the author's default cards", () => {
  const m = seedWithDefaultCards();
  applyProfile(m, "Search");
  setDecision(m, 0, "out");
  clearAllDecisions(m);
  assert.deepEqual([...m.activeProfiles].sort(), ["Base", PERSONAL_BUNDLE].sort());
  assert.equal(m.decision.size, 0);
});

test("canReset is false on the default cards, true once one is unchecked", () => {
  const m = seedWithDefaultCards();
  applyProfile(m, "Base"); // what a first launch restores
  assert.equal(canReset(m), false, "the author's proposal is the rest state");
  removeProfile(m, "Base");
  assert.equal(canReset(m), true, "unchecking a default card is a change");
  applyProfile(m, "Base");
  applyProfile(m, "Search");
  assert.equal(canReset(m), true, "a card beyond the defaults is a change");
});

test("rowGesture offers diff on a self-owned config-atom, and nothing else", () => {
  // The row button lost its ACTION role on 2026-10-10 — the Apply is the one path that
  // acts. What stays is inspection: the diff of a config the user chose to own.
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Starship config", bundle: "Terminal", posture: "opt-out", isConfig: true },
    { i: 1, name: "ripgrep", bundle: "Terminal", posture: "opt-in", canUninstall: true },
  ], []);
  setDecision(m, 0, "out");
  setDecision(m, 1, "in");
  setStatusData(m, 1, "waiting"); // absent → the Apply would install it
  assert.equal(actionOf(m, 1), "install", "the fixture is what it claims");
  assert.deepEqual(rowGesture(m, 0), { verb: "diff", type: "diff" });
  assert.equal(rowGesture(m, 1), null, "an install is the Apply's, not the row's");
  setDecision(m, 0, "in");
  assert.equal(rowGesture(m, 0), null, "a managed config-atom is converged, not inspected");
});

// --- B5: transitive requires pull (spec Consolidation §8) --------------------
// Wanting a package pulls its `requires:` in too, transitively. A package is
// wanted if a bundle pulls it OR the user forces it OR a wanted package requires
// it. The walk is cycle-guarded and lives in model.js (not Rust).
test("requires pull: forcing a package in pulls its direct requirement in", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Claude Code", bundle: "", posture: "opt-in", requires: ["Node.js"] },
    { i: 1, name: "Node.js", bundle: "", posture: "opt-in" },
  ], []);
  setDecision(m, 0, "in"); // want Claude Code
  assert.deepEqual(toggleOf(m, 1), "in"); // Node.js pulled in by the requirement
  assert.deepEqual(desiredOf(m, 1), "present");
});

test("requires pull: transitive through a chain (astral → Claude Code → Node.js)", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "astral", bundle: "", posture: "opt-in", requires: ["Claude Code"] },
    { i: 1, name: "Claude Code", bundle: "", posture: "opt-in", requires: ["Node.js"] },
    { i: 2, name: "Node.js", bundle: "", posture: "opt-in" },
  ], []);
  setDecision(m, 0, "in"); // want astral only
  assert.deepEqual(toggleOf(m, 1), "in"); // Claude Code pulled (direct)
  assert.deepEqual(toggleOf(m, 2), "in"); // Node.js pulled (transitive)
});

test("requires pull: an unwanted package does NOT pull its requirements", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Claude Code", bundle: "", posture: "opt-in", requires: ["Node.js"] },
    { i: 1, name: "Node.js", bundle: "", posture: "opt-in" },
  ], []);
  // nobody wants Claude Code → Node.js stays out (nothing-by-default)
  assert.deepEqual(toggleOf(m, 0), "out");
  assert.deepEqual(toggleOf(m, 1), "out");
});

test("requires pull: a manual 'out' on the required package wins over the pull", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Claude Code", bundle: "", posture: "opt-in", requires: ["Node.js"] },
    { i: 1, name: "Node.js", bundle: "", posture: "opt-in" },
  ], []);
  setDecision(m, 0, "in"); // want Claude Code
  setDecision(m, 1, "out"); // but veto Node.js by hand
  assert.deepEqual(toggleOf(m, 1), "out"); // manual out wins (guard rail)
});

test("requires pull: a cycle does not hang (a requires b, b requires a)", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "a", bundle: "", posture: "opt-in", requires: ["b"] },
    { i: 1, name: "b", bundle: "", posture: "opt-in", requires: ["a"] },
  ], []);
  setDecision(m, 0, "in");
  assert.deepEqual(toggleOf(m, 0), "in");
  assert.deepEqual(toggleOf(m, 1), "in"); // pulled by a, cycle-guarded (terminates)
});

test("requires pull: a bundle pulling a package also pulls that package's requires", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Claude Code", bundle: "", posture: "opt-in", requires: ["Node.js"] },
    { i: 1, name: "Node.js", bundle: "", posture: "opt-in" },
  ], [
    { name: "Base", packages: ["Claude Code"], needs: [] },
  ]);
  applyProfile(m, "Base"); // bundle pulls Claude Code
  assert.deepEqual(toggleOf(m, 0), "in"); // pulled by bundle
  assert.deepEqual(toggleOf(m, 1), "in"); // Node.js pulled via Claude Code's requires
});

test("scope: an out-of-scope row yields NO action, whatever the machine says", () => {
  const m = createModel();
  loadPlan(m, [
    // canUninstall:false → derived out of scope (a config-atom, no way back)
    { i: 0, name: "Starship config", canUninstall: false, isConfig: true },
    // a normal row, for contrast
    { i: 1, name: "Nushell", canUninstall: true },
  ]);
  // Both are wanted, both absent → row 1 would install, row 0 must not.
  setDecision(m, 0, "in");
  setDecision(m, 1, "in");
  assert.equal(scopeOf(m, 0), "out", "no way back → derived out");
  assert.equal(scopeOf(m, 1), "in");
  assert.equal(actionOf(m, 0), null, "out of scope → no action, ever");
  assert.equal(actionOf(m, 1), "install");
  assert.equal(isActionable(m, 0), false);
  assert.equal(isActionable(m, 1), true);
});

test("scope: gates ACTION but never OBSERVATION", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Git", canUninstall: true }]);
  setExternal(m, 0, true); // the probe found it installed outside our manager
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.51.0");
  setOutdated(m, 0, true, "2.52.0");
  assert.equal(scopeOf(m, 0), "out", "external → derived out");
  assert.equal(actionOf(m, 0), null, "never acted on");
  // …yet everything observed is still there, and still true.
  assert.equal(m.pkgs.get(0).present, true);
  assert.equal(m.pkgs.get(0).installedVersion, "2.51.0");
  assert.equal(m.pkgs.get(0).outdated, true);
  assert.equal(versionSummary(m, 0).from, "2.51.0");
});

test("scope: an uninstall is refused for an out-of-scope row (the Git hazard)", () => {
  const m = createModel();
  // Git declares `brew: git`, so canUninstall is TRUE — the hazard is that Talos
  // would happily run `brew uninstall git` on a binary brew never installed.
  loadPlan(m, [{ i: 0, name: "Git", canUninstall: true }]);
  setExternal(m, 0, true);
  setStatusData(m, 0, "ok");
  setDecision(m, 0, "out"); // the user asks for it gone
  assert.equal(desiredOf(m, 0), "absent", "the desire is real and preserved");
  assert.equal(actionOf(m, 0), null, "but scope refuses to act on it");
});

test("scope: the manual override wins in both directions", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Git", canUninstall: true }, { i: 1, name: "Nushell", canUninstall: true }]);
  setExternal(m, 0, true);
  setStatusData(m, 0, "ok");
  setDecision(m, 0, "out");
  // Pull the derived-out row back IN → the action returns.
  setScope(m, 0, "in");
  assert.equal(scopeOf(m, 0), "in");
  assert.equal(actionOf(m, 0), "uninstall", "in scope again → the action is live");
  // Push a normal row OUT → its action goes.
  setDecision(m, 1, "in");
  assert.equal(actionOf(m, 1), "install");
  setScope(m, 1, "out");
  assert.equal(actionOf(m, 1), null);
  // Clearing the override returns to the derivation.
  setScope(m, 1, null);
  assert.equal(scopeOf(m, 1), "in");
  assert.equal(actionOf(m, 1), "install");
});

test("scope: persisted sparsely, by name, and restored", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Git", canUninstall: true }, { i: 1, name: "Nushell", canUninstall: true }]);
  assert.deepEqual(persistableScope(m), {}, "untouched → nothing written");
  setScope(m, 0, "in");
  setScope(m, 1, "out");
  assert.deepEqual(persistableScope(m), { Git: "in", Nushell: "out" });
  // Restore into a fresh model.
  const m2 = createModel();
  loadPlan(m2, [{ i: 0, name: "Git", canUninstall: true }, { i: 1, name: "Nushell", canUninstall: true }]);
  applySavedScope(m2, { scope: { Git: "in", Nushell: "out" } });
  assert.equal(scopeOf(m2, 0), "in");
  assert.equal(scopeOf(m2, 1), "out");
  // Garbage and unknown names are ignored, not fatal.
  applySavedScope(m2, { scope: { Ghost: "in", Git: "maybe" } });
  assert.equal(scopeOf(m2, 0), "in", "a bad value leaves the good one alone");
});

test("scope: unmanagedIndices lists exactly what the server must refuse", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Starship config", canUninstall: false, isConfig: true },
    { i: 1, name: "Nushell", canUninstall: true },
    { i: 2, name: "Git", canUninstall: true },
  ]);
  setExternal(m, 2, true);
  setStatusData(m, 2, "ok");
  assert.deepEqual(unmanagedIndices(m), [0, 2]);
  setScope(m, 2, "in"); // pulled back in → drops off the list
  assert.deepEqual(unmanagedIndices(m), [0]);
});

// --- touched: a gesture must leave a trace ----------------------------------
// C's report: a row that vanishes the moment you act on it is "bizarre et ennuyeux…
// surtout si c'est une erreur" — undoing a mistake destroyed the evidence you had
// undone it, because the Changes tab only shows actionable rows.

test("touched: a row is marked by any gesture, including a no-op one", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Miro", canUninstall: true }]);
  assert.equal(isTouched(m, 0), false, "untouched by default");
  setDecision(m, 0, "in");
  assert.equal(isTouched(m, 0), true);
  // Back to auto — the row is no longer actionable, but the trace REMAINS. This is
  // the whole point: correcting an error must not erase the correction.
  setDecision(m, 0, null);
  assert.equal(actionOf(m, 0), null, "no action left…");
  assert.equal(isTouched(m, 0), true, "…yet the row still testifies");
});

test("touched: a scope gesture leaves a trace too", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Nushell", canUninstall: true }]);
  setScope(m, 0, "out");
  assert.equal(isTouched(m, 0), true);
  setScope(m, 0, null); // back to the derivation
  assert.equal(isTouched(m, 0), true, "the trace survives the undo");
});

test("touched: NEVER persisted — it is interface history, not intent", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Miro", canUninstall: true }]);
  setDecision(m, 0, "in");
  setDecision(m, 0, null); // net effect: nothing
  assert.deepEqual(persistablePkgs(m), {}, "a no-op writes no intent");
  assert.deepEqual(persistableScope(m), {}, "…and no scope either");
  assert.equal(isTouched(m, 0), true, "but the session still remembers the gesture");
});

test("touched: a fresh plan clears the traces", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Miro", canUninstall: true }]);
  setDecision(m, 0, "in");
  assert.equal(isTouched(m, 0), true);
  loadPlan(m, [{ i: 0, name: "Miro", canUninstall: true }]);
  assert.equal(isTouched(m, 0), false, "a new plan is a new slate");
});

test("touched: Reset clears the traces — it is the 'forget my choices' gesture", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Miro", canUninstall: true }]);
  setDecision(m, 0, "in");
  assert.equal(isTouched(m, 0), true);
  clearAllDecisions(m);
  assert.equal(isTouched(m, 0), false, "witnesses to discarded choices go with them");
});

// --- the plan an Apply would run ---------------------------------------------
//
// applyPlan is the button's input. Until 2026-10-10 it was `rungPlan(model, rung)`, which kept
// only the actions a rung of the ladder admitted; the ladder left, and the nature of a row
// (⚡ / 🧩 / 📦) is displayed, never filtered on.

test("the three facts and `secs` arrive from the plan, defaulted false/0", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "AWS CLI", canUninstall: true, uac: true, forbidden: false, slow: true, secs: 242 },
    { i: 1, name: "rg", canUninstall: true }, // a plan row that says nothing
  ]);
  assert.deepEqual(
    { uac: m.pkgs.get(0).uac, forbidden: m.pkgs.get(0).forbidden, slow: m.pkgs.get(0).slow, secs: m.pkgs.get(0).secs },
    { uac: true, forbidden: false, slow: true, secs: 242 },
  );
  // An absent field is FALSE and 0, never undefined: the row labels read them directly, and
  // `0` is the "never measured here" sentinel the estimate must show as unknown.
  assert.deepEqual(
    { uac: m.pkgs.get(1).uac, forbidden: m.pkgs.get(1).forbidden, slow: m.pkgs.get(1).slow, secs: m.pkgs.get(1).secs },
    { uac: false, forbidden: false, slow: false, secs: 0 },
  );
});

test("applyPlan lists every actionable row, whatever its nature", () => {
  // ⚡, 🧩 and 📦 rows alike, and the elevating and the slow one too: the nature and the facts
  // are shown on the row, they no longer separate anything.
  const m = createModel();
  loadPlan(m, [
    // canUninstall:true everywhere: without a route back a row derives OUT of scope, which is
    // a different gate and gates first.
    { i: 0, name: "Starship config", canUninstall: true, isConfig: true },
    { i: 1, name: "Chiron", canUninstall: true, isExtension: true },
    { i: 2, name: "rg", canUninstall: true },
    { i: 3, name: "bat", canUninstall: true },
    { i: 4, name: "AWS CLI", canUninstall: true, uac: true },
    { i: 5, name: "Xcode CLT", canUninstall: true, slow: true },
  ]);
  for (const i of [0, 1, 2, 3, 4, 5]) setDecision(m, i, "in");
  for (const i of [3, 4, 5]) {
    setStatusData(m, i, "ok");
    setOutdated(m, i, true, "9.9.9");
  }
  assert.deepEqual([0, 1, 2, 3, 4, 5].map((i) => actionOf(m, i)),
    ["install", "install", "install", "upgrade", "upgrade", "upgrade"],
    "the fixture is what it claims");
  assert.deepEqual(applyPlan(m), [0, 1, 2, 3, 4, 5]);
  // Exactly the rows the diff view shows — the button and the panel cannot disagree.
  assert.deepEqual(applyPlan(m), [0, 1, 2, 3, 4, 5].filter((i) => isActionable(m, i)));
});

test("a RESTORE leaves no trace — only a gesture of this session does", () => {
  // ⭐ C, on the running app: "j'ai fait refresh et elle ne part pas". A Refresh reloads the plan,
  // `loadPlan` clears `touched` — and then `applySavedSelection` replayed every stored choice
  // through `setDecision`, which re-marked each one. So the trace looked PERMANENT and the Changes
  // tab kept showing rows where nothing would happen.
  //
  // ⚠️ The distinction is the whole fix: `touched` is INTERFACE HISTORY ("you did this, just now")
  // while `decision`/`scope` are INTENT ("you want this, still"). Replaying intent is not doing
  // something. The trace itself stays deliberate — a cancelled row that VANISHED was a real bug,
  // twice — so this narrows WHO marks, never whether marking happens.
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "AWS CLI", canUninstall: true },
    { i: 1, name: "rg", canUninstall: true },
  ]);
  // A real gesture marks.
  setDecision(m, 0, "in");
  assert.equal(isTouched(m, 0), true, "a click is a gesture: it leaves a trace");

  // A RESTORE does not — the shape a Refresh takes.
  const m2 = createModel();
  loadPlan(m2, [
    { i: 0, name: "AWS CLI", canUninstall: true },
    { i: 1, name: "rg", canUninstall: true },
  ]);
  applySavedSelection(m2, { pkgs: { "AWS CLI": "in", rg: "out" } });
  assert.equal(isTouched(m2, 0), false, "restoring a stored choice is not a gesture");
  assert.equal(isTouched(m2, 1), false);
  // …and the INTENT is restored all the same, which is the half that must not regress.
  assert.equal(toggleOf(m2, 0), "in", "the decision itself is restored");
  assert.equal(toggleOf(m2, 1), "out");

  // Same for scope, which had the identical defect through setScope.
  const m3 = createModel();
  loadPlan(m3, [{ i: 0, name: "AWS CLI", canUninstall: true }]);
  applySavedScope(m3, { scope: { "AWS CLI": "out" } });
  assert.equal(isTouched(m3, 0), false, "a restored scope override leaves no trace either");
  assert.equal(scopeOf(m3, 0), "out", "…while the override itself is restored");
});

test("applyPlan honours a removal, and an out-of-scope row is never in it", () => {
  // The facts on row 0 are kept deliberately: they do not move it.
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Miro", canUninstall: true, uac: true, forbidden: true, slow: true },
    // 1 — the Git hazard: present, unwanted, but installed outside our manager.
    { i: 1, name: "Git", canUninstall: true },
  ]);
  for (const i of [0, 1]) setStatusData(m, i, "ok");
  setExternal(m, 1, true);
  assert.equal(actionOf(m, 0), "uninstall");
  assert.equal(actionOf(m, 1), null, "scope gates first");
  assert.deepEqual(applyPlan(m), [0], "the ✕ is honoured — and the external row still is not touched");
});

test("a catalogue RELOAD replaces the plan — it does not accumulate", () => {
  // ⭐ Refresh now re-reads catalog/ and sends a SECOND `plan` message. Until then
  // loadPlan ran exactly once per session, so nothing had ever exercised what a second
  // one does — and "it happens to work" is not a property, it is an absence of evidence.
  //
  // What must hold: the new plan REPLACES the old one. A package removed from the
  // catalogue must be gone (not lingering with its last verdict), a package added must be
  // there, and the count must be the NEW count rather than the sum of both.
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "rg", canUninstall: true },
    { i: 1, name: "bat", canUninstall: true },
    { i: 2, name: "jq", canUninstall: true },
  ]);
  // The user decides something, and the machine answers — both kinds of state that could
  // survive a reload if the model merged instead of replacing.
  setDecision(m, 1, "in");
  setStatusData(m, 2, "ok");
  assert.equal(m.pkgs.size, 3);

  // The catalogue changes on disk: `bat` is gone, `fd` appears. Indices SHIFT, which is
  // exactly why the server discards its positional last_seen on a reload.
  loadPlan(m, [
    { i: 0, name: "rg", canUninstall: true },
    { i: 1, name: "jq", canUninstall: true },
    { i: 2, name: "fd", canUninstall: true },
  ]);
  assert.equal(m.pkgs.size, 3, "the new count, not 3 + 3");
  const names = [...m.pkgs.values()].map((p) => p.name).sort();
  assert.deepEqual(names, ["fd", "jq", "rg"], "bat is GONE and fd is present");
  // ⚠️ And index 1 is now `jq`, not `bat`. A merge would have left `bat` at 1 carrying the
  // decision made about it, so the next Apply would act on the wrong package — the exact
  // failure the server's discard of last_seen prevents on its side.
  assert.equal(m.pkgs.get(1).name, "jq");
});

test("a reload drops the traces of the previous plan", () => {
  // `touched` is what makes a cancelled or undone gesture stay VISIBLE ("every gesture
  // leaves a trace"). Across a RELOAD those traces are moot: they describe rows that may
  // no longer exist, and keeping them would show a trace for a package that is gone.
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "rg", canUninstall: true }]);
  setDecision(m, 0, "in");
  setDecision(m, 0, "auto"); // moved and put back → a trace, deliberately kept
  assert.ok(m.touched.size > 0, "precondition: the trace exists");
  loadPlan(m, [{ i: 0, name: "rg", canUninstall: true }]);
  assert.equal(m.touched.size, 0, "a new plan is a new session's worth of rows");
});

test("versionSummary: a pending row shows the gap, greyed, and says it is held", () => {
  // ⭐ The display ALREADY EXISTS for a pinned row whose scan found something newer
  // (model.js: `muted: true`). `pending` reuses it — no new channel, no new message. What
  // changes is that the row can say WHY it is not moving.
  const model = createModel();
  loadPlan(model, [{ i: 0, name: "Git", pin: "pending", canUninstall: true }]);
  setPresence(model, 0, true);
  setInstalledVersion(model, 0, "2.50.1");
  setOutdated(model, 0, true, "2.51.0");
  const v = versionSummary(model, 0);
  assert.equal(v.from, "2.50.1");
  assert.equal(v.to, "2.51.0", "the gap is SHOWN — a hold, not a blindfold");
  assert.equal(v.muted, true, "…and greyed, because Apply will not push it");
  assert.equal(v.held, true, "…and it says it is held, not merely 'available'");
});

test("versionSummary: a pending row with nothing newer shows one number", () => {
  const model = createModel();
  loadPlan(model, [{ i: 0, name: "Git", pin: "pending", canUninstall: true }]);
  setPresence(model, 0, true);
  setInstalledVersion(model, 0, "2.50.1");
  const v = versionSummary(model, 0);
  assert.equal(v.from, "2.50.1");
  assert.equal(v.to, null, "no arbitration is pending when there is no gap to arbitrate");
  assert.equal(v.held, false);
});

test("versionSummary: a pending row with a BACKWARDS gap shows one number, not held", () => {
  // ⚠️ MEASURED: installed 1.133.0, available 1.132.0 (brew's receipt lagging the disk).
  // The defect shipped as "1.133.0 → 1.132.0  arbitration" and counted in the tab.
  // Fix: only treat as held when available is GREATER than installed.
  const model = createModel();
  loadPlan(model, [{ i: 0, name: "VS Code", pin: "pending", canUninstall: true }]);
  setPresence(model, 0, true);
  setInstalledVersion(model, 0, "1.133.0");
  setOutdated(model, 0, true, "1.132.0"); // backwards — available < installed
  const v = versionSummary(model, 0);
  assert.equal(v.from, "1.133.0");
  assert.equal(v.to, null, "a backwards gap is NOT arbitration — show one number");
  assert.equal(v.muted, false, "not muted");
  assert.equal(v.held, false, "not held");
});

test("isPending: only a held row with a real gap counts", () => {
  // ⚠️ The COUNT in the tab must be the number of rows with something to decide, not the
  // number of packages carrying the keyword. 25 packages declare `pending`; if only 6 have a
  // newer version available, the queue is 6.
  const model = createModel();
  loadPlan(model, [
    { i: 0, name: "Git", pin: "pending", canUninstall: true },
    { i: 1, name: "Node.js", pin: "pending", canUninstall: true },
    { i: 2, name: "Nushell", pin: "0.113.1", canUninstall: true },
  ]);
  setPresence(model, 0, true);
  setInstalledVersion(model, 0, "2.50.1");
  setOutdated(model, 0, true, "2.51.0");
  setPresence(model, 1, true);
  setInstalledVersion(model, 1, "24.4.0"); // present, nothing newer
  setPresence(model, 2, true);
  setInstalledVersion(model, 2, "0.113.1");
  assert.equal(isPending(model, 0), true);
  assert.equal(isPending(model, 1), false, "held, but nothing to arbitrate");
  assert.equal(isPending(model, 2), false, "an exact pin is a decision already taken");
});

test("isPending: an absent pending row is not awaiting arbitration — it is awaiting install", () => {
  const model = createModel();
  loadPlan(model, [{ i: 0, name: "Git", pin: "pending", canUninstall: true }]);
  setPresence(model, 0, false);
  assert.equal(isPending(model, 0), false);
  // Adding outdated + available still does NOT make it pending — presence guards first.
  setOutdated(model, 0, true, "2.51.0");
  assert.equal(isPending(model, 0), false, "absent with a gap is still not pending");
});

// --- requirements as a SCOPE fact (not merely a label) ----------------------
// The measured symptom (2026-08-21, on a Mac): `VC++ runtime` read `?` with an
// `install` button, marked `tracked` + `auto`, and counted against Base (10/12) —
// for a Microsoft redistributable that can never apply there. The `?` was honest
// (winget does not exist on macOS, so presence is genuinely undeterminable); what
// was false is that the row stayed actionable and counted.

// A host package and a dependent, so the requirement can be made to hold or not.
// ⚠️ `Windows` carries NO route — `canUninstall: false` — because that is what a
// platform package IS: `detect: cmd /c ver` and nothing else, observed and never
// installed. A fixture giving it a route would make the interesting case unreachable.
function seedWithRequirement() {
  const m = createModel();
  loadPlan(
    m,
    [
      { i: 0, name: "Windows", bundle: "", posture: "opt-in", canUninstall: false },
      {
        i: 1,
        name: "VC++ runtime",
        bundle: "Base",
        posture: "opt-in",
        canUninstall: true,
        requires: ["Windows"],
      },
    ],
    [{ name: "Base", emoji: "📦", packages: ["VC++ runtime"] }],
  );
  return m;
}

test("requiresReason: absent and unwanted requirement → the engine's wording", () => {
  const m = seedWithRequirement();
  setPresence(m, 0, false); // Windows is not here
  assert.equal(requiresReason(m, 1), "requires Windows");
});

test("requiresReason: a PRESENT requirement satisfies it", () => {
  const m = seedWithRequirement();
  setPresence(m, 0, true);
  assert.equal(requiresReason(m, 1), null);
});

test("requiresReason: a requirement Talos is about to install does hold", () => {
  // A requirement being installed in the SAME Apply must not block its dependent —
  // otherwise a bundle could never lay down a package and its plumbing in one gesture.
  // What makes it hold is not the WISH but the reachability: wanted AND actionable.
  const m = createModel();
  loadPlan(
    m,
    [
      { i: 0, name: "Nushell", bundle: "", posture: "opt-in", canUninstall: true },
      { i: 1, name: "Polars", bundle: "", posture: "opt-in", canUninstall: true, requires: ["Nushell"] },
    ],
    [],
  );
  setPresence(m, 0, false);
  setDecision(m, 1, "in"); // wanting Polars pulls Nushell in, transitively
  assert.equal(desiredOf(m, 0), "present");
  assert.equal(requiresReason(m, 1), null, "absent but on its way in");
});

test("requiresReason: wanting a row does not let it satisfy its OWN requirement", () => {
  // ⭐ The trap that made the first version of this rule vacuous. `wantedNames` closes
  // over `requires:`, so wanting `VC++ runtime` pulls `Windows` in — and a rule reading
  // "is the requirement desired-present?" would answer YES for every wanted row,
  // which is every row a bundle pulls. The measured symptom would have survived the fix.
  //
  // So the clause is reachability, not desire: a requirement with no practicable route
  // will never be laid down, whatever anyone wants. `canUninstall` is the codebase's
  // existing proxy for "Talos will actually act on this row" (scope.js's `no-route`),
  // and it is the right one here for that reason rather than for its name.
  const m = seedWithRequirement();
  setPresence(m, 0, false);
  applyProfile(m, "Base"); // Base pulls VC++ runtime → which pulls Windows "in"
  assert.equal(desiredOf(m, 0), "present", "the pull really did reach Windows");
  assert.equal(requiresReason(m, 1), "requires Windows");
});

test("requiresReason: ON Windows the very same row is fine", () => {
  // ⭐ The half that proves the rule is not just "platform packages are always
  // blocked": presence comes FIRST and unconditionally, so the day `cmd /c ver`
  // succeeds the row behaves like any other — same YAML, same catalogue, different
  // machine, different answer. Without this the fix would break Windows to fix macOS.
  const m = seedWithRequirement();
  applyProfile(m, "Base");
  setPresence(m, 0, true); // `cmd /c ver` answered
  assert.equal(requiresReason(m, 1), null);
  assert.equal(scopeOf(m, 1), "in");
});

test("requiresReason: 'nobody wants it' is NOT 'it is about to vanish'", () => {
  // ⭐ THE subtle decision, pinned here because getting it wrong is silent and loud
  // at once. `deps::requires_reason` reasons over `will_be_present`, which VETOES on
  // wanted-off — correct for the Apply, where `off` IS the uninstall instruction.
  // Transposed to a derivation AT REST it would be a disaster: every package is
  // either wanted or not, so a plugin row nobody asked for would read
  // `requires Nushell` while nushell sits installed on the machine. That is noise on
  // the majority state — the lesson caught twice after the padlock.
  //
  // So scope asks a NARROWER question: is the requirement there, or on its way? A
  // present-but-unwanted requirement satisfies it. `deps::requires_blocked` is the
  // engine's half of the same reading, and the pair is deliberately two functions
  // rather than one with a flag.
  const m = seedWithRequirement();
  setPresence(m, 0, true); // installed, and no bundle pulls it
  assert.equal(desiredOf(m, 0), "absent", "the fixture really is unwanted");
  assert.equal(requiresReason(m, 1), null, "installed is installed");
});

test("requiresReason: a dangling requirement name says so, and does not crash", () => {
  // The catalogue is a directory of files: a `requires:` can name a package nobody
  // shipped. catalog.rs:164 chose not to fail loudly, so the row must say WHICH
  // name is missing rather than silently behave as if satisfied.
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "rg", bundle: "", posture: "opt-in", requires: ["Ghost"] }], []);
  assert.equal(requiresReason(m, 0), "requires Ghost (unknown)");
});

test("requiresReason: no requirements → nothing to say", () => {
  const m = seedWithRequirement();
  assert.equal(requiresReason(m, 0), null);
  assert.equal(requiresReason(m, 99), null, "an unknown index is not a crash");
});

test("an unmet requirement takes the row OUT of scope", () => {
  const m = seedWithRequirement();
  setPresence(m, 0, false);
  assert.equal(scopeOf(m, 1), "out");
  assert.equal(scopeReason(m, 1), "requires Windows");
  // And back in the moment the requirement holds — the derivation is live, never stored.
  setPresence(m, 0, true);
  assert.equal(scopeOf(m, 1), "in");
  assert.equal(scopeReason(m, 1), null);
});

test("an unmet requirement offers NO action — the install button dies", () => {
  // ⭐ THE point of routing this through scope rather than through the label:
  // `actionOf` gates on scope in ONE line, so isActionable, the row button, the
  // will-change class and the plan count all go quiet without another edit.
  const m = seedWithRequirement();
  setPresence(m, 0, false);
  setPresence(m, 1, false);
  applyProfile(m, "Base"); // Base wants it; the machine cannot have it
  assert.equal(desiredOf(m, 1), "present", "the desire is untouched — only the action dies");
  assert.equal(actionOf(m, 1), null);
  assert.equal(isActionable(m, 1), false);
});

test("an unmet requirement reaches the SERVER in the unmanaged list", () => {
  // A front-only guard is cosmetic (talos-scope-second-axis): the server builds the
  // plan, so it must be TOLD. `unmanagedIndices` is that wire.
  const m = seedWithRequirement();
  setPresence(m, 0, false);
  // 0 = `Windows` itself (no route → the OLD derivation already put it out); 1 = the
  // dependent, which only this change reaches. Both must cross, in catalogue order.
  assert.deepEqual(unmanagedIndices(m), [0, 1]);
});

test("profileProgress: an out-of-scope member is not counted as missing", () => {
  // The second half of the measured symptom — and NOT specific to requirements:
  // the counter never consulted scope at all, so an `external` or no-route member
  // already inflated the denominator. Base read 10/12 for two rows Apply would
  // never touch.
  const m = seedWithRequirement();
  applyProfile(m, "Base"); // pulls VC++ runtime in → it is desired-present
  setPresence(m, 0, true);
  assert.deepEqual(profileProgress(m, "Base"), { present: 0, total: 1 });
  setPresence(m, 0, false); // Windows gone → the member leaves the perimeter
  assert.deepEqual(
    profileProgress(m, "Base"),
    { present: 0, total: 0 },
    "a row that cannot be installed is not a goal",
  );
});

test("planCounts names the reinstalls apart — a downgrade is an uninstall first", () => {
  // Spec: the row and the button say "reinstall" BEFORE the click. A downgrade counted
  // under "to apply" would hide that the package is removed first.
  const m = seedPinned({ canDowngrade: true }); // pin 1.8
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.0");
  assert.deepEqual(planCounts(m), { applies: 0, reinstalls: 1, removes: 0, total: 1 });
  setInstalledVersion(m, 0, "1.5");
  assert.deepEqual(planCounts(m), { applies: 1, reinstalls: 0, removes: 0, total: 1 });
});

test("planSeconds: what each row the Apply would run took here, 0 when never measured", () => {
  // The button's estimate sums these. Only the rows the Apply runs count: a row at its pin
  // costs nothing, and its old duration must not inflate the total.
  const m = seedPinned({ secs: 42 }); // pin 1.8
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.8");
  assert.deepEqual(planSeconds(m), [], "at the pin: not in the plan, not in the estimate");
  setInstalledVersion(m, 0, "1.5");
  assert.deepEqual(planSeconds(m), [42]);
  m.pkgs.get(0).secs = 0;
  assert.deepEqual(planSeconds(m), [0], "never measured stays 0 — the estimate counts it unknown");
});
