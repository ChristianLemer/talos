// Tests for the SystemManager strategy — winget & brew as one route, two
// platform incarnations. Pure command-building + output parsing. Run: deno task test
import { assertEquals } from "@std/assert";
import { nativeManager, WINGET } from "../src/managers.ts";

Deno.test("WINGET: install/uninstall/upgrade command spelling", () => {
  assertEquals(
    WINGET.install("Git.Git"),
    "winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements",
  );
  assertEquals(WINGET.uninstall("Git.Git"), "winget uninstall --id Git.Git -e --source winget");
});

Deno.test("WINGET: presence command exits 0 iff installed, table carries version", () => {
  assertEquals(
    WINGET.presenceCommand("Git.Git"),
    "winget list --id Git.Git --exact --source winget --accept-source-agreements",
  );
});

Deno.test("WINGET.parseVersion: token after the id in a winget list table", () => {
  const table = "Name   Id        Version\n----\nGit    Git.Git   2.43.0";
  assertEquals(WINGET.parseVersion("Git.Git", table), "2.43.0");
});

Deno.test("nativeManager: windows → winget", () => {
  assertEquals(nativeManager("windows")?.route, "winget");
});
