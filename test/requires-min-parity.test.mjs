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
  assert.match(hold, /const held = HELD_ACTIONS\.includes\(a\);/);
  assert.match(hold, /held \? ownMinimum\(model, i\) : null/);
  assert.match(hold, /held \? chainHold\(model, i, new Set\(\)\) : null/);

  // …and every server gate that can run a row asks the same function.
  const server = readFileSync(new URL("../src/server.rs", import.meta.url), "utf8");
  assert.match(slice(server, "async fn requirement_blocks_row(", "\n}"), /held_by_minimum\(action,/);
  const apply = slice(server, "async fn apply_diff(", "\nasync fn ");
  assert.match(apply, /held_by_minimum\(a, &nodes\[i\]/, "the batch holds by the action");
  assert.match(apply, /held_by_minimum\(Action::Install, &nodes\[d\]/, "the repair is an install");
});

test("the dependents' repair judges a minimum on the requirement AFTER its step", () => {
  // Review finding: the repair runs because the requirement's version moved, so reading the
  // version from before the step would refuse it on "found <old>". And a refusal says so.
  const server = readFileSync(new URL("../src/server.rs", import.meta.url), "utf8");
  const apply = slice(server, "async fn apply_diff(", "\nasync fn ");
  const refreshed = apply.indexOf("nodes[*i].version = after.version.clone();");
  const judged = apply.indexOf("held_by_minimum(Action::Install, &nodes[d]");
  assert.ok(refreshed > 0 && judged > refreshed, "refreshed before it is judged");
  const present = apply.indexOf("nodes[*i].present_now = after.present == Some(true);");
  assert.ok(present > 0 && judged > present, "presence refreshed too");
  assert.match(apply.slice(judged, judged + 300), /send_held\(socket, d, &low\)/, "the refusal leaves a trace");
});

test("a row held by a minimum AND an unmoved change gives the server's reason: the minimum first", () => {
  const server = readFileSync(new URL("../src/server.rs", import.meta.url), "utf8");
  const apply = slice(server, "async fn apply_diff(", "\nasync fn ");
  const rsMin = apply.indexOf("held_by_minimum(a, &nodes[i]");
  const rsUnmoved = apply.indexOf("unmoved_in_view(state, step, a,");
  assert.ok(rsMin > 0 && rsUnmoved > rsMin, "server: minimum before unmoved");
  const js = readFileSync(new URL("../public/model.js", import.meta.url), "utf8");
  const hold = slice(js, "export function holdReason(", "\n}");
  const jsMin = hold.indexOf("ownMinimum(model, i)");
  const jsUnmoved = hold.indexOf("unmovedHold(model, i)");
  const jsChain = hold.indexOf("chainHold(model, i,");
  assert.ok(jsMin > 0 && jsUnmoved > jsMin, "front: own minimum before unmoved");
  assert.ok(jsChain > jsUnmoved, "front: the chain after unmoved, as drop_requiring_held runs after the loop");
  const rsChain = apply.indexOf("drop_requiring_held(&mut visual_plan");
  assert.ok(rsChain > rsUnmoved, "server: the chain after the per-row holds");
});

test("paintVersion puts no probe text into innerHTML unescaped", () => {
  // Pre-tag review: a minimum's reason (holdWord) carries a content name and a version read
  // off a probe, and every version is probe output. This webview drives a server that runs
  // commands, so each text enters the markup through `attr`, at its leaf.
  const app = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const body = slice(app, "function paintVersion(i) {", "\nfunction ");
  // `pin` is the one plain-text helper (the leaf escapes its result); it is checked apart.
  const pinLine = /\n\s*const pin = \(t\) => `📌\$\{t\}`;/;
  assert.match(body, pinLine, "pin stays plain text: the leaf escapes it once");
  const rest = body.replace(pinLine, "");
  assert.ok(!/\$\{[^}]*\$\{/.test(rest), "no nested template: this guard could not read it");
  const raw = [...rest.matchAll(/\$\{([^}]*)\}/g)].map((m) => m[1].trim())
    .filter((e) => !/^attr\(/.test(e))
    // composed markup, every piece of it built from attr(…) or constants
    .filter((e) => !/^(cur|remove|push|muted)\(|^(left|right|re|own|lag|target)$/.test(e));
  assert.deepEqual(raw, [], "an interpolation that is neither attr(…) nor composed markup");
  // …and nothing joins the markup by `+` except those same composed pieces.
  for (const m of rest.matchAll(/innerHTML\s*=\s*([^;]+);/g)) {
    const parts = m[1].split("+").map((x) => x.trim());
    if (parts.length < 2) continue; // a single template: its interpolations are checked above
    const plus = parts.filter((x) => !x.startsWith("`") && !x.endsWith("`"));
    for (const x of plus) assert.match(x, /^(cur\(|own$|lag$)/, `joined raw into innerHTML: ${x}`);
  }
  for (const leaf of ["cur", "push", "remove"]) {
    assert.match(rest, new RegExp(`const ${leaf} = \\(t\\) => \`<span class="v-[a-z]+">\\$\\{attr\\(t\\)\\}</span>\``), leaf);
  }
});

test("the journal's rows escape what they show", () => {
  const app = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const body = slice(app, "function renderLog(", "\nfunction ");
  const row = slice(body, "row.innerHTML = ", ";\n");
  for (const f of ["when", "e.package", "e.version", "e.action"]) {
    assert.ok(!row.includes("${" + f), `${f} raw into the history row`);
  }
});
