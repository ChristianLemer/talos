// Tests for the pure decision logic. Run: node --test  (from talos/)
// No DOM, no server — just the rules. This replaces the ad-hoc /tmp scripts.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  actionFor,
  desiredState,
  effectiveToggle,
  forbiddenMessage,
  isDeviation,
  isLockedPosture,
  postureDefault,
  profileState,
  toggleState,
} from "../public/decision.js";

test("403 recovery: Retry while paused RETRIES — it does not just continue", () => {
  // The bug this pins: the button labelled "Retry" (and the modal step that says
  // "Talos downloads it for you") used to send `forbidden-continue`, which moves
  // the Apply on to the NEXT package and leaves this row `forbidden`. The word lied.
  assert.equal(forbiddenMessage("retry", true), "forbidden-retry");
});

test("403 recovery: three gestures, three distinct messages — no synonyms", () => {
  assert.equal(forbiddenMessage("continue", true), "forbidden-continue");
  // `stop` was unreachable from the UI: the server implements "abandon the rest of
  // the plan" but nothing ever sent it. A third outcome needs a third control.
  assert.equal(forbiddenMessage("stop", true), "forbidden-stop");
});

test("403 recovery: not paused → no loop to answer (caller uses retry-step)", () => {
  // Retrying a row when the Apply is NOT holding is a plain row action; sending a
  // `forbidden-*` decision into a server that isn't waiting would be ignored.
  assert.equal(forbiddenMessage("retry", false), null);
  assert.equal(forbiddenMessage("stop", false), null);
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

// State is DERIVED from the packages, not from the active flag (a bundle whose
// members are all wanted — even via manual picks — reads full). The `active`
// arg is ignored by profileState now; passed as false to prove it doesn't gate.
test("profileState: off when NONE of its packages are in", () => {
  const p = { packages: ["a", "b"] };
  assert.equal(profileState(p, false, () => false), "off");
});

test("profileState: full when all packages are in (even if 'inactive')", () => {
  const p = { packages: ["a", "b"] };
  assert.equal(profileState(p, false, () => true), "full");
});

test("profileState: hollow when only some packages are in", () => {
  const p = { packages: ["a", "b"] };
  const isIn = (k) => k !== "b"; // b out, a in
  assert.equal(profileState(p, false, isIn), "hollow");
});

test("profileState: empty package list → off (nothing to want)", () => {
  assert.equal(profileState({ packages: [] }, true, () => false), "off");
});

test("model A: an opt-in left untouched but PRESENT resolves to uninstall", () => {
  const desired = desiredState("opt-in", null); // → absent
  assert.equal(desired, "absent");
  assert.equal(
    actionFor(desired, { present: true, canUninstall: true }),
    "uninstall",
  );
});
