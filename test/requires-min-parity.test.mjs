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
    "function belowMinimum(",
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

test("a minimum holds the same actions on both sides: every one but a removal", () => {
  // The pre-tag finding: a row whose host is too old must stay removable. The engine says
  // which action is exempt (`held_by_minimum`), the front which are held (`HELD_ACTIONS`,
  // read by `holdReason` → `minimumReason`); the two must cover every Action, apart.
  const rsAll = readFileSync(new URL("../src/deps.rs", import.meta.url), "utf8");
  const decision = readFileSync(new URL("../src/decision.rs", import.meta.url), "utf8");
  const asStr = slice(decision, "pub fn as_str(&self)", "\n    }");
  const actions = Object.fromEntries([...asStr.matchAll(/Action::(\w+) => "(\w+)"/g)].map((m) => [m[1], m[2]]));
  assert.equal(Object.keys(actions).length, 4, "one word per Action");
  const held = slice(rsAll, "pub fn held_by_minimum(", "\n}");
  const exempt = [...held.matchAll(/action == crate::decision::Action::(\w+)/g)].map((m) => actions[m[1]]);
  assert.deepEqual(exempt, ["uninstall"]);

  const js = readFileSync(new URL("../public/model.js", import.meta.url), "utf8");
  const heldJs = JSON.parse(slice(js, "const HELD_ACTIONS = ", ";").slice("const HELD_ACTIONS = ".length));
  assert.deepEqual(
    [...heldJs].sort(),
    Object.values(actions).filter((a) => !exempt.includes(a)).sort(),
  );
  const hold = slice(js, "export function holdReason(", "\n}");
  assert.match(hold, /HELD_ACTIONS\.includes\(a\)\)\s*\{\s*const low = minimumReason\(model, i\)/);

  // …and every server gate that can run a row asks the same function.
  const server = readFileSync(new URL("../src/server.rs", import.meta.url), "utf8");
  assert.match(slice(server, "async fn requirement_blocks_row(", "\n}"), /held_by_minimum\(action,/);
  const apply = slice(server, "async fn apply_diff(", "\nasync fn ");
  assert.match(apply, /held_by_minimum\(a, &nodes\[i\]/, "the batch holds by the action");
  assert.match(apply, /held_by_minimum\(Action::Install, &nodes\[d\]/, "the repair is an install");
});
