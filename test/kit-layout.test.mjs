// The kit's LAYOUT is stated three times, in three languages, and nothing tied them
// together until this file.
//
// ⭐ WHY THIS EXISTS, measured rather than feared. The layout changed on 2026-09-11: each
// launcher moved into the folder of its OS. Both scripts were updated; the skill that
// DESCRIBES the kit was not, and it went on drawing `Talos.app` at the root of the folder.
// Nobody broke anything — the prose simply aged, exactly as `bundles.rs`'s authoring-
// reference guard says prose does. What makes it worse here than in a README is the
// consumer: `talos-kit` is carried by an agent, and a stale skill gets ACTED ON rather
// than doubted.
//
// ⚠️ IT COMPARES SETS, IN BOTH DIRECTIONS — and that is the difference from the
// authoring-reference guard, which only checks that a key is MENTIONED. A mention check
// would have passed the failure above: `Talos.app` was mentioned, at the wrong place. What
// aged was not an absence, it was a survivor. So an extra folder in the drawing fails just
// as loudly as a missing one.
//
// ⚠️ IT READS THE FENCED BLOCK, NEVER THE PROSE. Agreed with the session that owns the
// plugin, and it is the right line: the prose around the tree must stay free to be
// rewritten ("one folder per OS so each launcher keeps its standard name"), and a test that
// greps prose either breaks on a rewording or forces an author to write for a parser. The
// tree in the fence is a drawing of a structure; the sentences beside it are an argument.
// Only the drawing is under test.
//
// ⚠️ Every extraction asserts that it FOUND something, and says which anchor moved when it
// did not. An extractor that silently returns nothing turns this file into a test that
// compares two empty sets and passes — which would retire the question without answering
// it.
//
// It does NOT check that any of the three is CORRECT; no test can. It checks that the three
// agree. Whether the layout is a good one stays a reviewer's job.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (p) => readFileSync(new URL(p, import.meta.url), "utf8").replace(/\r/g, "");
const sh = read("../admin/get-talos.sh");
const ps1 = read("../admin/get-talos.ps1");
const skill = read("../plugin/skills/talos-kit/SKILL.md");

const set = (xs) => new Set(xs);
const sorted = (s) => [...s].sort();

/** The folders `get-talos.sh` treats as the kit's own: the manifest walk, plus the ones it
 *  creates outright. `catalog/` and `bundles/` are absent from the mkdir on purpose — they
 *  arrive by `cp -R` or by unzip — so neither anchor alone is the whole layout. */
function layoutFromSh() {
  const walk = sh.match(/for d in ([^;]+); do/);
  assert.ok(walk, "get-talos.sh: the manifest walk `for d in … ; do` moved — re-point this test");
  const mkdir = sh.match(/^mkdir -p ((?:"\$DEST\/[^"]+"\s*)+)$/m);
  assert.ok(mkdir, 'get-talos.sh: the `mkdir -p "$DEST/…"` line moved — re-point this test');
  const created = [...mkdir[1].matchAll(/"\$DEST\/([^"/]+)"/g)].map((m) => m[1]);
  return set([...walk[1].trim().split(/\s+/), ...created]);
}

/** The same, from `get-talos.ps1`: the array `Get-KitFileList` walks, plus `.talos`. */
function layoutFromPs1() {
  const walk = ps1.match(/foreach \(\$n in @\(((?:\s*'[^']+',?)+)\)\) \{\s*\n\s*\$dir = Join-Path \$Root \$n/);
  assert.ok(walk, "get-talos.ps1: the Get-KitFileList array moved — re-point this test");
  const dot = ps1.match(/\$dotTalos = Join-Path \$KitFull '([^']+)'/);
  assert.ok(dot, "get-talos.ps1: the $dotTalos assignment moved — re-point this test");
  const names = [...walk[1].matchAll(/'([^']+)'/g)].map((m) => m[1]);
  return set([...names, dot[1]]);
}

/** The top level of the tree DRAWN in the skill's fenced block. Nested lines are indented
 *  and belong to a folder already counted, so only column-zero branches count. */
function layoutFromSkill() {
  const fence = skill.match(/```\n<the kit folder>\n([\s\S]*?)```/);
  assert.ok(
    fence,
    "talos-kit/SKILL.md: the fenced tree opening with `<the kit folder>` moved or lost its " +
      "fence — this test reads the drawing, not the prose, so re-point it rather than " +
      "letting it match nothing",
  );
  const tops = [...fence[1].matchAll(/^[├└]── ([^\s/]+)/gm)].map((m) => m[1]);
  return set(tops);
}

test("kit layout: both scripts compose the same set of folders", () => {
  const fromSh = layoutFromSh();
  const fromPs1 = layoutFromPs1();
  assert.equal(fromSh.size, 6, `get-talos.sh: expected 6 kit folders, got ${sorted(fromSh)}`);
  assert.deepEqual(
    sorted(fromSh),
    sorted(fromPs1),
    "get-talos.sh and get-talos.ps1 no longer compose the same kit — the whole point of " +
      "having two scripts is that the folder does not record which machine made it",
  );
});

test("kit layout: talos-kit draws exactly what the scripts compose", () => {
  const fromSh = layoutFromSh();
  const fromSkill = layoutFromSkill();
  assert.deepEqual(
    sorted(fromSkill),
    sorted(fromSh),
    "the tree drawn in plugin/skills/talos-kit/SKILL.md is not the kit the scripts compose. " +
      "An agent reads that skill and acts on it, so a drawing that has aged is worse than " +
      "no drawing: update the fence, or the scripts, so the two agree",
  );
});
