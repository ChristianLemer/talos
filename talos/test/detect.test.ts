// Tests for presence-probe BUILDING (pure). The IO (detectPresent) is exercised
// live on Mac by the running server; here we pin the command construction.
import { assertEquals } from "@std/assert";
import { detectPresent, presenceProbe } from "../src/detect.ts";

Deno.test("presenceProbe: POSIX uses command -v on the first token", () => {
  const p = presenceProbe("node --version", false);
  assertEquals(p, { cmd: "/bin/sh", args: ["-c", "command -v 'node'"] });
});

Deno.test("presenceProbe: only the binary, arguments dropped", () => {
  const p = presenceProbe("winget list --id Foo", false);
  assertEquals(p?.args[1], "command -v 'winget'");
});

Deno.test("presenceProbe: Windows refreshes PATH then Get-Command", () => {
  const p = presenceProbe("git --version", true);
  assertEquals(p?.cmd, "powershell.exe");
  // PATH refresh must come BEFORE the probe (winget writes registry, not the
  // running process's PATH) — else a freshly-installed tool reads as absent.
  const script = p?.args[2] ?? "";
  assertEquals(
    script.includes("GetEnvironmentVariable('Path','Machine')"),
    true,
  );
  assertEquals(script.includes("Get-Command 'git'"), true);
});

Deno.test("presenceProbe: no detect → null (nothing to probe)", () => {
  assertEquals(presenceProbe(null, false), null);
  assertEquals(presenceProbe("", true), null);
  assertEquals(presenceProbe("   ", false), null);
});

// Live IO smoke: /bin/sh is always present on Mac, a nonsense binary never is.
// Proves detectPresent reads the exit code correctly and never throws.
Deno.test("detectPresent: real probe on Mac (sh present, garbage absent)", async () => {
  assertEquals(await detectPresent("sh --version", false), true);
  assertEquals(await detectPresent("nonexistent-binary-xyzzy", false), false);
});
