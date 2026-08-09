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
