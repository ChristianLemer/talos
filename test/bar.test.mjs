import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { barView, planSub, IDLE_HINT } from "../public/bar.js";

const open = { conn: "open", scanning: false };
const text = (v) => v.hint.map((h) => h.text).join(" · ");

// --- the plan: a short button, the full sentence beside it -----------------------

test("a plan: Apply is filled, Quit is not, and the line says the plan in full", () => {
  const v = barView({ ...open, actions: ["install", "install", "uninstall"], estimate: "~4 min for 2, 1 never timed here" });
  assert.equal(v.label, "Apply");
  assert.equal(v.sub, "2 installs · 1 removal");
  assert.equal(v.live, true);
  assert.equal(v.disabled, false);
  assert.equal(v.quitPrimary, false, "never two filled buttons");
  assert.equal(text(v), "2 to install · 1 to remove · ~4 min for 2, 1 never timed here");
  assert.deepEqual(v.hint.map((h) => h.tone), ["strong", "remove", undefined]);
});

test("the button names one kind by its name, several as changes", () => {
  assert.equal(planSub(["install"]), "1 install");
  assert.equal(planSub(["upgrade", "upgrade"]), "2 updates");
  assert.equal(planSub(["install", "upgrade", "upgrade"]), "3 changes");
});

test("a removal and a reinstall are always named on the button", () => {
  // Apply honours the rows you unchecked: a count that hid them would be the label lying. A
  // reinstall removes the package first, so it is named for the same reason.
  assert.equal(planSub(["uninstall"]), "1 removal");
  assert.equal(planSub(["install", "upgrade", "uninstall", "uninstall"]), "2 changes · 2 removals");
  assert.equal(planSub(["install", "downgrade"]), "1 install · 1 reinstall");
  const v = barView({ ...open, actions: ["downgrade"] });
  assert.equal(text(v), "1 to reinstall");
  assert.equal(v.hint[0].tone, "remove");
});

test("the line follows a fixed order, whatever order the plan came in", () => {
  const v = barView({ ...open, actions: ["uninstall", "upgrade", "install"] });
  assert.equal(text(v), "1 to install · 1 to update · 1 to remove");
});

// --- nothing to do: Quit takes the fill ------------------------------------------

test("nothing to send: Apply goes ghost and Quit is the filled one", () => {
  const v = barView({ ...open, actions: [] });
  assert.equal(v.sub, "nothing to do");
  assert.equal(v.live, false);
  assert.equal(v.disabled, true);
  assert.equal(v.quitPrimary, true);
  assert.equal(text(v), IDLE_HINT);
});

// --- the machine being read: no verdict yet --------------------------------------

test("while scanning, nothing is offered and Quit is not lit", () => {
  // An un-probed row must never count as absent, so an empty plan here proves nothing.
  const v = barView({ ...open, scanning: true, scan: { seen: 3, total: 5 }, actions: [] });
  assert.equal(v.sub, "scanning…");
  assert.equal(v.disabled, true);
  assert.equal(v.quitPrimary, false);
  assert.equal(text(v), "Reading this machine — 3 of 5 packages…");
  assert.equal(text(barView({ ...open, scanning: true })), "Reading this machine…");
});

test("before the socket opens and after it drops, nothing is offered either", () => {
  const a = barView({ conn: "connecting", actions: ["install"] });
  assert.equal(a.sub, "connecting…");
  assert.equal(a.disabled, true);
  assert.equal(a.quitPrimary, false);
  const b = barView({ conn: "closed", actions: [] });
  assert.equal(b.sub, "disconnected");
  assert.equal(b.quitPrimary, false);
});

// --- a run: the button says how far, the line says what --------------------------

