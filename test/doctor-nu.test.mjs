// Structural guards on admin/doctor's PowerShell probes. Run: node --test (from talos/)
//
// Odd home for a nushell guard, and deliberate: this repo has no nu test harness,
// while `node --test` already runs in CI and this defect is STRUCTURAL — it lives
// in how a string is built, which reading the source as text catches exactly.
// Same technique as veil.test.mjs and server.rs's own text-level guard.
//
// THE BUG THIS PINS (field report, 2026-08-09, Windows). `path-view` built its
// probe with nu interpolation:
//
//     $"($refresh) (Get-Command git -ErrorAction SilentlyContinue).Source"
//
// A PowerShell snippet is full of parentheses, and nu reads `(Get-Command …)`
// inside `$"..."` as ITS OWN subexpression: it tries to run Get-Command locally,
// throws, and the `catch { "" }` hands back an empty string — which then renders
// as `found: false`. So the gesture written to detect "Talos cannot see git"
// reported exactly that, on a machine where nothing was wrong with the PATH.
//
// A diagnostic that manufactures its own finding is worse than no diagnostic: it
// sends the reader hunting a defect that is in the instrument.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const nu = readFileSync(new URL("../admin/doctor/mod.nu", import.meta.url), "utf8");

// Lines that hand a snippet to powershell, minus comments — a `#` line may quote
// the broken form on purpose to explain it.
function codeLines() {
  return nu.split("\n").filter((l) => !l.trim().startsWith("#"));
}

test("no PowerShell snippet is built with nu string interpolation", () => {
  // `$"` anywhere on a line that also carries a PowerShell parenthesised call.
  const offenders = codeLines().filter(
    (l) => l.includes('$"') && /\((Get-Command|\[Environment\])/.test(l),
  );
  assert.deepEqual(
    offenders,
    [],
    'PowerShell snippets must be CONCATENATED (" + var + "), never interpolated:\n' +
      "nu evaluates the parentheses as its own subexpression and the probe returns\n" +
      "an empty string, which reads as 'not found' — a false diagnosis.",
  );
});

test("the rebuild snippet is READ from platform.rs, not retyped", () => {
  // A diagnostic that carries its own copy of what it diagnoses drifts silently
  // the day the engine changes, and then answers confidently about a Talos that
  // no longer exists. Same rule this module already applies to package ids.
  assert.ok(nu.includes("def win-path-refresh"), "win-path-refresh must exist");
  const body = nu.slice(nu.indexOf("def win-path-refresh"), nu.indexOf("export def wrapping"));
  assert.ok(
    body.includes('"platform.rs"'),
    "win-path-refresh must read src/platform.rs rather than hardcode the snippet",
  );
  // A fallback is fine — a deployed kit has no checkout — but it must not be the
  // only path, which the platform.rs read above already proves.
  assert.ok(
    body.includes("path exists"),
    "it must check the source exists before reading, and fall back explicitly",
  );
});

test("wrapping reports whether the engine assigns or appends", () => {
  // This is the hypothesis itself, made falsifiable: `keeps_live_path: false` is
  // the defect. If someone fixes platform.rs, this row must flip on its own.
  const body = nu.slice(nu.indexOf("export def wrapping"), nu.indexOf("export def marketplace-add-dry"));
  for (const field of ["source", "assigns_not_appends", "keeps_live_path"]) {
    assert.ok(body.includes(`${field}:`), `wrapping must report '${field}'`);
  }
});

test("marketplace-add-dry judges on the exit code, not on output", () => {
  // `git clone --help` prints a wall of text whether or not git was found, so
  // "was there output" is not a verdict. Only exit 0 means it worked.
  const start = nu.indexOf("export def marketplace-add-dry");
  assert.notEqual(start, -1, "marketplace-add-dry must exist");
  const body = nu.slice(start, nu.indexOf("def ps-probe"));
  assert.ok(
    body.includes("$r.code == 0"),
    "the verdict must come from the exit code, not from a non-empty stdout",
  );
  // It must compare wrapped against unwrapped, or a failure implicates nothing.
  assert.ok(body.includes("talos wrap") && body.includes("no wrap"), "both rows are required");
});

test("the PATH rebuild snippet is a plain string literal", () => {
  // It must be assignable verbatim, because its whole purpose is to reproduce
  // platform.rs's WIN_PATH_REFRESH byte for byte.
  const refresh = codeLines().filter((l) => l.includes("GetEnvironmentVariable('Path','Machine')"));
  assert.ok(refresh.length > 0, "the rebuild snippet must exist");
  for (const l of refresh) {
    assert.ok(
      !l.includes('$"'),
      `the rebuild snippet must not be interpolated: ${l.trim()}`,
    );
  }
});

test("a probe that cannot run reports a reason, never a bare 'not found'", () => {
  // The load-bearing distinction: `found: false` means "the exe is not on that
  // PATH", and it must be impossible to reach from a probe that failed to launch.
  assert.ok(nu.includes("def ps-probe"), "ps-probe must exist");
  const start = nu.indexOf("def ps-probe");
  const body = nu.slice(start, start + 1400);
  assert.ok(
    body.includes("note:"),
    "ps-probe must carry a `note` field so a broken probe can say so",
  );
  assert.ok(
    /probe failed|did not run/.test(body),
    "ps-probe must distinguish a failed probe from an absent executable",
  );
});

test("path-view keeps its control row", () => {
  // Three rows or the table proves nothing: interactive, powershell WITHOUT the
  // rebuild (the control), and powershell WITH it. Drop the control and
  // 'wrapped is false' no longer implicates the rebuild.
  const start = nu.indexOf("export def path-view");
  assert.notEqual(start, -1, "path-view must exist");
  const body = nu.slice(start, nu.indexOf("def ps-probe"));
  for (const row of ["interactive (nu)", "powershell -NoProfile", "talos_wrap"]) {
    assert.ok(body.includes(row), `path-view must report the '${row}' row`);
  }
});
