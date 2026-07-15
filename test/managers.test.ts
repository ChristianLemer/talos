// Tests for the SystemManager strategy — winget & brew as one route, two
// platform incarnations. Pure command-building + output parsing. Run: deno task test
import { assertEquals } from "@std/assert";
import {
  BREW,
  nativeManager,
  parseWingetUpgrade,
  WINGET,
} from "../src/managers.ts";

const brewOutdated = await Deno.readTextFile(
  new URL("./fixtures/brew-outdated.json", import.meta.url),
);

const wingetUpgrade = await Deno.readTextFile(
  new URL("./fixtures/winget-upgrade.txt", import.meta.url),
);

Deno.test("WINGET: install/uninstall/upgrade command spelling", () => {
  assertEquals(
    WINGET.install("Git.Git"),
    "winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements",
  );
  assertEquals(
    WINGET.uninstall("Git.Git"),
    "winget uninstall --id Git.Git -e --source winget",
  );
});

Deno.test("WINGET.upgrade command spelling", () => {
  assertEquals(
    WINGET.upgrade("Git.Git"),
    "winget upgrade --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements",
  );
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

Deno.test("parseWingetUpgrade: real fixture → one entry per outdated row, keyed lowercased id", () => {
  const map = parseWingetUpgrade(wingetUpgrade);
  assertEquals(map.size, 3);
  assertEquals(map.has("git.git"), true);
});

Deno.test("BREW: install/uninstall/upgrade (brew resolves cask vs formula itself)", () => {
  // --yes: brew 6.x prompts "[y/n]" before upgrading dependencies. Talos's xterm
  // is display-only (no stdin path), so a prompt DEADLOCKS the step. Pressing
  // Apply already IS the consent — auto-answer so the command never asks.
  assertEquals(BREW.install("ripgrep"), "brew install --yes ripgrep");
  assertEquals(BREW.uninstall("ripgrep"), "brew uninstall ripgrep");
  assertEquals(BREW.upgrade("ripgrep"), "brew upgrade --yes ripgrep");
});

Deno.test("BREW.installPinned: versioned formula, also non-interactive", () => {
  assertEquals(BREW.installPinned("jq", "1.8"), "brew install --yes jq@1.8");
});

Deno.test("BREW: presence command is uniform across formula and cask", () => {
  assertEquals(
    BREW.presenceCommand("visual-studio-code"),
    "brew list --versions visual-studio-code || brew list --cask --versions visual-studio-code",
  );
});

Deno.test("BREW.parseVersion: token after the id (formula and cask alike)", () => {
  assertEquals(BREW.parseVersion("jq", "jq 1.8.2"), "1.8.2");
  assertEquals(BREW.parseVersion("obsidian", "obsidian 1.12.7"), "1.12.7");
  assertEquals(BREW.parseVersion("jq", ""), "");
});

Deno.test("BREW.parseVersion: id followed by a NON-version word → no match (binary-format line)", () => {
  // The binary probe for a brew-declared package can print a line like
  // "git version 2.50.1 (Apple Git-155)" (Apple git, NOT brew list's "git 2.50.1").
  // The token after the id is then "version", not a number — must NOT be taken as
  // the version. Returning "" lets versionFrom fall back to the generic matcher
  // that finds 2.50.1. (This is why Git showed the literal word "version".)
  assertEquals(
    BREW.parseVersion("git", "git version 2.50.1 (Apple Git-155)"),
    "",
  );
});

Deno.test("BREW.parseOutdated: json=v2 formulae+casks → map keyed by lowercased name", () => {
  const map = BREW.parseOutdated(brewOutdated);
  assertEquals(map.size, 2);
  assertEquals(map.get("nushell"), { current: "0.99.0", available: "0.100.0" });
  assertEquals(map.get("visual-studio-code"), {
    current: "1.126.0",
    available: "1.127.0",
  });
});

Deno.test("BREW.parseOutdated: malformed → empty map (safe direction)", () => {
  assertEquals(BREW.parseOutdated("not json").size, 0);
});

Deno.test("nativeManager: darwin and linux → brew", () => {
  assertEquals(nativeManager("darwin")?.route, "brew");
  assertEquals(nativeManager("linux")?.route, "brew");
});
