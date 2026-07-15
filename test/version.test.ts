// Tests for version comparison — the pure core of version pinning. compareVersions
// decides below/equal/above so the repaint can pick install/upgrade/downgrade.
// Run: deno task test
import { assertEquals } from "@std/assert";
import { compareVersions } from "../public/version.js";

Deno.test("compareVersions: equal versions → 0", () => {
  assertEquals(compareVersions("1.8.0", "1.8.0"), 0);
});

Deno.test("compareVersions: lower → -1, higher → 1", () => {
  assertEquals(compareVersions("1.8.0", "2.0.0"), -1);
  assertEquals(compareVersions("2.0.0", "1.8.0"), 1);
});

Deno.test("compareVersions: numeric segments, NOT lexicographic (2.10 > 2.9)", () => {
  assertEquals(compareVersions("2.10", "2.9"), 1);
  assertEquals(compareVersions("2.9", "2.10"), -1);
});

Deno.test("compareVersions: missing trailing segments count as 0 (1.8 == 1.8.0)", () => {
  assertEquals(compareVersions("1.8", "1.8.0"), 0);
  assertEquals(compareVersions("1.8.0", "1.8"), 0);
  assertEquals(compareVersions("1.8", "1.8.1"), -1);
});

Deno.test("compareVersions: leading numeric prefix of each segment (suffixes ignored)", () => {
  // node's "v26.4.0" style is stripped upstream; here we get plain dotted numbers,
  // but a segment like "0-beta" is read by its numeric prefix (0). Documented rule.
  assertEquals(compareVersions("2.43.0-beta", "2.43.0"), 0);
  assertEquals(compareVersions("1.0.0-rc1", "1.0.1"), -1);
});

Deno.test("compareVersions: empty / non-numeric → treated as 0", () => {
  assertEquals(compareVersions("", ""), 0);
  assertEquals(compareVersions("1.0", ""), 1);
});
