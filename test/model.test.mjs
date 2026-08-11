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
  buttonAction,
  canReset,
  clearAllDecisions,
  createModel,
  desiredOf,
  isActionable,
  isDeviated,
  isInPersonal,
  isLocked,
  isProfileActive,
  isTouched,
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
  rungPlan,
  rungSeconds,
  scopeOf,
  setDecision,
  setExternal,
  setInstalledVersion,
  setOutdated,
  setScope,
  setStatusData,
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
      { i: 0, name: "Node", bundle: "Base", posture: "mandatory" },
      { i: 1, name: "rg", bundle: "Extras", posture: "opt-in" },
      { i: 2, name: "bat", bundle: "Extras", posture: "opt-in" },
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

test("buttonAction always inverts machine state", () => {
  const m = seed();
  setStatusData(m, 1, "waiting"); // absent
  assert.deepEqual(buttonAction(m, 1)?.type, "install");
  setStatusData(m, 1, "ok"); // present
  assert.deepEqual(buttonAction(m, 1)?.type, "uninstall");
});

test("buttonAction: an OUTDATED package the user turned OUT → uninstall, not update", () => {
  const m = seed(); // pkg 1 = opt-in rg, canUninstall
  setStatusData(m, 1, "ok"); // present
  setOutdated(m, 1, true, "9.9"); // and outdated
  setDecision(m, 1, "out"); // but the user wants it GONE
  // The desired state (absent) must win: you don't "update" something you're
  // removing. Button + plan agree → uninstall. (Was showing "update" — the
  // outdated branch fired before the desired-absent check.)
  assert.deepEqual(buttonAction(m, 1), {
    verb: "uninstall",
    dir: "remove",
    type: "uninstall",
  });
  assert.deepEqual(actionOf(m, 1), "uninstall"); // plan agrees
});

// --- version pinning at the model layer -------------------------------------
// A pin plan seeds one pinned package the user WANTS (setDecision in — nothing is
// wanted by default now), so the installed version drives the three-way action.
// See memory talos-version-pin.
function seedPinned() {
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

test("buttonAction: pinned + installed above pin → downgrade button", () => {
  const m = seedPinned();
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.0");
  assert.deepEqual(buttonAction(m, 0), {
    verb: "downgrade",
    dir: "remove",
    type: "downgrade",
  });
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
  });
});

test("versionSummary: pinned ABOVE → installed→pin, MUTED (downgrade is manual, never pushed)", () => {
  const m = seedPinned(); // pin 1.8
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.0");
  // A downgrade is manual-only (never in Apply), so its target reads muted —
  // same "not pushed" grey as an upgrade past a pin. Below the pin (a real
  // upgrade Apply runs) stays not-muted. The direction sign is what differs.
  assert.deepEqual(versionSummary(m, 0), {
    from: "2.0",
    to: "1.8",
    pinned: "to",
    muted: true,
  });
});

test("versionSummary: absent → empty (nothing installed to show)", () => {
  const m = seedPinned();
  setStatusData(m, 0, "waiting"); // absent
  assert.deepEqual(versionSummary(m, 0), {
    from: "",
    to: null,
    pinned: null,
    muted: false,
  });
});

test("isActionable: a downgrade is NOT in the auto plan (mirrors server AUTO_ACTS)", () => {
  const m = seedPinned();
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "2.0"); // above pin → downgrade
  // The button offers a downgrade, but Apply must NOT act — so the row is not
  // "in plan" and its Apply-preview stays neutral (no green/red tint).
  assert.deepEqual(buttonAction(m, 0)?.type, "downgrade");
  assert.deepEqual(isActionable(m, 0), false);
  // Below the pin (upgrade) IS in the plan.
  setInstalledVersion(m, 0, "1.5");
  assert.deepEqual(isActionable(m, 0), true);
});

