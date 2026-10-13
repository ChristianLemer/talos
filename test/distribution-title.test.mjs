// The window title and the footer line, as data (model.js `windowTitle`, `buildLine`):
// the distribution leads when the content has one, the engine's stamp follows, and
// without a distribution both read exactly as every release before did. Pure — no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildLine, windowTitle } from "../public/model.js";

const build = { tag: "v0.0.1-beta.44", sha: "1643d1b", builtAt: "2026-10-13T06:06:20Z" };
const dist = { name: "Data management", version: "2026.10", label: "Data management 2026.10" };

test("the title leads with the distribution, then the engine's version", () => {
  assert.equal(windowTitle(dist, build.tag), "Data management 2026.10 · Talos v0.0.1-beta.44");
});

test("without a distribution the title is the one every release had", () => {
  assert.equal(windowTitle(null, build.tag), "Talos · v0.0.1-beta.44");
  assert.equal(windowTitle(undefined, build.tag), "Talos · v0.0.1-beta.44");
});

test("the footer names the distribution before the stamp, or just the stamp", () => {
  assert.equal(
    buildLine(dist, build),
    "Data management 2026.10 · v0.0.1-beta.44 · 1643d1b — 2026-10-13T06:06:20Z",
  );
  assert.equal(buildLine(null, build), "v0.0.1-beta.44 · 1643d1b — 2026-10-13T06:06:20Z");
});
