import { assertEquals } from "@std/assert";
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
  setStatusData,
  toggleOf,
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

Deno.test("loadPlan wires packages into bundles", () => {
  const m = seed();
  assertEquals(m.pkgs.size, 3);
  assertEquals(m.bundles.get("Extras")?.pkgIds, [1, 2]);
  assertEquals(m.pkgs.get(1)?.key, "Extras::rg");
});

Deno.test("posture defaults: mandatory locked in, opt-in defaults out", () => {
  const m = seed();
  assertEquals(isLocked(m, 0), true); // mandatory
  assertEquals(toggleOf(m, 0), "in");
  assertEquals(isLocked(m, 1), false); // opt-in
  assertEquals(toggleOf(m, 1), "out"); // opt-in defaults out
  assertEquals(desiredOf(m, 1), "absent");
});

Deno.test("setDecision moves an opt-in in; locked package refuses", () => {
  const m = seed();
  assertEquals(setDecision(m, 1, "in"), true);
  assertEquals(toggleOf(m, 1), "in");
  assertEquals(isDeviated(m, 1), true);
  assertEquals(setDecision(m, 0, "out"), false); // mandatory can't move
  assertEquals(toggleOf(m, 0), "in");
});

Deno.test("actionOf: opt-in wanted+absent → install; mandatory present → null", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setStatusData(m, 1, "waiting"); // absent
  assertEquals(actionOf(m, 1), "install");
  assertEquals(isActionable(m, 1), true);
  setStatusData(m, 0, "ok"); // mandatory present, current
  assertEquals(actionOf(m, 0), null);
});

Deno.test("buttonAction always inverts machine state", () => {
  const m = seed();
  setStatusData(m, 1, "waiting"); // absent
  assertEquals(buttonAction(m, 1)?.type, "install");
  setStatusData(m, 1, "ok"); // present
  assertEquals(buttonAction(m, 1)?.type, "uninstall");
});

Deno.test("bundle toggle state: mixed when packages disagree", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setDecision(m, 2, "out");
  assertEquals(bundleToggleState(m, "Extras"), "mixed");
  setDecision(m, 2, "in");
  assertEquals(bundleToggleState(m, "Extras"), "on-in");
});

Deno.test("Base bundle is locked (not selectable)", () => {
  const m = seed();
  assertEquals(bundleLocked(m, "Base"), true);
  assertEquals(bundleLocked(m, "Extras"), false);
});

Deno.test("persist + restore selection by key", () => {
  const m = seed();
  setDecision(m, 1, "in");
  const saved = { pkgs: persistablePkgs(m) };
  assertEquals(saved.pkgs, { "Extras::rg": "in" });
  const m2 = seed();
  applySavedSelection(m2, saved);
  assertEquals(toggleOf(m2, 1), "in");
});

Deno.test("clearAllDecisions returns everything to posture defaults", () => {
  const m = seed();
  setDecision(m, 1, "in");
  clearAllDecisions(m);
  assertEquals(toggleOf(m, 1), "out"); // back to opt-in default
  assertEquals(m.decision.size, 0);
});

Deno.test("bundleAnyActionable + bundleAct reflect pending package actions", () => {
  const m = seed();
  setDecision(m, 1, "in");
  setStatusData(m, 1, "waiting"); // opt-in wanted but absent → install
  assertEquals(bundleAnyActionable(m, "Extras"), true);
  assertEquals(bundleAct(m, "Extras"), "add");
});

Deno.test("bundleToggleState: all-locked bundle defaults to on-in", () => {
  const m = seed();
  // Base holds only the mandatory Node (locked) → no free packages.
  assertEquals(bundleToggleState(m, "Base"), "on-in");
});

// --- profiles: additive pull, hollow on manual out, clean removal ------------
Deno.test("applyProfile: pulls its packages in (additive, no decision written)", () => {
  const m = seedWithProfiles();
  assertEquals(toggleOf(m, 1), "out"); // rg opt-in, default out
  applyProfile(m, "Search");
  assertEquals(toggleOf(m, 1), "in"); // pulled in by the profile
  assertEquals(desiredOf(m, 1), "present");
  assertEquals(m.decision.size, 0); // no manual decision written — purely additive
  assertEquals(profileStateOf(m, "Search"), "full");
});

