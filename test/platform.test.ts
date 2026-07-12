// Tests for the Platform substrate — the single source of `os` and how to run a
// command string on this OS. Pure; no IO. Run: deno task test
import { assertEquals } from "@std/assert";
import { currentOs, type Os } from "../src/platform.ts";

Deno.test("currentOs: returns one of the three known values", () => {
  const os: Os = currentOs();
  assertEquals(["windows", "darwin", "linux"].includes(os), true);
});
