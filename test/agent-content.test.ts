// Pure parsers for `claude plugin list --json` and `npx skills list -g`.
// Defensive: malformed input → nothing installed (safe direction).
import { assertEquals } from "@std/assert";
import {
  parsePluginList,
  parseSkillList,
  pluginPresent,
  skillPresent,
} from "../src/agent-content.ts";

const PLUGIN_JSON = JSON.stringify([
  { id: "astral@astral-sh", enabled: true },
  { id: "chiron@tekton", enabled: true },
  { id: "caveman@caveman", enabled: false },
]);

Deno.test("parsePluginList: ids from the JSON array", () => {
  assertEquals(parsePluginList(PLUGIN_JSON), [
    "astral@astral-sh",
    "chiron@tekton",
    "caveman@caveman",
  ]);
});

Deno.test("parsePluginList: malformed → empty (never throws)", () => {
  assertEquals(parsePluginList("not json"), []);
  assertEquals(parsePluginList(""), []);
  assertEquals(parsePluginList("{}"), []); // object, not array
});

Deno.test("pluginPresent: matches full id or the plugin part before @", () => {
  assertEquals(pluginPresent("chiron@tekton", PLUGIN_JSON), true);
  assertEquals(pluginPresent("chiron", PLUGIN_JSON), true); // detect: chiron
  assertEquals(pluginPresent("absent@x", PLUGIN_JSON), false);
});

const SKILL_LIST = "\x1b[1mGlobal Skills\x1b[0m\n\n" +
  "\x1b[36mgws-gmail  \x1b[0m \x1b[38;5;102m~/.agents/skills/gws-gmail\x1b[0m Agents: Claude Code\n" +
  "\x1b[36mherdr      \x1b[0m \x1b[38;5;102m~/.claude/skills/herdr\x1b[0m Agents: Claude Code\n";

Deno.test("parseSkillList: names from the first column, ANSI stripped", () => {
  assertEquals(parseSkillList(SKILL_LIST), ["gws-gmail", "herdr"]);
});

Deno.test("parseSkillList: ignores npm warn noise + header", () => {
  const raw = "npm warn exec The following package was not found\n" +
    SKILL_LIST;
  assertEquals(parseSkillList(raw), ["gws-gmail", "herdr"]);
});

Deno.test("parseSkillList: empty → nothing", () => {
  assertEquals(parseSkillList(""), []);
});

Deno.test("skillPresent: name match", () => {
  assertEquals(skillPresent("herdr", SKILL_LIST), true);
  assertEquals(skillPresent("nope", SKILL_LIST), false);
});
