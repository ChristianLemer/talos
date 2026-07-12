// Tests for parseWingetUpgrade (pure) — slicing the fixed-width `winget upgrade`
// table by HEADER column offsets, not by splitting on spaces (names/versions
// contain spaces; offsets don't lie). Defensive: any hiccup → empty map.
import { assertEquals } from "@std/assert";
import {
  outdatedFor,
  parseWingetUpgrade,
  scanOutdated,
} from "../src/outdated.ts";

const fixture = await Deno.readTextFile(
  new URL("./fixtures/winget-upgrade.txt", import.meta.url),
);

Deno.test("parseWingetUpgrade: real fixture → one entry per outdated row", () => {
  const map = parseWingetUpgrade(fixture);
  assertEquals(map.size, 3);
});

Deno.test("parseWingetUpgrade: keys are the id, lowercased", () => {
  const map = parseWingetUpgrade(fixture);
  assertEquals(map.has("git.git"), true);
  assertEquals(map.has("microsoft.visualstudiocode"), true);
  assertEquals(map.has("7zip.7zip"), true);
});

Deno.test("parseWingetUpgrade: current→available sliced by offsets", () => {
  const map = parseWingetUpgrade(fixture);
  assertEquals(map.get("git.git"), { current: "2.54.0", available: "2.55.0" });
});

Deno.test("parseWingetUpgrade: names with spaces don't corrupt the id column", () => {
  const map = parseWingetUpgrade(fixture);
  // "Microsoft Visual Studio Code" has spaces; a space-split would break it.
  assertEquals(map.get("microsoft.visualstudiocode"), {
    current: "1.104.1",
    available: "1.105.0",
  });
  // "7-Zip 24.09 (x64)" — spaces AND digits in the name column.
  assertEquals(map.get("7zip.7zip"), {
    current: "24.09",
    available: "25.00",
  });
});

Deno.test("parseWingetUpgrade: strips ANSI clear-line before the header", () => {
  // winget clears the spinner line with ESC[2K then CR, then writes the header
  // on that same (now-blank) line. Stripping the ANSI + CR must leave the header
  // flush-left so its column offsets match the data rows.
  const raw = "\x1b[2K\r" +
    "Name   Id        Version   Available   Source\n" +
    "----   --        -------   ---------   ------\n" +
    "Foo    Foo.Bar   1.0       2.0         winget\n";
  const map = parseWingetUpgrade(raw);
  assertEquals(map.get("foo.bar"), { current: "1.0", available: "2.0" });
});

Deno.test("parseWingetUpgrade: no header → empty map (never throws)", () => {
  assertEquals(parseWingetUpgrade("nothing to see here").size, 0);
});

Deno.test("parseWingetUpgrade: empty input → empty map", () => {
  assertEquals(parseWingetUpgrade("").size, 0);
});

Deno.test("parseWingetUpgrade: trailing summary line is not a package", () => {
  const map = parseWingetUpgrade(fixture);
  assertEquals(map.has("12"), false);
  assertEquals([...map.keys()].some((k) => k.includes("upgrade")), false);
});

// --- scan IO shell -------------------------------------------------------
Deno.test("scanOutdated: returns a Map and never throws (darwin bench)", async () => {
  const map = await scanOutdated("darwin");
  assertEquals(map instanceof Map, true);
});

// --- bridge: does a Step match an outdated row? --------------------------
Deno.test("outdatedFor: matches wingetId case-insensitively", () => {
  const map = parseWingetUpgrade(fixture);
  assertEquals(outdatedFor("Git.Git", map), {
    current: "2.54.0",
    available: "2.55.0",
  });
});

Deno.test("outdatedFor: no wingetId → null (nothing to match on)", () => {
  const map = parseWingetUpgrade(fixture);
  assertEquals(outdatedFor(null, map), null);
});

Deno.test("outdatedFor: id absent from scan → null (up to date)", () => {
  const map = parseWingetUpgrade(fixture);
  assertEquals(outdatedFor("Not.Listed", map), null);
});

Deno.test("outdatedFor: matches on the ACTIVE manager id (brew key ≠ winget id)", () => {
  // On Mac the scan is keyed by the brew short name; a package's systemId is its
  // brew id, NOT its winget id. Passing the winget id would never match.
  const scan = new Map([["obsidian", {
    current: "1.5.0",
    available: "1.6.0",
  }]]);
  // systemId is what the caller must pass — the brew id on this bench:
  assertEquals(outdatedFor("obsidian", scan)?.available, "1.6.0");
  // the winget id would miss:
  assertEquals(outdatedFor("Obsidian.Obsidian", scan), null);
});
