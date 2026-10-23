// The "This machine" card of the Log tab. Pure — no DOM, no server.
// Run: node --test test/machine.test.mjs   (from talos/)
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  formatMb,
  formatMs,
  machineRows,
  scanSummary,
  shareLine,
  SLOW_SHELL_MS,
} from "../public/machine.js";

const windows = {
  kind: "machine",
  os: "Windows 11 Enterprise",
  arch: "x86_64",
  cpu: "Intel Core i5",
  cores: 8,
  ram_total_mb: 16384,
  ram_available_mb: 6000,
  system_disk: { mount: "C:\\", free_mb: 51200, total_mb: 256000 },
  kit: { path: "~\\OneDrive - Corp\\Talos\\Windows", location: "onedrive" },
  shell_ms: 2400,
  tools: [
    { name: "winget", path: null },
    { name: "git", path: "C:\\Program Files\\Git\\cmd\\git.exe" },
  ],
  winget: { alias: false, on_path: true },
  antivirus: ["Windows Defender"],
  realtime: true,
  power: "battery",
};

test("units read like a person would say them", () => {
  assert.equal(formatMs(850), "850 ms");
  assert.equal(formatMs(4200), "4.2 s");
  assert.equal(formatMb(512), "512 MB");
  assert.equal(formatMb(16384), "16.0 GB");
});

test("the share line says where the copy goes, or that there is none", () => {
  assert.match(shareLine({ share: true }, "~/kit/logs/h/u.jsonl"), /~\/kit\/logs\/h\/u\.jsonl/);
  assert.match(shareLine({ share: false }, "x"), /this machine only/);
  assert.match(shareLine(null, "x"), /this machine only/);
});

test("no profile yet means no rows, not a crash", () => {
  assert.deepEqual(machineRows(null), []);
});

test("every collected fact becomes a row, and the suspicious ones are flagged", () => {
  const rows = machineRows(windows);
  const by = Object.fromEntries(rows.map((r) => [r.label, r]));
  assert.equal(by.CPU.value, "Intel Core i5 · 8 logical cores");
  assert.equal(by.Memory.value, "5.9 GB free of 16.0 GB");
  assert.ok(by.Kit.warn, "a OneDrive kit is flagged");
  assert.ok(by["One shell"].warn, `${windows.shell_ms} ms ≥ ${SLOW_SHELL_MS} is flagged`);
  assert.equal(by.winget.value, "not found");
  assert.ok(by.winget.warn);
  assert.ok(!by.git.warn);
  assert.match(by["winget alias"].value, /NOT registered/);
  assert.ok(by["winget alias"].warn);
  assert.equal(by.Antivirus.value, "Windows Defender · Defender real-time on");
  assert.equal(by.Power.value, "on battery");
});

test("a quiet local machine raises no flag", () => {
  const rows = machineRows({
    ...windows,
    kit: { path: "C:\\Tools\\Talos", location: "local" },
    shell_ms: 300,
    tools: [{ name: "git", path: "C:\\git.exe" }],
    winget: { alias: true, on_path: true },
    power: "ac",
  });
  assert.deepEqual(rows.filter((r) => r.warn), []);
});

test("the scan line names its total, its slowest probe and what was unavailable", () => {
  const line = scanSummary({
    total_ms: 42000,
    packages: 19,
    probes: [
      { name: "jq", ms: 900 },
      { name: "Docker", ms: 12000 },
    ],
    unavailable: ["outdated: the upgrade scan could not run: program not found"],
  });
  assert.match(line, /42\.0 s for 19 packages/);
  assert.match(line, /slowest: Docker \(12\.0 s\)/);
  assert.match(line, /program not found/);
  assert.equal(scanSummary(null), "");
});

test("a failed collection is one flagged row with its reason", () => {
  const rows = machineRows({ kind: "machine", error: "the collection did not finish in 60 s" });
  assert.deepEqual(rows, [
    { label: "Profile", value: "the collection did not finish in 60 s", warn: true },
  ]);
});

