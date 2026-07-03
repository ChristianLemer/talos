// Tests for the pure decision logic. Run: node --test  (from talos/)
// No DOM, no server — just the rules. This replaces the ad-hoc /tmp scripts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { resolvePosture, postureDefault, isLockedPosture, toggleState, desiredState, isDeviation, actionFor } from "../public/decision.js";

test("resolvePosture: pkg › bundle › mandatory", () => {
  assert.equal(resolvePosture("opt-in", "opt-out"), "opt-in");   // pkg wins
  assert.equal(resolvePosture(undefined, "opt-out"), "opt-out"); // inherits bundle
  assert.equal(resolvePosture(undefined, undefined), "mandatory"); // default
  assert.equal(resolvePosture("bogus", "nope"), "mandatory");    // invalid → default
});

test("postureDefault: which side the toggle starts on", () => {
  assert.equal(postureDefault("mandatory"), "in");
  assert.equal(postureDefault("opt-out"), "in");
  assert.equal(postureDefault("opt-in"), "out");
  assert.equal(postureDefault("forbidden"), "out");
});

test("isLockedPosture", () => {
  assert.equal(isLockedPosture("mandatory"), true);
  assert.equal(isLockedPosture("forbidden"), true);
  assert.equal(isLockedPosture("opt-in"), false);
  assert.equal(isLockedPosture("opt-out"), false);
});

test("toggleState: locked postures ignore the user toggle", () => {
  assert.equal(toggleState("mandatory", "out"), "in");   // can't flip a mandatory out
  assert.equal(toggleState("forbidden", "in"), "out");   // can't flip a forbidden in
});

test("toggleState: free postures follow the user, else the default", () => {
  assert.equal(toggleState("opt-out", null), "in");   // untouched → default in
  assert.equal(toggleState("opt-out", "out"), "out"); // user flipped
  assert.equal(toggleState("opt-in", null), "out");   // untouched → default out
  assert.equal(toggleState("opt-in", "in"), "in");    // user flipped
});

test("desiredState: in→present, out→absent", () => {
  assert.equal(desiredState("mandatory", null), "present");
  assert.equal(desiredState("forbidden", null), "absent");
  assert.equal(desiredState("opt-out", null), "present");
  assert.equal(desiredState("opt-out", "out"), "absent");
  assert.equal(desiredState("opt-in", null), "absent");
  assert.equal(desiredState("opt-in", "in"), "present");
});

test("isDeviation: true only when the user moved off the default", () => {
  assert.equal(isDeviation("opt-out", null), false);  // at default
  assert.equal(isDeviation("opt-out", "in"), false);  // same as default
  assert.equal(isDeviation("opt-out", "out"), true);  // moved
  assert.equal(isDeviation("opt-in", null), false);
  assert.equal(isDeviation("opt-in", "in"), true);    // moved
  assert.equal(isDeviation("mandatory", "out"), false); // locked → never a deviation
  assert.equal(isDeviation("forbidden", "in"), false);
});

test("actionFor: converge desired vs machine", () => {
  assert.equal(actionFor("present", { present: false }), "install");
  assert.equal(actionFor("present", { present: true, outdated: true }), "upgrade");
  assert.equal(actionFor("present", { present: true, outdated: false }), null); // up to date → nothing
  assert.equal(actionFor("absent", { present: true, canUninstall: true }), "uninstall");
  assert.equal(actionFor("absent", { present: true, canUninstall: false }), null); // can't remove
  assert.equal(actionFor("absent", { present: false }), null); // already gone
});

test("model A: an opt-in left untouched but PRESENT resolves to uninstall", () => {
  const desired = desiredState("opt-in", null);   // → absent
  assert.equal(desired, "absent");
  assert.equal(actionFor(desired, { present: true, canUninstall: true }), "uninstall");
});
