// Tests for presence-probe BUILDING (pure) — both natures. The IO (detectPresent)
// is exercised live on Mac below.
import { assertEquals } from "@std/assert";
import { detectPresent, presenceProbe, routeProbe } from "../src/detect.ts";
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
    detect: partial.detect ?? null,
    requires: [],
    selfHost: false,
    posture: "opt-in",
  };
}

// --- nature 1: binary on PATH (source-agnostic) ---
Deno.test("presenceProbe: POSIX uses command -v on the first token", () => {
  assertEquals(presenceProbe("node --version", false), {
    cmd: "/bin/sh",
    args: ["-c", "command -v 'node'"],
  });
});

Deno.test("presenceProbe: Windows refreshes PATH before Get-Command", () => {
  const p = presenceProbe("git --version", true);
  assertEquals(p?.cmd, "powershell.exe");
  const s = p?.args[2] ?? "";
  assertEquals(s.includes("GetEnvironmentVariable('Path','Machine')"), true);
  assertEquals(s.includes("Get-Command 'git'"), true);
});

Deno.test("presenceProbe: no detect → null", () => {
  assertEquals(presenceProbe(null, false), null);
  assertEquals(presenceProbe("   ", true), null);
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

Deno.test("detectPresent: winget-only package on Mac → null (indeterminate)", async () => {
  // No detect binary, winget route, not on Windows → can't constate → null.
  const p = await detectPresent(
    step({ route: "winget", wingetId: "Obsidian.Obsidian" }),
    false,
  );
  assertEquals(p, null);
});