test("a scan that overlapped the collection says so", () => {
  const base = { total_ms: 1000, packages: 1, probes: [] };
  assert.doesNotMatch(scanSummary(base), /machine profile/);
  assert.match(scanSummary({ ...base, profile_running: true }), /while the machine profile/);
});

test("the card has a tab of its own: read-only facts, apart from the settings", () => {
  const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");
  assert.match(html, /<button class="tab" data-view="machine">[\s\S]*?Machine<\/button>/);
  const card = html.indexOf('id="machine-card"');
  const view = html.indexOf('id="view-machine"');
  assert.notEqual(card, -1, "#machine-card must exist");
  assert.notEqual(view, -1, "#view-machine must exist");
  assert.ok(card > view, "inside the Machine view");
  assert.ok(card < html.indexOf('id="view-log"'), "not inside the History view");
});

test("an open Machine tab waiting for the profile asks again, since nothing pushes it", () => {
  const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const body = appjs.slice(appjs.indexOf("function renderMachine"));
  const fn = body.slice(0, body.indexOf("\n}\n"));
  assert.match(fn, /view-machine/);
  assert.match(fn, /setTimeout/);
  assert.match(fn, /get-log/);
});

test("the Machine tab asks for the records when it opens, as the Log tab does", () => {
  const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const body = appjs.slice(appjs.indexOf("function setTab"));
  const fn = body.slice(0, body.indexOf("\n}\n"));
  assert.match(fn, /view-machine/);
  assert.match(fn, /isMachine/);
  assert.match(fn, /get-log/);
});

test("policies are shown as stored, one row each; none set is said, not read is silent", () => {
  const base = { os: "windows", cores: 1, ram_total_mb: 1, ram_available_mb: 1, shell_ms: 1 };
  const rows = machineRows({
    ...base,
    policies: [
      { name: "AppInstaller.EnableDefaultSource", value: "0" },
      { name: "WindowsStore.RequirePrivateStoreOnly", value: "1" },
    ],
  });
  const pol = rows.filter((r) => r.label === "Policy");
  assert.deepEqual(pol.map((r) => r.value), [
    "AppInstaller.EnableDefaultSource = 0",
    "WindowsStore.RequirePrivateStoreOnly = 1",
  ]);
  // A fact, not a verdict: what a policy blocks is the content's to say.
  assert.ok(pol.every((r) => !r.warn));
  const none = machineRows({ ...base, policies: [] }).find((r) => r.label === "Policies");
  assert.equal(none.value, "none set");
  assert.ok(!machineRows(base).some((r) => /^Polic/.test(r.label)));
});

test("a user PATH stored as REG_SZ is flagged only when it names a %variable%", () => {
  const base = { os: "windows", cores: 1, ram_total_mb: 1, ram_available_mb: 1, shell_ms: 1 };
  const pathRow = (m) => machineRows({ ...base, ...m }).find((r) => r.label === "User PATH");
  const sz = pathRow({ user_path_kind: "String", user_path_vars: true });
  assert.equal(sz.warn, true);
  assert.match(sz.value, /^REG_SZ — /);
  assert.match(sz.value, /never expanded/);
  // REG_SZ of absolute paths only loses nothing: said, not flagged.
  assert.deepEqual(pathRow({ user_path_kind: "String", user_path_vars: false }), {
    label: "User PATH", value: "REG_SZ", warn: false,
  });
  const ok = machineRows({ ...base, user_path_kind: "ExpandString" }).find((r) => r.label === "User PATH");
  assert.deepEqual(ok, { label: "User PATH", value: "REG_EXPAND_SZ", warn: false });
  assert.ok(!machineRows(base).some((r) => r.label === "User PATH"));
});

test("machine data reaches the page as text, never as markup", () => {
  // A path or an antivirus name is machine data: rendered with textContent, so a `<` in
  // it can never become an element.
  const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
  const body = appjs.slice(appjs.indexOf("function renderMachine"));
  const fn = body.slice(0, body.indexOf("\n}\n"));
  assert.ok(fn.length > 0, "renderMachine must exist");
  assert.ok(!fn.includes("innerHTML"), "renderMachine must not use innerHTML");
});
