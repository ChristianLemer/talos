// Ported from the Deno-era test/model.test.ts to node:test (the frontend tests
// were not carried through the Tauri/Rust port; this restores model.js coverage).
// Run: node --test test/model.test.mjs   (from talos/). Pure — no DOM, no server.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  actionOf,
  applyProfile,
  applySavedSelection,
  bundleAct,
  bundleAnyActionable,
  bundleLocked,
  bundleToggleState,
  buttonAction,
  canReset,
  clearAllDecisions,
  createModel,
  desiredOf,
  isActionable,
  isDeviated,
  isLocked,
  loadPlan,
  persistablePkgs,
  profileProgress,
  profilesForPkg,
  profileStateOf,
  removeProfile,
  setDecision,
  setInstalledVersion,
  setOutdated,
  setStatusData,
  toggleOf,
  versionSummary,
} from "../public/model.js";

// A plan WITH profiles: reuses the opt-in rg/bat fixture, adds two profiles.
function seedWithProfiles() {
  const m = createModel();
  loadPlan(
    m,
    [
      { name: "Base", posture: "mandatory", selectable: false },
      { name: "Extras", posture: "opt-in", selectable: true },
    ],
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
      { name: "Base", posture: "mandatory", selectable: false },
      { name: "Extras", posture: "opt-in", selectable: true },
    ],
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

test("loadPlan wires packages into bundles", () => {
  const m = seed();
  assert.deepEqual(m.pkgs.size, 3);
  assert.deepEqual(m.bundles.get("Extras")?.pkgIds, [1, 2]);
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
    [{ name: "Base", posture: "mandatory", selectable: false }],
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

test("bundle toggle state: mixed when packages disagree", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setDecision(m, 2, "out");
  assert.deepEqual(bundleToggleState(m, "Extras"), "mixed");
  setDecision(m, 2, "in");
  assert.deepEqual(bundleToggleState(m, "Extras"), "on-in");
});

test("Base bundle is locked (not selectable)", () => {
  const m = seed();
  assert.deepEqual(bundleLocked(m, "Base"), true);
  assert.deepEqual(bundleLocked(m, "Extras"), false);
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

test("bundleAnyActionable + bundleAct reflect pending package actions", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setStatusData(m, 1, "waiting"); // opt-in wanted but absent → install
  assert.deepEqual(bundleAnyActionable(m, "Extras"), true);
  assert.deepEqual(bundleAct(m, "Extras"), "add");
});

test("bundleToggleState: all-locked (forbidden) bundle defaults to on-in", () => {
  // Only `forbidden` locks now (§4). Build a bundle whose sole package is
  // forbidden → no free packages → the toggle defaults to on-in.
  const m = createModel();
  loadPlan(
    m,
    [{ name: "Banned", posture: "forbidden", selectable: false }],
    [{ i: 0, name: "nope", bundle: "Banned", posture: "forbidden" }],
  );
  assert.deepEqual(bundleToggleState(m, "Banned"), "on-in");
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

test("profileProgress: counts present packages over total (machine truth)", () => {
  const m = seedWithProfiles();
  // nothing present yet
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

test("profileProgress: reflects presence, not intent", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools"); // intent: full — but nothing installed
  assert.deepEqual(profileProgress(m, "All tools"), { present: 0, total: 2 });
});

test("clearAllDecisions also drops active profiles (true reset)", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools");
  setDecision(m, 1, "out");
  clearAllDecisions(m);
  assert.deepEqual(m.activeProfiles.size, 0);
  assert.deepEqual(m.decision.size, 0);
  assert.deepEqual(toggleOf(m, 1), "out"); // back to opt-in default
  assert.deepEqual(profileStateOf(m, "All tools"), "off");
});

test("buttonAction: config-atom turned out → diff (not install)", () => {
  const m = createModel();
  loadPlan(m, [{ name: "Terminal" }], [
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
  loadPlan(m, [{ name: "Terminal" }], [
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
