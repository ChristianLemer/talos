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

test("scopeOf: an unmet requirement derives the row OUT", () => {
  // The measured symptom: a Microsoft redistributable on a Mac read `tracked` with an
  // `install` button, because scope knew nothing about requirements. A row whose
  // requirement will not hold cannot be installed NOR removed — the requirement is
  // usually the very tool that would perform the gesture — so it leaves the perimeter
  // in BOTH directions, exactly like the two derivations beside it.
  const blocked = { ...managed, requiresReason: "requires Windows" };
  assert.equal(scopeOf(blocked), "out");
  // Satisfied (or absent) → the fact says nothing and the row stays in scope.
  assert.equal(scopeOf({ ...managed, requiresReason: null }), "in");
  assert.equal(scopeOf(managed), "in", "the fact is optional — an absent one is not a veto");
});

test("scopeOf: the user can still pull a requirement-blocked row back in", () => {
  // Same shape as the other two derivations: derived out, manual "in" wins. The
  // gesture is futile here (the Apply will fail loudly), but scope is the user's
  // question to answer and a derivation that could not be overridden would be a lock.
  const blocked = { ...managed, requiresReason: "requires Windows" };
  assert.equal(scopeOf({ ...blocked, manual: "in" }), "in");
});

test("scopeReason: an unmet requirement gives the engine's own wording", () => {
  // ⭐ NOT a token from SCOPE_LABEL: the string carries the requirement's NAME, and it
  // is byte-for-byte what `deps::requires_reason` produces server-side. One wording,
  // two callers — so the row reads the same whether the verdict came from the scan
  // (here) or from the Apply (server.rs). A token would have forced a second
  // vocabulary for the same fact.
  assert.equal(
    scopeReason({ ...managed, requiresReason: "requires Windows" }),
    "requires Windows",
  );
  assert.equal(
    scopeReason({ ...managed, requiresReason: "requires Windows (unknown)" }),
    "requires Windows (unknown)",
  );
});

test("scopeReason: the requirement outranks every other derived reason", () => {
  // It names the actual blocker. `no-route` and `not-managed` describe the row's
  // shape; `requires Windows` says what is missing and therefore what would fix it.
  assert.equal(
    scopeReason({
      canUninstall: false, external: true, isConfig: true, pulled: true,
      manual: "out", requiresReason: "requires Windows",
    }),
    "requires Windows",
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
