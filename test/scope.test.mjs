// Pure — no DOM, no server, no model. The scope rule is a function of facts.
// Run: node --test test/scope.test.mjs   (from talos/)
import { test } from "node:test";
import assert from "node:assert/strict";
import { scopeOf, scopeReason } from "../public/scope.js";

// A row Talos can fully manage: it has a way back, and we installed it.
const managed = { canUninstall: true, external: false, isConfig: false, pulled: true };

test("scopeOf: the truth table, exhaustively", () => {
  const cases = [
    // canUninstall, external, manual        → expected
    [true, false, null, "in", "a normal row is in scope"],
    [false, false, null, "out", "no way back → derived out"],
    [true, true, null, "out", "installed elsewhere → derived out"],
    [false, false, "in", "in", "user pulled a no-way-back row in (the warned direction)"],
    [true, true, "in", "in", "user pulled an external row in (the warned direction)"],
    [true, false, "out", "out", "user pushed a normal row out"],
    [false, false, "out", "out", "derived out and confirmed by hand"],
    [true, true, "out", "out", "derived out and confirmed by hand"],
  ];
  for (const [canUninstall, external, manual, expected, why] of cases) {
    assert.equal(
      scopeOf({ canUninstall, external, isConfig: false, pulled: true, manual }),
      expected,
      why,
    );
  }
});

test("scopeOf: a garbage manual value falls back to the derivation", () => {
  // Defensive: selection.json is user-editable and can carry anything.
  assert.equal(scopeOf({ ...managed, manual: "maybe" }), "in");
  assert.equal(scopeOf({ ...managed, canUninstall: false, manual: 42 }), "out");
});

test("scopeReason: the label discriminator", () => {
  // A config-atom a bundle pulls: the user owns that file.
  assert.equal(
    scopeReason({ canUninstall: false, external: false, isConfig: true, pulled: true }),
    "yours",
  );
  // A config-atom nobody pulls (Bun PATH): in the catalogue, not requested.
  assert.equal(
    scopeReason({ canUninstall: false, external: false, isConfig: true, pulled: false }),
    "available",
  );
  // Present, installed outside our manager (Git via Xcode CLT).
  assert.equal(
    scopeReason({ canUninstall: true, external: true, isConfig: false, pulled: true }),
    "external",
  );
  // No route on this platform (winget-only on a Mac) — NOT a config-atom.
  assert.equal(
    scopeReason({ canUninstall: false, external: false, isConfig: false, pulled: true }),
    "no-route",
  );
  // In scope → no reason to give.
  assert.equal(scopeReason(managed), null);
});

test("scopeReason: a user-pushed-out row says so, and outranks nothing", () => {
  // Manual "out" on a row that IS manageable: the reason is the user's own hand.
  assert.equal(scopeReason({ ...managed, manual: "out" }), "not-managed");
  // But a DERIVED reason wins over the manual one — the row's nature is the
  // more informative fact ("external" tells you why, "not managed" does not).
  assert.equal(
    scopeReason({ canUninstall: true, external: true, isConfig: false, pulled: true, manual: "out" }),
    "external",
  );
});

test("scopeOf is platform-dependent by construction", () => {
  // Notepad++ declares only `winget:`. On macOS commands_for returns no commands
  // → canUninstall false → derived out. On Windows it has a route → in scope.
  // Same row, same YAML, different answer — the fixture that would catch a
  // porting regression (talos-windows-porting-regressions).
  const onMac = { canUninstall: false, external: false, isConfig: false, pulled: true };
  const onWindows = { ...onMac, canUninstall: true };
  assert.equal(scopeOf(onMac), "out");
  assert.equal(scopeOf(onWindows), "in");
  assert.equal(scopeReason(onMac), "no-route");
});
