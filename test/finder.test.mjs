// finder.test.mjs — the Catalog finder's matcher (public/finder.js): fzf-like subsequence
// matching, folded case and accents, the name ranked above everything else, and the view a
// query asks for (which categories open, which fold).
import { test } from "node:test";
import assert from "node:assert/strict";
import { fold, scoreText, findEntry, matchEntry, rank, findView, terms, TIER_NAME, TIER_OTHER }
  from "../public/finder.js";

const entry = (name, more = {}) => findEntry({ name, ...more });
const keys = (q, list) => rank(q, list.map((e, key) => ({ key, entry: e }))).map((r) => r.key);

test("fold drops case and accents, on both sides", () => {
  assert.equal(fold("Écran Ça ÏÔ"), "ecran ca io");
  assert.equal(fold(null), "");
  assert.notEqual(matchEntry("ECRAN", entry("écran")), null);
  assert.notEqual(matchEntry("écran", entry("Ecran")), null);
});

test("a subsequence matches, in order only", () => {
  assert.notEqual(scoreText("vsc", fold("Visual Studio Code")), null);
  assert.equal(scoreText("csv", fold("Visual Studio Code")), null);
  assert.equal(scoreText("gitx", "git"), null);
  assert.equal(scoreText("", "git"), 0);
});

test("consecutive characters and word starts score higher than scattered ones", () => {
  // "code" as a word beats the same letters spread through a longer word.
  assert.ok(scoreText("code", "visual studio code") > scoreText("code", "cxoxdxe"));
  // Starting a word beats the middle of one.
  assert.ok(scoreText("st", "visual studio") > scoreText("st", "fastest"));
  // A contiguous run beats a gapped one of the same length.
  assert.ok(scoreText("nu", "nushell") > scoreText("nu", "n-x-u"));
  // ... even when each scattered letter starts a word: the run carries its first letter's
  // word-start bonus (fzf's rule), so "git" itself beats "g-i-t".
  assert.ok(scoreText("git", "git") > scoreText("git", "g-i-t"));
  assert.ok(scoreText("nu", "nushell") > scoreText("nu", "n-u"));
  assert.ok(scoreText("code", "code") > scoreText("code", "c-o-d-e"));
  assert.deepEqual(keys("git", [entry("g i t"), entry("Git")]), [1, 0]);
});

test("the name outranks the description, the category and the id", () => {
  const list = [
    entry("Alpha", { description: "a shell for structured data", categories: ["shell"] }),
    entry("Nushell", { description: "the one" }),
    entry("Beta", { id: "shell-beta" }),
  ];
  const r = rank("shell", list.map((e, key) => ({ key, entry: e })));
  assert.equal(r[0].key, 1);
  assert.equal(r[0].tier, TIER_NAME);
  assert.ok(r.slice(1).every((x) => x.tier === TIER_OTHER));
  assert.equal(r.length, 3);
});

test("a weak name match still beats a strong one elsewhere", () => {
  const list = [
    entry("Git", { description: "version control" }),
    entry("s-h-e-l-l-y"), // scattered, but in the NAME
  ];
  assert.deepEqual(keys("shell", list), [1]);
  list.push(entry("Zed", { description: "shell" }));
  assert.deepEqual(keys("shell", list), [1, 2]);
});

test("the catalogue id is searched", () => {
  assert.deepEqual(keys("vscode", [entry("Code"), entry("Visual Studio Code", { id: "vscode" })]), [1]);
});

test("prose is matched word by word, so a sentence does not match every query", () => {
  const e = entry("Zed", { description: "run a command for you" });
  assert.equal(matchEntry("nu", e), null, "n… u spread over three words must not match");
  assert.notEqual(matchEntry("comm", e), null);
  assert.notEqual(matchEntry("cmd", e), null, "within one word, fzf's subsequence still holds");
});

test("several terms must all match (AND)", () => {
  const list = [
    entry("Visual Studio Code", { categories: ["editors"] }),
    entry("Claude Code for VS Code", { categories: ["editors"], description: "extension" }),
  ];
  assert.deepEqual(keys("code ext", list), [1]);
  assert.deepEqual(terms("  a   B "), ["a", "b"]);
  // The row's tier is its worst term's: one term only in the description = not a name match.
  assert.equal(matchEntry("code ext", list[1]).tier, TIER_OTHER);
});

