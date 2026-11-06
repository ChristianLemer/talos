// The release's asset names (#3) are written in four places that must agree: the workflow that
// publishes them, the two kit scripts that fetch them, and the release page that names them.
// One target-triple name per launcher. The workflow publishes it alone since 0.1.0; the scripts
// keep the betas' names as a fallback, so a pin on a beta still composes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

// The files the build jobs upload: the lines of their `artifacts: |` lists.
const published = (wf) =>
  [...wf.matchAll(/artifacts: \|\n((?: {14}\S+\n)+)/g)].flatMap((m) => m[1].trim().split(/\s+/));
const read = (p) => readFileSync(new URL(`../${p}`, import.meta.url), "utf8");
const workflow = read(".github/workflows/release.yml");
const sh = read("admin/get-talos.sh");
const ps1 = read("admin/get-talos.ps1");
const notes = read("admin/release-notes.sh");
const ci = read("plugin/template/.github/workflows/talos-check.yml");

const TRIPLES = [
  ["aarch64-apple-darwin.app.zip", "Talos-macos-aarch64.app.zip"],
  ["x86_64-pc-windows-msvc.exe", "Talos.exe"],
  ["x86_64-unknown-linux-gnu", "Talos-linux-x86_64"],
];

test("every launcher is published, fetched and named under one target-triple name", () => {
  for (const [triple, legacy] of TRIPLES) {
    assert.ok(workflow.includes(`Talos-*-${triple}`), `the workflow publishes ${triple}`);
    assert.ok(!published(workflow).some((f) => f.endsWith(legacy)), `the workflow no longer publishes ${legacy}`);
    assert.ok(sh.includes(`"Talos-$VER-${triple}" ${legacy}`), `get-talos.sh: ${triple}, else ${legacy}`);
    assert.ok(ps1.includes(`"Talos-$ver-${triple}", '${legacy}'`), `get-talos.ps1: ${triple}, else ${legacy}`);
    assert.ok(notes.includes(`Talos-$VER-${triple}`), `the release page names ${triple}`);
  }
  assert.ok(workflow.includes("talos-content-*.zip"));
  assert.ok(!published(workflow).includes("talos-content.zip"), "the workflow no longer publishes talos-content.zip");
  assert.equal(published(workflow).length, 6, "three launchers, the content, two kit scripts");
  assert.ok(sh.includes(`"talos-content-$VER.zip" talos-content.zip`));
  assert.ok(ps1.includes(`"talos-content-$ver.zip", 'talos-content.zip'`));
});

test("a content repo's CI finds the engine under either name", () => {
  assert.match(ci, /Talos-\$\{TAG#v\}-x86_64-unknown-linux-gnu/);
  assert.match(ci, /\|\| curl -fsSL -o talos "\$BASE\/Talos-linux-x86_64"/);
});

test("the release page is written once every build is in, even when one failed", () => {
  assert.match(workflow, /notes:\n\s+needs: build\n\s+if: \$\{\{ !cancelled\(\) \}\}/);
  assert.match(workflow, /sh admin\/release-notes\.sh "\$GITHUB_REF_NAME" > notes\.md/);
});
