// The Doctor's dropdown as data (model.js `doctorChoices`): present candidates in
// catalogue order, the OS shell last and always, the remembered choice honoured only
// while present, absent candidates listed apart. Pure — no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { doctorChoices } from "../public/model.js";

const agents = [
  { name: "Claude Code", present: true, clean: true },
  { name: "Codex", present: false, clean: true },
  { name: "Nushell", present: true, clean: true },
];

test("present candidates in catalogue order, then the shell as the floor", () => {
  const { options } = doctorChoices(agents, "zsh", "");
  assert.deepEqual(
    options.map((o) => o.value),
    ["Claude Code", "Nushell", "shell"]
  );
  assert.equal(options.at(-1).clean, false, "the floor never offers a clean variant");
});

test("the remembered choice wins while present, else the first present", () => {
  assert.equal(doctorChoices(agents, "zsh", "Nushell").selected, "Nushell");
  assert.equal(doctorChoices(agents, "zsh", "Codex").selected, "Claude Code");
  assert.equal(doctorChoices(agents, "zsh", "").selected, "Claude Code");
});

test("absent candidates are named apart — the Doctor points, it does not install", () => {
  assert.deepEqual(doctorChoices(agents, "zsh", "").absent, ["Codex"]);
});

test("with no agent present the shell is the only choice, and the selected one", () => {
  const none = agents.map((a) => ({ ...a, present: false }));
  const r = doctorChoices(none, "powershell.exe", "Claude Code");
  assert.deepEqual(r.options.map((o) => o.value), ["shell"]);
  assert.equal(r.selected, "shell");
  assert.deepEqual(r.absent, ["Claude Code", "Codex", "Nushell"]);
});

test("garbage in, empty out — never a crash on the rescue path", () => {
  const r = doctorChoices(undefined, "", "x");
  assert.deepEqual(r, { options: [], absent: [], selected: "" });
});

import { doctorSessionLabel, doctorNextActive } from "../public/model.js";

test("a session tab says its number, what runs, and whether it runs clean", () => {
  assert.equal(doctorSessionLabel("Claude Code", false, "zsh", 1), "1 · Claude Code");
  assert.equal(doctorSessionLabel("Claude Code", true, "zsh", 2), "2 · Claude Code · clean");
  assert.equal(doctorSessionLabel("shell", false, "powershell.exe", 3), "3 · powershell.exe");
  assert.equal(doctorSessionLabel("shell", true, "", 4), "4 · shell · clean");
  assert.equal(doctorSessionLabel("Codex", false, "zsh"), "Codex", "no number, no prefix");
});

test("closing a tab: the active one stays unless it is the closed one", () => {
  assert.equal(doctorNextActive([1, 2, 3], 1, 3), 1);
  assert.equal(doctorNextActive([1, 2, 3], 2, 2), 3, "right-hand neighbour first");
  assert.equal(doctorNextActive([1, 2, 3], 3, 3), 2, "then the left one");
  assert.equal(doctorNextActive([1], 1, 1), null, "nothing left");
  assert.equal(doctorNextActive([1, 2], 9, 2), 1, "a stale active falls back to what remains");
});
