// Structural guards for the ONE path: an Apply converges the whole desire, no rung.
//
// The ladder (⚡ config only · 🧩 extensions · 📦 apps) left on 2026-10-10: applying PART of a
// desire built states nothing reasons about — a fresh config over a stale app — and nobody ever
// moved the slider. The nature glyph stays on the row; only the filter left. Read as text for
// the reason ladder-ui.test.mjs gave: app.js and index.html are DOM-bound and imported by no test.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";

const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");

test("no ladder is left in the page or the app", () => {
  assert.doesNotMatch(html, /id="ladder"|id="dial"|class="zone/);
  assert.doesNotMatch(appjs, /\brung\b|RUNGS|renderDial|setRungOut/);
  assert.ok(!existsSync(new URL("../public/ladder.js", import.meta.url)));
});

test("the apply message carries the desire and no rung", () => {
  const at = appjs.indexOf("function applyAll(");
  assert.notEqual(at, -1, "applyAll moved — re-point this test");
  const body = appjs.slice(at, appjs.indexOf("\n}", at));
  assert.match(body, /type:\s*"apply"/);
  assert.match(body, /unmanaged:/, "the derived out-of-scope list still travels");
  assert.doesNotMatch(body, /rung/);
});

test("the Apply button is the only way in, and it asks the shared plan", () => {
  // Moved from ladder-ui.test.mjs: the button is wired, and its emptiness comes from the same
  // function that lists what an Apply would touch — never a second rule.
  assert.match(appjs, /getElementById\("install-all"\)\.onclick = \(\) => applyAll\(\)/);
  const at = appjs.indexOf("function refreshLiveness()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /M\.applyPlan\(model\)/, "the button reads the shared plan");
});

test("the empty state speaks without a rung, in the list, never mid-scan", () => {
  assert.match(html, /id="steps-empty"/);
  assert.ok(
    html.indexOf('id="steps-empty"') < html.indexOf('id="steps"'),
    "the empty state belongs with the list, above its rows",
  );
  assert.match(appjs, /already matches what you asked for/);
  assert.doesNotMatch(appjs, /widen the scope/);
  // An un-probed row must never count as absent: no "nothing to send" while scanning.
  assert.match(appjs, /hint\.hidden = plan\.length !== 0 \|\| scanning/);
});

test("no row sends an action verb; retry re-sends the Apply's own action", () => {
  // The row button sent `{ type: act.type }` — install, uninstall, upgrade, downgrade. It left
  // on 2026-10-10; what a row may still send is a Stop (`cancel-step`) or a `diff`.
  assert.doesNotMatch(appjs, /type:\s*act\.type/);
  assert.doesNotMatch(appjs, /buttonAction/);
  const at = appjs.indexOf("function retryStep(");
  assert.notEqual(at, -1, "retryStep moved — re-point this test");
  const body = appjs.slice(at, appjs.indexOf("\n}", at));
  assert.match(body, /M\.actionOf\(model,\s*i\)/, "a retry re-runs what the Apply chose");
});

test("every running status the server emits is one the row treats as running", () => {
  // Found at the final review: the batched downgrade emitted "reinstalling at the pin", which
  // no front set listed — the row lost its Stop button, its spinner and its terminal while an
  // uninstall + reinstall ran. Read the statuses off `do_step` itself, so a new one cannot
  // be added on one side only.
  const rs = readFileSync(new URL("../src/server.rs", import.meta.url), "utf8");
  const at = rs.indexOf("let running = match action {");
  assert.notEqual(at, -1, "do_step's running statuses moved — re-point this test");
  const block = rs.slice(at, rs.indexOf("};", at));
  const statuses = [...block.matchAll(/=> "([^"]+)"/g)].map((m) => m[1]);
  assert.equal(statuses.length, 4, "one status per action");
  const set = (name) => appjs.slice(appjs.indexOf(`const ${name} =`), appjs.indexOf("\n", appjs.indexOf(`const ${name} =`)));
  for (const st of statuses) {
    assert.match(st, /^[a-z]+$/, `"${st}" must be one token — it becomes a CSS class`);
    assert.ok(set("RUNNING").includes(`"${st}"`), `RUNNING lacks "${st}" → no Stop button`);
    assert.ok(set("SPIN_STATES").includes(`"${st}"`), `SPIN_STATES lacks "${st}"`);
    const glyph = appjs.slice(appjs.indexOf("const GLYPH = {"), appjs.indexOf("};", appjs.indexOf("const GLYPH = {")));
    const label = appjs.slice(appjs.indexOf("const LABEL = {"), appjs.indexOf("};", appjs.indexOf("const LABEL = {")));
    assert.match(glyph, new RegExp(`\\b${st}:`), `GLYPH lacks ${st}`);
    assert.match(label, new RegExp(`\\b${st}:`), `LABEL lacks ${st}`);
  }
});

test("the button and the row both say a downgrade reinstalls", () => {
  const at = appjs.indexOf("function refreshLiveness()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /M\.planCounts\(model\)/, "the button counts through the shared helper");
  assert.match(body, /to reinstall/);
  const pv = appjs.slice(appjs.indexOf("function paintVersion("), appjs.indexOf("\n}\n", appjs.indexOf("function paintVersion(")));
  assert.match(pv, />reinstall</, "the row names the reinstall beside its target");
});

test("the empty state does not claim a match while a row is held", () => {
  const at = appjs.indexOf('getElementById("steps-empty")');
  assert.notEqual(at, -1, "the empty state moved — re-point this test");
  const body = appjs.slice(at, appjs.indexOf("\n  }\n", at));
  assert.match(body, /M\.heldRows\(model\)/, "the empty state must ask which rows are held");
  assert.match(body, /held/, "and say so instead of 'already matches'");
  // Found at the real click (Windows VM, 2026-10-22): a row held because its change moved
  // nothing read "held at a pin this route cannot reach" — it had no pin. The line counts;
  // the row names the cause.
  assert.doesNotMatch(body, /at a pin/, "the empty state must not name one cause for all");
});

test("the held reason is the same words on the row and in the server log", () => {
  const server = readFileSync(new URL("../src/server.rs", import.meta.url), "utf8");
  const model = readFileSync(new URL("../public/model.js", import.meta.url), "utf8");
  const rs = server.match(/const NO_DOWNGRADE_ROUTE: &str = "([^"]+)";/);
  assert.ok(rs, "NO_DOWNGRADE_ROUTE moved — re-point this test");
  assert.ok(model.includes(`"${rs[1]}"`), "model.js holdReason drifted from the server's words");
});

test("a row the server holds at Apply keeps the hold and shows it, even with nothing installed", () => {
  // `held` (server::send_held): brew cannot serve the pin. The row must remember it, or the
  // Apply button offers the same change forever; and an ABSENT row has no version to grey, so
  // paintVersion must show the pin itself as held.
  const at = appjs.indexOf('case "held"');
  assert.notEqual(at, -1, "app.js does not handle the server's held message");
  const body = appjs.slice(at, appjs.indexOf("break;", at));
  assert.match(body, /M\.setHold\(model, msg\.i, msg\.reason\)/);
  assert.match(body, /paintVersion\(msg\.i\)/);
  assert.match(body, /refreshLiveness\(\)/);
  // A retry from the firewall banner can be held too (it asks brew again): the hold is the
  // retry's verdict, so it settles the banner, or its controls stay inert for good.
  assert.match(body, /clearForbidden\(msg\.i\)/);
  // Found at the second review: the hold is the row's verdict, so a retry's "failed" or
  // "blocked by firewall" no longer stands; and an ABSENT row — a dependent held with
  // "requires nushell", no pin of its own — says why in its status, as a state with a reason does.
  // The status follows presence exactly as a `state` does: unknown stays unknown.
  assert.match(body, /present === true \? "ok" : present === false \? "waiting" : "unknown"/);
  assert.match(body, /if \(present !== true && rows\[msg\.i\]\) rows\[msg\.i\]\.statusLabel\.textContent = msg\.reason/);
  // Third review: "ok" clears `outdated`/`available` (setStatusData) and no `outdated` message
  // follows a `held`, so a present dependent's upgrade — and with it its hold — vanished.
  assert.match(body, /const \{ outdated, available \} = p/, "a held row keeps what it was behind on");
  assert.match(body, /Object\.assign\(p, \{ outdated, available \}\)/);
  const pv = appjs.slice(appjs.indexOf("function paintVersion("), appjs.indexOf("\n}\n", appjs.indexOf("function paintVersion(")));
  assert.match(pv, /if \(!v\.from\) \{[^}]*why/, "an absent held row shows why");
  assert.doesNotMatch(pv, /why && p\?\.pin\s*\?/, "the held word does not wait for a pin");
});

test("the Apply button says how long, from what these rows took here", () => {
  // The estimate used to sit beside the ladder's rungs; the ladder left and took the only
  // display with it. It belongs on the one button that runs the plan.
  const at = appjs.indexOf("function refreshLiveness()");
  const body = appjs.slice(at, appjs.indexOf("\n}\n", at));
  assert.match(body, /formatEstimate\(estimate\(M\.planSeconds\(model\)\)\)/);
  assert.match(appjs, /from "\.\/estimate\.js"/);
});