test("an Apply: how far on the button, the row running now in the line", () => {
  const v = barView({
    ...open,
    running: { kind: "apply", done: 1, total: 3, current: { name: "Gamma", status: "installing" } },
  });
  assert.equal(v.label, "Applying");
  assert.equal(v.sub, "1 of 3");
  assert.equal(v.disabled, true);
  assert.equal(v.quitPrimary, false);
  assert.equal(text(v), "Installing Gamma · 1 of 3 · Stop is on its row");
  const rm = barView({ ...open, running: { kind: "apply", done: 2, total: 2, current: { name: "Delta", status: "uninstalling" } } });
  assert.equal(rm.hint[0].text, "Removing Delta");
});

test("an Apply before its plan is known says it is checking, not 0 of 0", () => {
  const v = barView({ ...open, running: { kind: "apply", done: 0, total: 0, current: null } });
  assert.equal(v.sub, "starting…");
  assert.equal(text(v), "Checking this machine before acting…");
});

test("a transient word overrides the progress until the server answers", () => {
  const v = barView({ ...open, running: { kind: "apply", word: "stopping…", done: 1, total: 3, current: null } });
  assert.equal(v.sub, "stopping…");
});

test("a paused Apply says it waits for an answer", () => {
  const v = barView({ ...open, running: { kind: "apply", paused: true, done: 1, total: 3, current: null } });
  assert.equal(v.label, "Paused");
  assert.equal(v.sub, "waiting for you");
});

test("a diff and a retry are named for what they are, not as an Apply", () => {
  assert.equal(barView({ ...open, running: { kind: "diff", done: 0, total: 0 } }).label, "Diffing");
  assert.equal(barView({ ...open, running: { kind: "retry", done: 0, total: 0 } }).label, "Retrying");
});

test("a diff and a retry have no plan: their row is what the bar names, never 'starting…'", () => {
  // Found by the in-branch review: neither sends an `apply-plan`, so "starting…" stayed on the
  // button from start to end.
  const d = barView({ ...open, running: { kind: "diff", row: "Delta", done: 0, total: 0, current: null } });
  assert.equal(d.sub, "Delta");
  assert.equal(text(d), "Diffing Delta…");
  const r = barView({
    ...open,
    running: { kind: "retry", row: "Gamma", done: 0, total: 0, current: { name: "Gamma", status: "installing" } },
  });
  assert.equal(r.sub, "Gamma");
  assert.equal(text(r), "Installing Gamma · Stop is on its row");
});

test("an Apply the engine found nothing for says so once, before the rest of the line", () => {
  const empty = barView({ ...open, actions: [], note: "The engine found nothing to do." });
  assert.equal(text(empty), "The engine found nothing to do.");
  const stale = barView({ ...open, actions: ["install"], note: "The engine found nothing to do." });
  assert.equal(text(stale), "The engine found nothing to do. · 1 to install");
});

// --- the wiring -----------------------------------------------------------------

test("app.js paints the bar from bar.js, and the old status word is gone", () => {
  const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
  assert.match(appjs, /import \{ barView \} from "\.\/bar\.js";/);
  assert.match(appjs, /classList\.toggle\("primary", v\.quitPrimary\)/);
  // A row that starts clears the transient word ("retrying…"), or it masks the progress for
  // the rest of the run; and the plan's arrival repaints, or "starting…" outlives it.
  const setStatus = appjs.slice(appjs.indexOf("function setStatus("), appjs.indexOf("\n}\n", appjs.indexOf("function setStatus(")));
  assert.match(setStatus, /barWord = "";/);
  const planCase = appjs.slice(appjs.indexOf('case "apply-plan":'), appjs.indexOf('case "apply-plan-add":'));
  assert.match(planCase, /paintBar\(\)/);
  // `#overall` said "ready" / "applying…" beside a button that now says more; a write to it
  // that survived would be a TypeError on the first click.
  assert.doesNotMatch(appjs, /\boverall\b/);
  assert.doesNotMatch(html, /id="overall"/);
  // The per-bundle Apply left with the one-path rule; its CSS went with this branch.
  assert.doesNotMatch(html, /\.bundle-head \.applybtn/);
});
