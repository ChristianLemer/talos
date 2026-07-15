// Tests for presence-probe BUILDING (pure) — both natures. The IO (detectPresent)
// is exercised live on Mac below.
import { assertEquals } from "@std/assert";
import {
  checkProbe,
  detectPresent,
  detectPresentDetailed,
  listProbe,
  presenceProbe,
  systemProbe,
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
    downgrade: null,
    route: partial.route ?? null,
    systemId: partial.systemId ?? null,
    detect: partial.detect ?? null,
    check: partial.check ?? null,
    isConfig: partial.isConfig ?? false,
    versionRegex: partial.versionRegex ?? null,
    pin: partial.pin ?? null,
    requires: partial.requires ?? [],
    posture: "opt-in",
  };
}

// --- nature 1: RUN the author's detect command (presence + version) ---
Deno.test("presenceProbe: POSIX runs the full detect command", () => {
  const p = presenceProbe("node --version", "darwin");
  assertEquals(p?.cmd, "/bin/sh");
  assertEquals(p?.args, ["-lc", "node --version"]);
});

Deno.test("presenceProbe: Windows wraps with the 127 guard", () => {
  const p = presenceProbe("node --version", "windows");
  assertEquals(p?.cmd, "powershell.exe");
  assertEquals(p?.args[2].includes("catch { exit 127 }"), true);
});

Deno.test("presenceProbe: no detect → null", () => {
  assertEquals(presenceProbe(null, "darwin"), null);
  assertEquals(presenceProbe("   ", "windows"), null);
});

// --- nature 1b: run the CHECK command verbatim (run-route config-atoms) ---
Deno.test("checkProbe: POSIX runs the command verbatim under /bin/sh", () => {
  assertEquals(checkProbe("nu -c 'exit 0'", "darwin"), {
    cmd: "/bin/sh",
    args: ["-lc", "nu -c 'exit 0'"],
  });
});

Deno.test("checkProbe: Windows refreshes PATH and forwards $LASTEXITCODE", () => {
  const p = checkProbe("nu -c 'exit 0'", "windows");
  assertEquals(p?.cmd, "powershell.exe");
  const s = p?.args[2] ?? "";
  assertEquals(s.includes("GetEnvironmentVariable('Path','Machine')"), true);
  assertEquals(s.includes("nu -c 'exit 0'"), true);
  assertEquals(s.includes("exit $LASTEXITCODE"), true);
  assertEquals(s.includes("catch { exit 127 }"), true); // missing tool → 127
});

Deno.test("checkProbe: no check → null", () => {
  assertEquals(checkProbe(null, "darwin"), null);
  assertEquals(checkProbe("   ", "windows"), null);
});

// A `check` command is the definitive per-atom signal: exit 0 = present.
Deno.test("detectPresent: check exit 0 → present, non-zero → absent", async () => {
  assertEquals(
    await detectPresent(
      step({ route: "run", check: "sh -c 'exit 0'" }),
      "darwin",
    ),
    true,
  );
  assertEquals(
    await detectPresent(
      step({ route: "run", check: "sh -c 'exit 1'" }),
      "darwin",
    ),
    false,
  );
});

// --- nature 2: ask the system-manager route (GUI apps, no CLI binary) ---
Deno.test("systemProbe: winget id on windows → winget list probe", () => {
  const s = step({ route: "winget", systemId: "Git.Git" });
  const p = systemProbe(s, "windows");
  assertEquals(p?.cmd, "powershell.exe");
  assertEquals(p?.args[2].includes("winget list --id Git.Git"), true);
});

Deno.test("systemProbe: brew id on darwin → brew list probe", () => {
  const s = step({ route: "brew", systemId: "ripgrep" });
  const p = systemProbe(s, "darwin");
  assertEquals(p?.cmd, "/bin/sh");
  assertEquals(p?.args[1].includes("brew list --versions ripgrep"), true);
});

Deno.test("systemProbe: winget package on darwin → null (indeterminate, never guessed)", () => {
  const s = step({ route: "winget", systemId: "Git.Git" });
  assertEquals(systemProbe(s, "darwin"), null);
});

