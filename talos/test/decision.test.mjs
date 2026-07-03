// Tests for the pure decision logic. Run: node --test  (from talos/)
// No DOM, no server — just the rules. This replaces the ad-hoc /tmp scripts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { resolvePosture, desiredState, actionFor, isLockedPosture } from "../public/decision.js";

test("resolvePosture: pkg › bundle › mandatory", () => {
  assert.equal(resolvePosture("opt-in", "opt-out"), "opt-in");   // pkg wins
  assert.equal(resolvePosture(undefined, "opt-out"), "opt-out"); // inherits bundle
  assert.equal(resolvePosture(undefined, undefined), "mandatory"); // default
  assert.equal(resolvePosture("bogus", "nope"), "mandatory");    // invalid → default
});

test("desiredState: mandatory/forbidden ignore the override", () => {
  assert.equal(desiredState("mandatory", "off"), "present");
  assert.equal(desiredState("mandatory", "auto"), "present");
  assert.equal(desiredState("forbidden", "on"), "absent");
  assert.equal(desiredState("forbidden", "auto"), "absent");
});

test("desiredState: opt-out present by default, removable", () => {
  assert.equal(desiredState("opt-out", "auto"), "present");
  assert.equal(desiredState("opt-out", "off"), "absent");
  assert.equal(desiredState("opt-out", "on"), "present");
});

test("desiredState: opt-in absent by default, addable", () => {
  assert.equal(desiredState("opt-in", "auto"), "absent");
  assert.equal(desiredState("opt-in", "on"), "present");
  assert.equal(desiredState("opt-in", "off"), "absent");
});

test("actionFor: converge desired vs machine", () => {
  assert.equal(actionFor("present", { present: false }), "install");
  assert.equal(actionFor("present", { present: true, outdated: true }), "upgrade");
  assert.equal(actionFor("present", { present: true, outdated: false }), null); // up to date → nothing
  assert.equal(actionFor("absent", { present: true, canUninstall: true }), "uninstall");
  assert.equal(actionFor("absent", { present: true, canUninstall: false }), null); // can't remove
  assert.equal(actionFor("absent", { present: false }), null); // already gone
});

test("model A: an opt-in left auto but PRESENT resolves to uninstall", () => {
  const desired = desiredState("opt-in", "auto");            // → absent
  assert.equal(desired, "absent");
  assert.equal(actionFor(desired, { present: true, canUninstall: true }), "uninstall");
});

test("isLockedPosture", () => {
  assert.equal(isLockedPosture("mandatory"), true);
  assert.equal(isLockedPosture("forbidden"), true);
  assert.equal(isLockedPosture("opt-in"), false);
  assert.equal(isLockedPosture("opt-out"), false);
});