test("equal scores: the shorter name first, then the catalogue's order", () => {
  assert.deepEqual(keys("git", [entry("Git LFS"), entry("Git"), entry("Git LFS")]), [1, 0, 2]);
});

test("an empty query is no find at all", () => {
  assert.equal(findView("", [{ key: 0, cat: "a", entry: entry("x") }]), null);
  assert.equal(findView("   ", [{ key: 0, cat: "a", entry: entry("x") }]), null);
  assert.equal(matchEntry("", entry("x")), null);
});

test("the view: matching categories open, best first; the others fold, in catalogue order", () => {
  const rows = [
    { key: 0, cat: "cli", entry: entry("jq", { description: "json processor" }) },
    { key: 1, cat: "editors", entry: entry("Visual Studio Code") },
    { key: 2, cat: "foundation", entry: entry("Git") },
    { key: 3, cat: "shell", entry: entry("Nushell", { description: "structured json data" }) },
  ];
  const v = findView("json", rows);
  // Both are description matches; jq's description starts with the word.
  assert.deepEqual(v.matched, [0, 3]);
  assert.deepEqual(v.cats, [
    { cat: "cli", open: true },
    { cat: "shell", open: true },
    { cat: "editors", open: false },
    { cat: "foundation", open: false },
  ]);
  // A name match pulls its category to the top.
  assert.deepEqual(findView("nushell", rows).cats[0], { cat: "shell", open: true });
});

test("the view of a query nothing matches: every category folds, none vanishes", () => {
  const rows = [
    { key: 0, cat: "a", entry: entry("x") },
    { key: 1, cat: "b", entry: entry("y") },
  ];
  const v = findView("zzz", rows);
  assert.deepEqual(v.matched, []);
  assert.deepEqual(v.cats, [{ cat: "a", open: false }, { cat: "b", open: false }]);
});

// ⭐ The finder is a VIEW: in app.js, the block that paints it (from its banner to the next
// function) may read rows and toggle classes, but must never reach the model, the socket or
// the row's content. A filter SORTS, a label INFORMS — and a view decides nothing.
test("the finder's wiring in app.js changes no scope, no intention, no Apply, no label", async () => {
  const { readFileSync } = await import("node:fs");
  const app = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const start = app.indexOf("// --- the Catalog finder ---");
  assert.notEqual(start, -1);
  // The block, plus the find branch of the category header's click (inside render()).
  const branchAt = app.indexOf("if (findOn()) {");
  assert.notEqual(branchAt, -1);
  const branch = app.slice(branchAt, app.indexOf("}", branchAt));
  const block = app.slice(start, app.indexOf("function ensureTerm", start)) + "\n" + branch;
  const code = block.split("\n").filter((l) => !l.trim().startsWith("//"));
  const banned = [/ws\.send/, /\bM\./, /setToggle/, /setScope/, /setDecision/, /innerHTML/,
    /localStorage/, /persist/, /textContent\s*=/];
  for (const re of banned) {
    // The empty line is the one text it writes, and it is its own element.
    const hits = code.filter((l) => re.test(l) && !/empty\.textContent\s*=/.test(l));
    assert.deepEqual(hits, [], `the finder must not use ${re}`);
  }
  // The only classes it sets are its own (find-*) and `finding` on the body.
  for (const l of code) {
    for (const m of l.matchAll(/classList\.(?:add|toggle|remove)\(([^)]*)\)/g)) {
      for (const cls of m[1].matchAll(/"([^"]+)"/g)) {
        assert.ok(/^find(ing|-)/.test(cls[1]), `the finder sets a class that is not its own: ${cls[1]}`);
      }
    }
  }
  const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
  // Catalog only: hidden by default, shown under tab-catalog, and the find's hiding rules
  // all scoped to it, so the Changes tab never sees a query.
  assert.match(html, /#finder \{ display:none; \}/);
  assert.match(html, /body\.tab-catalog #finder \{ display:flex;/);
  // Every rule that acts on the find is scoped to the Catalog and yields to an Apply's focus
  // mode: a standing query must never hide the running row or a failure.
  const rules = html.match(/^\s*body\.tab-catalog\.finding.*$/gm);
  assert.ok(rules.length >= 4);
  for (const rule of rules) assert.match(rule, /:not\(\.applying\)/, rule.trim());
  for (const rule of html.match(/^.*details\.find-(miss|fold).*$/gm)) {
    assert.match(rule, /body\.tab-catalog\.finding:not\(\.applying\)/, rule.trim());
  }
});