// --- live IO on Mac: the three outcomes ---
Deno.test("detectPresent: binary present / absent (Mac)", async () => {
  assertEquals(
    await detectPresent(step({ detect: "sh --version" }), "darwin"),
    true,
  );
  assertEquals(
    await detectPresent(step({ detect: "nonexistent-binary-xyzzy" }), "darwin"),
    false,
  );
});

// Two-source combine: a package with a `detect` binary AND a system route, where
// the binary is ABSENT and the manager can't list it → absent, and crucially NOT
// flagged external (external requires the binary to actually respond). This is the
// regression for the "present · external" ghost on an uninstalled tool. Uses the
// brew route so the native manager on this Mac bench IS practicable.
Deno.test("detectPresentDetailed: binary absent + system route (Mac) → absent, not external", async () => {
  const r = await detectPresentDetailed(
    step({
      detect: "nonexistent-binary-xyzzy --version",
      route: "brew",
      systemId: "nonexistent-formula-xyzzy",
    }),
    "darwin",
  );
  assertEquals(r.present, false);
  assertEquals(r.external, undefined); // never external when the binary didn't respond
});

Deno.test("detectPresentDetailed: binary present, no system route → present, not external", async () => {
  const r = await detectPresentDetailed(
    step({ detect: "sh --version", route: null }),
    "darwin",
  );
  assertEquals(r.present, true);
  assertEquals(!!r.external, false);
});

Deno.test("detectPresent: winget-only package on Mac → null (indeterminate)", async () => {
  // No detect binary, winget route, not on Windows → can't constate → null.
  const p = await detectPresent(
    step({
      route: "winget",
      systemId: "Obsidian.Obsidian",
    }),
    "darwin",
  );
  assertEquals(p, null);
});

// NOTE: `requires` is NO LONGER a detect-time PATH guard — it's a package-level
// FUTURE-STATE dependency resolved globally in deps.ts (see test/deps.test.ts).
// detectPresentDetailed is pure per-step presence + a tool-absent reason only.

Deno.test("listProbe: claude-plugin → claude plugin list --json", () => {
  const p = listProbe(step({ route: "claude-plugin" }), "darwin");
  assertEquals(p?.cmd, "claude");
  assertEquals(p?.args, ["plugin", "list", "--json"]);
});

Deno.test("listProbe: skill → npx skills list -g", () => {
  const p = listProbe(step({ route: "skill" }), "darwin");
  assertEquals(p?.cmd, "npx");
  assertEquals(p?.args, ["skills", "list", "-g"]);
});

Deno.test("listProbe: other routes → null", () => {
  assertEquals(listProbe(step({ route: "winget" }), "darwin"), null);
});

// versionFrom: pull the installed version the probe output already reveals.
Deno.test("versionFrom: winget route grabs the token after the id", () => {
  const out = "Name     Id                Version\n" +
    "----------------------------------\n" +
    "marktext MarkText.MarkText 0.19.1\n";
  assertEquals(
    versionFrom(
      step({
        route: "winget",
        systemId: "MarkText.MarkText",
      }),
      out,
    ),
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
      step({
        route: "winget",
        systemId: "Helix.Helix",
      }),
      "helix 25.07.1 (a05c151b)",
    ),
    "25.07.1",
  );
});

Deno.test("versionFrom: brew route, binary 'git version X' line → 2.50.1, NOT the word 'version'", () => {
  // Git declares brew: git but the machine has Apple git (external). The binary
  // probe prints "git version 2.50.1 (Apple Git-155)" — its second token is the
  // word "version". parseVersion must reject that non-numeric token so the generic
  // matcher finds 2.50.1. (Bug: the row displayed the literal word "version".)
  assertEquals(
    versionFrom(
      step({ route: "brew", systemId: "git" }),
      "git version 2.50.1 (Apple Git-155)",
    ),
    "2.50.1",
  );
});

Deno.test("versionFrom: unknown / empty → empty string (never guesses)", () => {
  assertEquals(
    versionFrom(step({ route: "winget", systemId: "X.Y" }), "no match here"),
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
