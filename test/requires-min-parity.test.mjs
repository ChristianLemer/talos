// One rule, two callers: the hold a minimum puts on a row must read the same whether the
// scan said it (model.js `requiresReason`) or the Apply did (deps.rs `below_minimum`).
// The wordings are read off both sources, so neither can change alone.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const slice = (text, start, end) => {
  const at = text.indexOf(start);
  assert.notEqual(at, -1, `${start} moved — re-point this test`);
  return text.slice(at, text.indexOf(end, at));
};

test("the minimum's wordings are the same in deps.rs and model.js", () => {
  const rs = slice(
    readFileSync(new URL("../src/deps.rs", import.meta.url), "utf8"),
    "fn below_minimum(",
    "\n}",
  );
  const js = slice(
    readFileSync(new URL("../public/model.js", import.meta.url), "utf8"),
    "export function requiresReason(",
    "\n}",
  );
  // `{}` / `{min}` in Rust, `${…}` in JS: both become one placeholder.
  const rsWords = [...rs.matchAll(/"(requires \{[^"]*)"/g)].map((m) => m[1].replace(/\{[^}]*\}/g, "{}"));
  const jsWords = [...js.matchAll(/`(requires \$\{[^`]*>= [^`]*)`/g)].map((m) =>
    m[1].replace(/\$\{[^}]*\}/g, "{}"),
  );
  assert.deepEqual(rsWords.sort(), ["requires {} >= {} (found {})", "requires {} >= {} (version unknown)"]);
  assert.deepEqual(jsWords.sort(), rsWords);
});