test("buttonAction: pinned + installed below pin → update button", () => {
  const m = seedPinned();
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "1.5");
  assert.deepEqual(buttonAction(m, 0)?.type, "upgrade");
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

test("buttonAction: config-atom turned out → diff (not install)", () => {
  const m = createModel();
  loadPlan(m, [
    {
      i: 0,
      name: "Starship config",
      bundle: "Terminal",
      posture: "opt-out",
      isConfig: true,
    },
  ], []);
  setDecision(m, 0, "out");
  assert.deepEqual(buttonAction(m, 0), { verb: "diff", dir: "", type: "diff" });
});

test("buttonAction: config-atom left in → install path unchanged", () => {
  const m = createModel();
  loadPlan(m, [
    {
      i: 0,
      name: "Starship config",
      bundle: "Terminal",
      posture: "opt-out",
      isConfig: true,
    },
  ], []);
  setDecision(m, 0, "in"); // wanted (nothing-by-default → pull it in)
  // in + absent (not present) → install, unchanged behaviour
  assert.deepEqual(buttonAction(m, 0)?.type, "install");
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

// --- the ladder's per-rung count ---------------------------------------------
//
// rungPlan is the WIDGET's input, not the plan: the server filters (ladder.js's header
// says why). What it must never do is disagree with `isActionable` about which rows are
// candidates at all, or read the rung off the wrong row.

test("ladder: the three discriminants and `secs` arrive from the plan, defaulted false/0", () => {
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "AWS CLI", canUninstall: true, uac: true, forbidden: false, slow: true, secs: 242 },
    { i: 1, name: "rg", canUninstall: true }, // a plan row that says nothing
  ]);
  assert.deepEqual(
    { uac: m.pkgs.get(0).uac, forbidden: m.pkgs.get(0).forbidden, slow: m.pkgs.get(0).slow, secs: m.pkgs.get(0).secs },
    { uac: true, forbidden: false, slow: true, secs: 242 },
  );
  // An absent field is FALSE and 0, never undefined: rungAllows reads them directly, and
  // `0` is the "never measured here" sentinel the estimate must show as unknown.
  assert.deepEqual(
    { uac: m.pkgs.get(1).uac, forbidden: m.pkgs.get(1).forbidden, slow: m.pkgs.get(1).slow, secs: m.pkgs.get(1).secs },
    { uac: false, forbidden: false, slow: false, secs: 0 },
  );
});

