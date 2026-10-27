// The boot splash covers the scan until `state-done`, and only a scan that STOPS ANSWERING lifts
// it early. A fixed 2-minute deadline lifted it in the middle of a slow Windows scan and left the
// rows saying "checking…" with nothing to say why. Read as text: app.js is DOM-bound and imported
// by no test (see one-path-ui.test.mjs).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");

const body = (name) => {
  const at = appjs.indexOf(`function ${name}(`);
  assert.notEqual(at, -1, `${name} is gone`);
  return appjs.slice(at, appjs.indexOf("\n}\n", at));
};

test("no fixed deadline hides the splash: only state-done and the stall detector call hideSplash", () => {
  assert.doesNotMatch(appjs, /setTimeout\(hideSplash/, "a timer hides the splash whatever the scan is doing");
  const callers = [...appjs.matchAll(/(?<!function )hideSplash\(\)/g)].length;
  assert.equal(callers, 2, "hideSplash has a caller besides state-done and splashStalled");
  const done = appjs.slice(appjs.indexOf('case "state-done":'));
  assert.match(done.slice(0, done.indexOf("break;")), /hideSplash\(\);/);
});

test("every verdict re-arms the stall timer", () => {
  assert.match(body("splashProgress"), /clearTimeout\(splashStall\);\s*splashStall = setTimeout\(splashStalled, SPLASH_STALL_MS\);/);
  const ms = Number(appjs.match(/const SPLASH_STALL_MS = (\d+);/)?.[1]);
  assert.ok(ms >= 300000, `a stall is minutes without one verdict, not ${ms} ms`);
});

test("a stall says so before the splash lifts, and state-done takes the line back", () => {
  const stalled = body("splashStalled");
  assert.ok(stalled.indexOf('"scan-warn"') < stalled.indexOf("hideSplash()"), stalled);
  assert.match(stalled, /classList\.add\("show"\)/);
  assert.match(stalled, /checking…/, "the line names what the rows still say");
  const done = appjs.slice(appjs.indexOf('case "state-done":'));
  assert.match(done.slice(0, done.indexOf("break;")), /"scan-warn"\)\?\.classList\.remove\("show"\)/);
  assert.match(html, /<div id="scan-warn" class="warn-banner"><\/div>/);
});

test("an engine that closes the socket mid-scan lifts the splash with the same line, at once", () => {
  const close = appjs.slice(appjs.indexOf("ws.onclose = () => {"));
  assert.match(close.slice(0, close.indexOf("\n};")), /splashStalled\("the engine closed the connection"\);/);
});
