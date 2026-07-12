// Tests for presence-probe BUILDING (pure) — both natures. The IO (detectPresent)
// is exercised live on Mac below.
import { assertEquals } from "@std/assert";
import {
  checkProbe,
  detectPresent,
  detectPresentDetailed,
  listProbe,
  presenceProbe,
  routeProbe,
  versionFrom,
} from "../src/detect.ts";
import type { Step } from "../src/bundles.ts";

// Minimal Step factory — only the fields detection reads.
function step(partial: Partial<Step>): Step {
  return {
    bundle: "T",
    name: partial.name ?? "x",
    description: "",
    install: null,
    uninstall: null,
    upgrade: null,
    route: partial.route ?? null,
    wingetId: partial.wingetId ?? null,
    systemId: partial.systemId ?? null,
    detect: partial.detect ?? null,
    check: partial.check ?? null,
    versionRegex: partial.versionRegex ?? null,
    requires: partial.requires ?? [],
    posture: "opt-in",
  };
}

// --- nature 1: RUN the author's detect command (presence + version) ---
Deno.test("presenceProbe: POSIX runs the full detect command", () => {
  assertEquals(presenceProbe("node --version", false), {
    cmd: "/bin/sh",
    args: ["-c", "node --version"],
  });
});

Deno.test("presenceProbe: Windows refreshes PATH then runs the command", () => {
  const p = presenceProbe("git --version", true);
  assertEquals(p?.cmd, "powershell.exe");
  const s = p?.args[2] ?? "";
  assertEquals(s.includes("GetEnvironmentVariable('Path','Machine')"), true);
  assertEquals(s.includes("git --version"), true);
  // A missing command must fail loud (127), not inherit a stale $LASTEXITCODE=0
  // (the rg/fd/bat false-positive). The try/catch is the guard.
  assertEquals(s.includes("exit $LASTEXITCODE"), true);
  assertEquals(s.includes("catch { exit 127 }"), true);
});

Deno.test("presenceProbe: no detect → null", () => {
  assertEquals(presenceProbe(null, false), null);
  assertEquals(presenceProbe("   ", true), null);
});

// --- nature 1b: run the CHECK command verbatim (run-route config-atoms) ---
Deno.test("checkProbe: POSIX runs the command verbatim under /bin/sh", () => {
  assertEquals(checkProbe("nu -c 'exit 0'", false), {
    cmd: "/bin/sh",
    args: ["-c", "nu -c 'exit 0'"],
  });
});

Deno.test("checkProbe: Windows refreshes PATH and forwards $LASTEXITCODE", () => {
  const p = checkProbe("nu -c 'exit 0'", true);
  assertEquals(p?.cmd, "powershell.exe");
  const s = p?.args[2] ?? "";
  assertEquals(s.includes("GetEnvironmentVariable('Path','Machine')"), true);
  assertEquals(s.includes("nu -c 'exit 0'"), true);
  assertEquals(s.includes("exit $LASTEXITCODE"), true);
  assertEquals(s.includes("catch { exit 127 }"), true); // missing tool → 127
});

Deno.test("checkProbe: no check → null", () => {
  assertEquals(checkProbe(null, false), null);
  assertEquals(checkProbe("   ", true), null);
});

// A `check` command is the definitive per-atom signal: exit 0 = present.
Deno.test("detectPresent: check exit 0 → present, non-zero → absent", async () => {
  assertEquals(
    await detectPresent(step({ route: "run", check: "sh -c 'exit 0'" }), false),
    true,
  );
  assertEquals(
    await detectPresent(step({ route: "run", check: "sh -c 'exit 1'" }), false),
    false,
  );
});

// --- nature 2: ask the route (GUI apps, no CLI binary) ---
Deno.test("routeProbe: winget route on Windows → winget list --id", () => {
  const p = routeProbe(
    step({ route: "winget", wingetId: "Obsidian.Obsidian" }),
    true,
  );
  assertEquals(p?.cmd, "powershell.exe");
  const s = p?.args[2] ?? "";
  assertEquals(s.includes("winget list --id Obsidian.Obsidian --exact"), true);
  assertEquals(s.includes("exit $LASTEXITCODE"), true);
  // The output must reach us (it carries the version) — no `| Out-Null`.
  assertEquals(s.includes("Out-Null"), false);
});

Deno.test("routeProbe: winget route on Mac → null (no winget → indeterminate)", () => {
  assertEquals(
    routeProbe(step({ route: "winget", wingetId: "Obsidian.Obsidian" }), false),
    null,
  );
});

Deno.test("routeProbe: unimplemented routes → null (don't guess)", () => {
  assertEquals(routeProbe(step({ route: "brew" }), false), null);
  assertEquals(routeProbe(step({ route: "npm" }), false), null);
});

// --- live IO on Mac: the three outcomes ---
Deno.test("detectPresent: binary present / absent (Mac)", async () => {
  assertEquals(
    await detectPresent(step({ detect: "sh --version" }), false),
    true,
  );
  assertEquals(
    await detectPresent(step({ detect: "nonexistent-binary-xyzzy" }), false),
    false,
  );
});