Deno.test("profileStateOf: off when not active", () => {
  const m = seedWithProfiles();
  assertEquals(profileStateOf(m, "Search"), "off");
  assertEquals(profileStateOf(m, "All tools"), "off");
});

Deno.test("profileStateOf: hollow when a manual out overrides the pull", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools"); // pulls rg + bat in
  assertEquals(profileStateOf(m, "All tools"), "full");
  setDecision(m, 2, "out"); // user pulls bat back out
  assertEquals(toggleOf(m, 2), "out"); // manual out wins over the profile
  assertEquals(profileStateOf(m, "All tools"), "hollow");
});

Deno.test("removeProfile: pull vanishes, but a shared package survives", () => {
  const m = seedWithProfiles();
  applyProfile(m, "Search"); // rg
  applyProfile(m, "All tools"); // rg + bat
  assertEquals(toggleOf(m, 1), "in"); // rg pulled by both
  removeProfile(m, "All tools");
  assertEquals(toggleOf(m, 1), "in"); // rg survives — still pulled by Search
  assertEquals(toggleOf(m, 2), "out"); // bat drops — only All tools wanted it
  assertEquals(profileStateOf(m, "Search"), "full");
});

Deno.test("canReset: false at rest, true after a manual move or a profile", () => {
  const m = seedWithProfiles();
  assertEquals(canReset(m), false); // pristine
  setDecision(m, 1, "in");
  assertEquals(canReset(m), true); // moved a package
  clearAllDecisions(m);
  assertEquals(canReset(m), false);
  applyProfile(m, "Search");
  assertEquals(canReset(m), true); // active profile alone is enough
});

Deno.test("profilesForPkg: lists the profiles a package belongs to", () => {
  const m = seedWithProfiles();
  // rg is in both Search and All tools; bat only in All tools; Node in neither.
  assertEquals(profilesForPkg(m, 1).map((p) => p.name), [
    "Search",
    "All tools",
  ]);
  assertEquals(profilesForPkg(m, 2).map((p) => p.name), ["All tools"]);
  assertEquals(profilesForPkg(m, 0), []);
});

Deno.test("profileProgress: counts present packages over total (machine truth)", () => {
  const m = seedWithProfiles();
  // nothing present yet
  assertEquals(profileProgress(m, "All tools"), { present: 0, total: 2 });
  setStatusData(m, 1, "ok"); // rg present
  assertEquals(profileProgress(m, "All tools"), { present: 1, total: 2 });
  setStatusData(m, 2, "ok"); // bat present
  assertEquals(profileProgress(m, "All tools"), { present: 2, total: 2 });
});

Deno.test("profileProgress: unknown profile → 0/0", () => {
  const m = seedWithProfiles();
  assertEquals(profileProgress(m, "Nope"), { present: 0, total: 0 });
});

Deno.test("profileProgress: reflects presence, not intent", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools"); // intent: full — but nothing installed
  assertEquals(profileProgress(m, "All tools"), { present: 0, total: 2 });
});

Deno.test("clearAllDecisions also drops active profiles (true reset)", () => {
  const m = seedWithProfiles();
  applyProfile(m, "All tools");
  setDecision(m, 1, "out");
  clearAllDecisions(m);
  assertEquals(m.activeProfiles.size, 0);
  assertEquals(m.decision.size, 0);
  assertEquals(toggleOf(m, 1), "out"); // back to opt-in default
  assertEquals(profileStateOf(m, "All tools"), "off");
});

Deno.test("buttonAction: config-atom turned out → diff (not install)", () => {
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
  assertEquals(buttonAction(m, 0), { verb: "diff", dir: "", type: "diff" });
});

Deno.test("buttonAction: config-atom left in → install path unchanged", () => {
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
  // in + absent (not present) → install, unchanged behaviour
  assertEquals(buttonAction(m, 0)?.type, "install");
});
