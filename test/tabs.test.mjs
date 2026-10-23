// tabs.test.mjs — the tab bar: one icon per tab, flat and colourless, and History apart
// from Settings (what Talos DID here, read-only, vs what the user SETS).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
const nav = html.slice(html.indexOf('<nav class="tabs"'), html.indexOf("</nav>"));

// The slice of markup from a view's opening tag to the next view (or the scripts).
function view(id) {
  const start = html.indexOf(`<div id="${id}" class="view`);
  assert.notEqual(start, -1, `#${id} must exist`);
  const next = html.indexOf('class="view', html.indexOf(">", start));
  const end = next === -1 ? html.indexOf("<script", start) : html.lastIndexOf("<div", next);
  return html.slice(start, end);
}

function fnBody(name) {
  const body = appjs.slice(appjs.indexOf(`function ${name}`));
  return body.slice(0, body.indexOf("\n}\n"));
}

test("the tabs, in order, each with its label", () => {
  const tabs = [...nav.matchAll(/<button class="tab[^"]*" data-view="(\w+)">([\s\S]*?)<\/button>/g)]
    .map((m) => [m[1], m[2].replace(/<svg[\s\S]*?<\/svg>/, "").trim()]);
  assert.deepEqual(tabs, [
    ["bundles", "Changes"],
    ["catalog", "Catalog"],
    ["arbitration", "Arbitration"],
    ["machine", "Machine"],
    ["log", "History"],
    ["settings", "Settings"],
    ["doctor", "Doctor"],
  ]);
});

test("every tab carries an icon drawn from the one sprite", () => {
  const buttons = [...nav.matchAll(/<button class="tab[\s\S]*?<\/button>/g)].map((m) => m[0]);
  assert.equal(buttons.length, 7);
  for (const b of buttons) {
    const use = b.match(/<svg class="ti" aria-hidden="true"><use href="#(i-[\w-]+)"\/><\/svg>/);
    assert.ok(use, `no icon in ${b}`);
    assert.match(html, new RegExp(`<symbol id="${use[1]}" viewBox="0 0 24 24">`), use[1]);
  }
});

test("the icons are flat: no colour of their own, they take the tab's", () => {
  const sprite = html.slice(html.indexOf('<svg id="tab-icons"'), html.indexOf("</svg>", html.indexOf('<svg id="tab-icons"')));
  assert.ok(sprite.length > 0, "the sprite must exist");
  assert.doesNotMatch(sprite, /(fill|stroke)="(?!none|currentColor)/);
  assert.match(html, /\.ti \{[^}]*stroke:currentColor/);
  assert.match(html, /\.ti \{[^}]*fill:none/);
});

test("the icons' licence travels with them", () => {
  assert.match(html, /Lucide/);
  assert.match(html, /ISC License/);
  assert.match(html, /Permission to use, copy, modify, and\/or distribute this software/);
});

test("History holds the journal and its delete; Settings holds the settings", () => {
  const history = view("view-log");
  assert.match(history, /id="history"/);
  assert.match(history, /id="clear-log"/);
  assert.doesNotMatch(history, /id="settings"/);
  const settings = view("view-settings");
  assert.match(settings, /id="settings"/);
  assert.match(settings, /id="consent-toggle"/);
  assert.doesNotMatch(settings, /id="clear-log"/);
  assert.doesNotMatch(settings, /id="history"/);
});

test("the share destination is said under the share switch too", () => {
  const settings = view("view-settings");
  assert.ok(settings.indexOf('id="share-where"') > settings.indexOf('id="consent-toggle"'), "below the switch");
  assert.match(settings, /id="share-where"/);
  assert.match(fnBody("renderLog"), /share-where/);
  assert.match(fnBody("renderLog"), /shareLine/);
});

test("setTab swaps to the Settings view and asks for the records it shows", () => {
  const fn = fnBody("setTab");
  assert.match(fn, /view-settings/);
  assert.match(fn, /isSettings/);
});