// Two-source combine: a package with a `detect` binary AND a winget route, where
// the binary is ABSENT and winget is unreachable (Mac) → absent, and crucially
// NOT flagged external (external requires the binary to actually respond). This
// is the regression for the "present · external" ghost on an uninstalled tool.
Deno.test("detectPresentDetailed: binary absent + winget route (Mac) → absent, not external", async () => {
  const r = await detectPresentDetailed(
    step({
      detect: "nonexistent-binary-xyzzy --version",
      route: "winget",
      wingetId: "Some.Pkg",
    }),
    false,
  );
  assertEquals(r.present, false);
  assertEquals(r.external, undefined); // never external when the binary didn't respond
});

Deno.test("detectPresentDetailed: binary present, no winget route → present, not external", async () => {
  const r = await detectPresentDetailed(
    step({ detect: "sh --version", route: null }),
    false,
  );
  assertEquals(r.present, true);
  assertEquals(!!r.external, false);
});

Deno.test("detectPresent: winget-only package on Mac → null (indeterminate)", async () => {
  // No detect binary, winget route, not on Windows → can't constate → null.
  const p = await detectPresent(
    step({ route: "winget", wingetId: "Obsidian.Obsidian" }),
    false,
  );
  assertEquals(p, null);
});

// NOTE: `requires` is NO LONGER a detect-time PATH guard — it's a package-level
// FUTURE-STATE dependency resolved globally in deps.ts (see test/deps.test.ts).
// detectPresentDetailed is pure per-step presence + a tool-absent reason only.

Deno.test("listProbe: claude-plugin → claude plugin list --json", () => {
  const p = listProbe(step({ route: "claude-plugin" }), false);
  assertEquals(p?.cmd, "claude");
  assertEquals(p?.args, ["plugin", "list", "--json"]);
});

Deno.test("listProbe: skill → npx skills list -g", () => {
  const p = listProbe(step({ route: "skill" }), false);
  assertEquals(p?.cmd, "npx");
  assertEquals(p?.args, ["skills", "list", "-g"]);
});

Deno.test("listProbe: other routes → null", () => {
  assertEquals(listProbe(step({ route: "winget" }), false), null);
});

// versionFrom: pull the installed version the probe output already reveals.
Deno.test("versionFrom: winget route grabs the token after the id", () => {
  const out = "Name     Id                Version\n" +
    "----------------------------------\n" +
    "marktext MarkText.MarkText 0.19.1\n";
  assertEquals(
    versionFrom(step({ route: "winget", wingetId: "MarkText.MarkText" }), out),
    "0.19.1",
  );
});

Deno.test("versionFrom: binary --version output → first version token", () => {
  assertEquals(
    versionFrom(
      step({ route: "run", detect: "git --version" }),
      "git version 2.43.0",
    ),
    "2.43.0",
  );
});

Deno.test("versionFrom: a 'v' prefix (node's v26.4.0) keeps the whole number", () => {
  // Regression: a leading \b made the regex backtrack to "4.0". Must be full.
  assertEquals(versionFrom(step({ route: "run" }), "v26.4.0"), "26.4.0");
  assertEquals(versionFrom(step({ route: "run" }), "0.113.1"), "0.113.1");
});

Deno.test("versionFrom: winget route but --version output → falls back to token", () => {
  // Helix has route=winget AND detect: hx --version, so the output is a
  // --version line, not a winget table. The id column isn't found → don't give
  // up, match the generic token. (Regression: it returned "" before.)
  assertEquals(
    versionFrom(
      step({ route: "winget", wingetId: "Helix.Helix" }),
      "helix 25.07.1 (a05c151b)",
    ),
    "25.07.1",
  );
});

Deno.test("versionFrom: unknown / empty → empty string (never guesses)", () => {
  assertEquals(
    versionFrom(step({ route: "winget", wingetId: "X.Y" }), "no match here"),
    "",
  );
  assertEquals(versionFrom(step({ route: "run" }), "nothing numeric"), "");
});

// version-regex: a bundle's optional override wins over the default extraction.
Deno.test("versionFrom: bundle version-regex overrides, capture group 1 wins", () => {
  // A package whose default extraction would be wrong supplies its own regex.
  assertEquals(
    versionFrom(
      step({ versionRegex: "v(\\d+\\.\\d+\\.\\d+)" }),
      "node v26.4.0",
    ),
    "26.4.0",
  );
  // No capture group → the whole match is the version.
  assertEquals(
    versionFrom(step({ versionRegex: "\\d+\\.\\d+" }), "build 3.14 xyz"),
    "3.14",
  );
});

Deno.test("versionFrom: a malformed bundle regex falls back to empty, never throws", () => {
  assertEquals(versionFrom(step({ versionRegex: "(" }), "1.2.3"), "");
});
