// Tests for the pure decision logic. Run: node --test  (from talos/)
// No DOM, no server — just the rules. This replaces the ad-hoc /tmp scripts.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  actionFor,
  desiredState,
  effectiveToggle,
  isDeviation,
  isLockedPosture,
  postureDefault,
  profileState,
  resolvePosture,
  toggleState,
} from "../public/decision.js";

test("resolvePosture: pkg › bundle › mandatory", () => {
  assert.equal(resolvePosture("opt-in", "opt-out"), "opt-in"); // pkg wins
  assert.equal(resolvePosture(undefined, "opt-out"), "opt-out"); // inherits bundle
  assert.equal(resolvePosture(undefined, undefined), "mandatory"); // default
  assert.equal(resolvePosture("bogus", "nope"), "mandatory"); // invalid → default
});

test("postureDefault: which side the toggle starts on", () => {
  assert.equal(postureDefault("mandatory"), "in");
  assert.equal(postureDefault("opt-out"), "in");
  assert.equal(postureDefault("opt-in"), "out");
  assert.equal(postureDefault("forbidden"), "out");
});

test("isLockedPosture: only forbidden locks (nothing is indispensable, §4)", () => {
  assert.equal(isLockedPosture("forbidden"), true);
  assert.equal(isLockedPosture("mandatory"), false); // no longer locked-in
  assert.equal(isLockedPosture("opt-in"), false);
  assert.equal(isLockedPosture("opt-out"), false);
});

test("bundle-driven: untouched package is OUT regardless of posture", () => {
  // No active bundle (pulled=false), no manual toggle → out. Posture no longer
  // sets the default side (spec Consolidation §3-4).
  assert.equal(toggleState("opt-out", null, false), "out");
  assert.equal(toggleState("opt-in", null, false), "out");
  assert.equal(toggleState("mandatory", null, false), "out");
});

test("bundle-driven: an active-bundle pull makes it in", () => {
  assert.equal(toggleState("opt-in", null, true), "in"); // pulled by a bundle
  assert.equal(desiredState("opt-in", null, true), "present");
});

test("bundle-driven: manual toggle wins over the pull", () => {
  assert.equal(toggleState("opt-out", "out", true), "out"); // manual out beats pull
  assert.equal(toggleState("opt-in", "in", false), "in"); // manual in without pull
  assert.equal(desiredState("opt-in", "in", false), "present");
});

test("forbidden stays locked out even if pulled or manually set in", () => {
  assert.equal(toggleState("forbidden", "in", true), "out");
  assert.equal(desiredState("forbidden", "in", true), "absent");
});

test("isDeviation: true iff the user set a manual toggle (forbidden never)", () => {
  assert.equal(isDeviation("opt-out", null), false); // no manual → not deviated
  assert.equal(isDeviation("opt-out", "in"), true); // manual set
  assert.equal(isDeviation("opt-in", "out"), true); // manual set
  assert.equal(isDeviation("forbidden", "in"), false); // locked → never
});

test("actionFor: converge desired vs machine", () => {
  assert.equal(actionFor("present", { present: false }), "install");
  assert.equal(
    actionFor("present", { present: true, outdated: true }),
    "upgrade",
  );
  assert.equal(actionFor("present", { present: true, outdated: false }), null); // up to date → nothing
  assert.equal(
    actionFor("absent", { present: true, canUninstall: true }),
    "uninstall",
  );
  assert.equal(
    actionFor("absent", { present: true, canUninstall: false }),
    null,
  ); // can't remove
  assert.equal(actionFor("absent", { present: false }), null); // already gone
});

test("actionFor with a pin: below → upgrade, equal → nothing, above → downgrade", () => {
  // installed BELOW the pin → bring it up to the pin (Apply auto).
  assert.equal(
    actionFor("present", {
      present: true,
      pin: "1.8.0",
      installedVersion: "1.5.0",
    }),
    "upgrade",
  );
  // installed AT the pin → satisfied, nothing to do.
  assert.equal(
    actionFor("present", {
      present: true,
      pin: "1.8.0",
      installedVersion: "1.8.0",
    }),
    null,
  );
  // installed ABOVE the pin → downgrade (manual button, never Apply).
  assert.equal(
    actionFor("present", {
      present: true,
      pin: "1.8.0",
      installedVersion: "2.0.0",
    }),
    "downgrade",
  );
});

test("actionFor: a pin OVERRIDES the machine-wide outdated flag (pin is the reference)", () => {
  // outdated says "newer exists", but the pin says 1.8 is the truth and we're AT
  // it → nothing. Talos must not chase past the user's own pin.
  assert.equal(
    actionFor("present", {
      present: true,
      outdated: true,
      pin: "1.8.0",
      installedVersion: "1.8.0",
    }),
    null,
  );
});

test("actionFor: absent + pin → install (command carries the pin), absent still wins for removal", () => {
  assert.equal(
    actionFor("present", { present: false, pin: "1.8.0" }),
    "install",
  );
  // A pin never keeps a package the user turned off: absent + present → uninstall.
  assert.equal(
    actionFor("absent", {
      present: true,
      canUninstall: true,
      pin: "1.8.0",
      installedVersion: "2.0.0",
    }),
    "uninstall",
  );
});

test("effectiveToggle: manual toggle always wins over a profile pull", () => {
  assert.equal(effectiveToggle("out", true), "out"); // manual out beats profile → hollow
  assert.equal(effectiveToggle("in", false), "in"); // manual in stands
  assert.equal(effectiveToggle(null, true), "in"); // profile pulls it in
  assert.equal(effectiveToggle(null, false), null); // untouched → follow posture
});

test("profileState: off when not active", () => {
  const p = { packages: ["a", "b"] };
  assert.equal(profileState(p, false, () => true), "off");
});

test("profileState: full when active and all packages are in", () => {
  const p = { packages: ["a", "b"] };
  assert.equal(profileState(p, true, () => true), "full");
});

test("profileState: hollow when active but a package was pulled out", () => {
  const p = { packages: ["a", "b"] };
  const isIn = (k) => k !== "b"; // b manually out
  assert.equal(profileState(p, true, isIn), "hollow");
});

test("profileState: empty package list is trivially full when active", () => {
  assert.equal(profileState({ packages: [] }, true, () => false), "full");
});

test("model A: an opt-in left untouched but PRESENT resolves to uninstall", () => {
  const desired = desiredState("opt-in", null); // → absent
  assert.equal(desired, "absent");
  assert.equal(
    actionFor(desired, { present: true, canUninstall: true }),
    "uninstall",
  );
});