test("ladder: rungPlan grows monotonically, and each row enters at ITS rung", () => {
  const m = createModel();
  loadPlan(m, [
    // 0 — a config-atom to install: rung 0. canUninstall:false would derive it OUT of
    // scope, so it is given a route back; scope is a different gate and gates first.
    { i: 0, name: "Starship config", canUninstall: true, isConfig: true },
    // 1 — an EXTENSION: rung 1. One row per rung, so the fixture demonstrates the whole scale.
    { i: 1, name: "Chiron", canUninstall: true, isExtension: true },
    // 2 — an app to install: rung 2
    { i: 2, name: "rg", canUninstall: true },
    // 3, 4, 5 — apps to UPGRADE, one per fact. ⭐ All three now land on the SAME rung: the
    // rungs that sorted by `uac` / `slow` are gone, and those facts are shown on the row
    // instead. Kept in the fixture precisely to prove they no longer separate anything.
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
  assert.deepEqual(rungPlan(m, 0), [0], "⚡ the config-atom alone");
  assert.deepEqual(rungPlan(m, 1), [0, 1], "🧩 adds the extension and NOT the app");
  assert.deepEqual(rungPlan(m, 2), [0, 1, 2, 3, 4, 5],
    "📦 adds every app at once — including the elevating and the slow one");
  // The top rung is exactly the rows a plain Apply would touch — the widget's count there
  // must equal what the diff view already shows, or the ladder contradicts the panel.
  assert.deepEqual(rungPlan(m, 2), [0, 1, 2, 3, 4, 5].filter((i) => isActionable(m, i)));
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

test("ladder: a removal rides its KIND, and an out-of-scope row is in no rung at all", () => {
  // ⭐ THIS TEST ASSERTED THE OPPOSITE, and the reversal is C's: a removal used to be in every
  // rung ("the ladder governs how far to GO; a ✕ is honoured or it is not"). That was right
  // while the axis was TIME. The axis is WHERE IT LANDS now, and removing an app touches the
  // machine — so ⚡ Config only, whose whole promise is "your own files only", cannot be the
  // rung that quietly uninstalls software.
  //
  // The facts on row 0 are kept deliberately: they no longer move it, which is the other half
  // of the reshape.
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Miro", canUninstall: true, uac: true, forbidden: true, slow: true },
    // 1 — the Git hazard: present, unwanted, but installed outside our manager.
    { i: 1, name: "Git", canUninstall: true },
  ]);
  for (const i of [0, 1]) setStatusData(m, i, "ok");
  setExternal(m, 1, true);
  assert.equal(actionOf(m, 0), "uninstall");
  assert.equal(actionOf(m, 1), null, "scope gates before the rung does");
  assert.deepEqual(rungPlan(m, 0), [], "⚡ removes nothing from the machine");
  assert.deepEqual(rungPlan(m, 1), [], "🧩 neither — Miro is an app, not an extension");
  assert.deepEqual(rungPlan(m, 2), [0], "📦 honours the ✕ — and the external row still does not");
});

test("ladder: a downgrade is in NO rung — it mirrors AUTO_ACTS, not the ladder", () => {
  const m = createModel();
  loadPlan(m, [{ i: 0, name: "Nushell", canUninstall: true, pin: "0.113.1" }]);
  setDecision(m, 0, "in");
  setStatusData(m, 0, "ok");
  setInstalledVersion(m, 0, "0.114.0"); // above the pin → downgrade, manual only
  assert.equal(actionOf(m, 0), "downgrade");
  assert.equal(isActionable(m, 0), false);
  for (const r of [0, 1, 2]) assert.deepEqual(rungPlan(m, r), [], `rung ${r}`);
});

test("ladder: rungSeconds reads the LOCAL secs of exactly the rung's rows", () => {
  // Two things this must not do, and both would be invisible on screen:
  //
  // 1. Read the SHARED classification. `slow` is a fleet-wide max reduced to a boolean, so
  //    minutes derived from it could only be an invented constant — and a row is legitimately
  //    `slow: true, secs: 1` when this machine's cache is warm (timings.rs, measured on 7-Zip).
  // 2. Describe a different set of rows than the count beside it. It goes through rungPlan
  //    for exactly that reason.
  const m = createModel();
  loadPlan(m, [
    { i: 0, name: "Starship config", canUninstall: true, isConfig: true, secs: 3 },
    // ⭐ An extension with NO measurement — which is the real situation, not a convenience:
    // nothing has ever timed a plugin or skill route (timings.yaml holds five brew entries),
    // so rung 1 legitimately reads "unknown" and must COUNT the row rather than drop it.
    { i: 1, name: "Chiron", canUninstall: true, isExtension: true },
    { i: 2, name: "rg", canUninstall: true, secs: 12 },
    { i: 3, name: "bat", canUninstall: true }, // never measured here → 0, the unknown sentinel
    { i: 4, name: "AWS CLI", canUninstall: true, uac: true, secs: 242 },
    // slow for EVERYONE (last rung), and one second HERE. Not a contradiction: the shared
    // file classifies, the local file estimates.
    { i: 5, name: "7-Zip", canUninstall: true, slow: true, secs: 1 },
  ]);
  for (const i of [0, 1, 2, 3, 4, 5]) setDecision(m, i, "in");
  for (const i of [3, 4, 5]) {
    setStatusData(m, i, "ok");
    setOutdated(m, i, true, "9.9.9");
  }
  assert.deepEqual(rungSeconds(m, 0), [3]);
  assert.deepEqual(rungSeconds(m, 1), [3, 0], "the extension is counted, at 0 = unknown");
  assert.deepEqual(
    rungSeconds(m, 2),
    [3, 0, 12, 0, 242, 1],
    "📦 carries every app's seconds, unmeasured rows present as 0 rather than dropped — and " +
      "the slow-for-everyone row is 1s HERE, which is not a contradiction: the shared file " +
      "classifies, the local one estimates",
  );
  // One value per row the rung touches, always — a length mismatch is how "N items" and the
  // minutes beside it would come to describe different sets.
  for (const r of [0, 1, 2]) {
    assert.equal(rungSeconds(m, r).length, rungPlan(m, r).length, `rung ${r}`);
  }
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
