// server.js — the engine: an imperative host that runs DECLARATIVE bundles.
//
// A bundle is a FOLDER (bundle.yaml + optional config/). The engine scans two
// places — talos/bundles/ (the Base, inside the coque) and ../bundles/ (extras
// and integrator bundles, OUTSIDE the coque) — parses each bundle.yaml, and
// builds a flat list of packages tagged by bundle. The engine knows no specific
// tool: it runs whatever bundles are present (host/plugins model).
//
// Each package declares a package manager as a NAMED field (winget: / npm:) —
// winget is winget, known; we don't abstract into opaque command strings. The
// install/uninstall commands are DERIVED from that field. `requires:` lists
// dependencies. `detect:` is used to capture the installed version (not to
// decide whether to act — winget is idempotent and decides that itself).
//
// One language owns the loop here (Node). Bundles carry no logic; if a step
// needs logic, use a `nu -c "…"` package (nushell as surgical instrument).
//
// Events over one WebSocket (pure JSON, no prefix):
//   {type:"plan", steps, bundles}     — render cards + rows
//   {type:"step", i, status}          — waiting | installing | uninstalling | ok | absent | fail
//   {type:"out",  i, data}            — raw pty bytes (base64), tagged by step
//   {type:"done"}

import http from "node:http";
import { readFileSync, readdirSync, existsSync, mkdirSync, appendFileSync, writeFileSync, rmSync, createWriteStream } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { execFile } from "node:child_process";
import os from "node:os";
import { WebSocketServer } from "ws";
import { parse as parseYaml } from "yaml";
import pty from "node-pty";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, "..");        // talos/  (the machinery)
const PORT = 7682;
const isWindows = process.platform === "win32";
const HOST = os.hostname();
const USER = (os.userInfo().username || "user").replace(/[^\w.-]/g, "_");

// --- local state (per machine, outside the coque) --------------------------
// Consent + private history live here. The repo stays generic; nothing
// personal is written into it.
const STATE_DIR = join(process.env.LOCALAPPDATA || process.env.HOME || os.tmpdir(), "Talos");
const CONSENT_FILE = join(STATE_DIR, "consent.json");
// Shared/collective history sits NEXT TO the system (repo root), so the disk's
// topology decides reach: personal disk → just you; collective disk → the team.
const SHARED_LOG_DIR = join(ROOT, "..", "logs", HOST);
const SHARED_LOG = join(SHARED_LOG_DIR, `${USER}.jsonl`);
const PRIVATE_LOG = join(STATE_DIR, "history.jsonl");

function ensureDir(d) { try { mkdirSync(d, { recursive: true }); } catch {} }
// Consent: { decided: bool, share: bool }. opt-in by default (share=false).
function readConsent() {
  try { return JSON.parse(readFileSync(CONSENT_FILE, "utf8")); } catch { return { decided: false, share: false }; }
}
function writeConsent(c) { ensureDir(STATE_DIR); try { writeFileSync(CONSENT_FILE, JSON.stringify(c, null, 2)); } catch (e) { log(`consent write failed: ${e.message}`); } }

// Append one history entry. Shared file if consented, else private to this
// machine — the Log tab works either way; only the LOCATION (and thus who can
// see it) changes. Talos never sends anything outbound — it just writes a file.
function appendHistory(entry) {
  const line = JSON.stringify({ ...entry, host: HOST, user: USER }) + "\n";
  const share = readConsent().share;
  const target = share ? SHARED_LOG : PRIVATE_LOG;
  ensureDir(dirname(target));
  try { appendFileSync(target, line); } catch (e) { log(`history write failed: ${e.message}`); }
}
function readHistory() {
  const share = readConsent().share;
  const target = share ? SHARED_LOG : PRIVATE_LOG;
  try { return readFileSync(target, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l)); }
  catch { return []; }
}
function clearHistory() {
  for (const f of [SHARED_LOG, PRIVATE_LOG]) { try { rmSync(f); } catch {} }
}

// --- trace, since node runs hidden (no console to watch) -------------------
const logFile = createWriteStream(join(ROOT, "panel.log"), { flags: "a" });
function log(msg) {
  const line = `${new Date().toISOString()}  ${msg}`;
  logFile.write(line + "\n");
  console.log(line);
}
log(`--- panel start (pid ${process.pid}, platform ${process.platform}) ---`);

// --- bundles: pure data, no logic ------------------------------------------
// Two scan roots: the Base inside the coque, then extras outside it. Each
// subfolder with a bundle.yaml is a bundle. A broken YAML is skipped with a
// logged message, not defended against (robust, not maternal).
const BUNDLE_ROOTS = [
  join(ROOT, "bundles"),            // coque — the Base
  join(ROOT, "..", "bundles"),      // outside the coque — extras / integrator
];

// Build a package manager's install/uninstall command from a package's named
// field. Adding a manager = adding a case here; data files stay clean.
function commandsFor(pkg) {
  if (pkg.winget) return {
    install: `winget install --id ${pkg.winget} -e --accept-source-agreements --accept-package-agreements`,
    uninstall: `winget uninstall --id ${pkg.winget} -e`,
  };
  if (pkg.npm) return {
    install: `npm install -g ${pkg.npmFlags ? pkg.npmFlags + " " : ""}${pkg.npm}`,
    uninstall: `npm uninstall -g ${pkg.npm}`,
  };
  if (pkg.run) return { install: pkg.run, uninstall: pkg.runUninstall || null };  // escape hatch: raw command
  return { install: null, uninstall: null };
}

function loadBundles() {
  const bundles = [];   // { name, emoji, description, priority, selectable }
  const steps = [];     // flat packages, each tagged { bundle, name, install, uninstall, detect, requires, selfHost }
  for (const root of BUNDLE_ROOTS) {
    if (!existsSync(root)) continue;
    for (const dir of readdirSync(root, { withFileTypes: true })) {
      if (!dir.isDirectory()) continue;
      const file = join(root, dir.name, "bundle.yaml");
      if (!existsSync(file)) continue;
      try {
        const b = parseYaml(readFileSync(file, "utf8"));
        const meta = {
          name: b.bundle || dir.name,
          emoji: b.emoji || "📦",
          description: b.description || "",
          priority: b.priority ?? 100,
          selectable: b.selectable !== false,
        };
        bundles.push(meta);
        for (const p of (b.packages || [])) {
          const cmd = commandsFor(p);
          steps.push({
            bundle: meta.name, name: p.name, description: p.description || "",
            install: cmd.install, uninstall: cmd.uninstall,
            detect: p.detect || null, requires: p.requires || [], selfHost: !!p.selfHost,
          });
        }
        log(`bundle loaded: ${meta.name} (${(b.packages || []).length} packages) from ${dir.name}`);
      } catch (e) {
        log(`bundle skipped (bad YAML): ${dir.name} — ${e.message}`);
      }
    }
  }
  bundles.sort((a, b) => a.priority - b.priority);
  return { bundles, steps };
}
const { bundles: BUNDLES, steps: STEPS } = loadBundles();

// winget exit codes that mean "nothing to do" — already installed / already
// latest. Treated as success: the desired state is reached. (Codes are signed
// 32-bit; node-pty reports them as-is.)
const BENIGN_CODES = new Set([
  -1978335189, // APPINSTALLER_CLI_ERROR_NO_APPLICABLE_UPGRADE  (no newer version)
  -1978335212, // APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE   (already installed, no upgrade)
]);

// --- serve the page + VENDORED xterm.js (never a CDN — corporate firewall 403s it)-
const XTERM = join(ROOT, "node_modules", "@xterm", "xterm");
const ROUTES = {
  "/vendor/xterm.js": [join(XTERM, "lib", "xterm.js"), "text/javascript"],
  "/vendor/xterm.css": [join(XTERM, "css", "xterm.css"), "text/css"],
};
const server = http.createServer((req, res) => {
  if (req.url === "/" || req.url === "/index.html") {
    res.writeHead(200, { "content-type": "text/html" });
    res.end(readFileSync(join(ROOT, "public", "index.html")));
  } else if (ROUTES[req.url]) {
    const [file, type] = ROUTES[req.url];
    try {
      res.writeHead(200, { "content-type": type });
      res.end(readFileSync(file));
    } catch {
      log(`vendor asset missing: ${file} — run npm install`);
      res.writeHead(500).end();
    }
  } else {
    res.writeHead(404).end();
  }
});

const wss = new WebSocketServer({ server });
const send = (ws, obj) => { if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(obj)); };

// --- lifecycle: the server lives only as long as the panel is open ----------
// When the user closes the panel, its WebSocket closes. After a grace delay
// (to tolerate a reload/reconnect), if no client is back AND no step is mid-
// install, node exits cleanly. So nobody ever kills a hidden process, no
// ghosts accumulate, and every open starts fresh on the latest code.
let busy = 0;                 // > 0 while a step is running — never quit then
let shutdownTimer = null;
const GRACE_MS = 4000;

function scheduleShutdownIfIdle() {
  if (shutdownTimer) clearTimeout(shutdownTimer);
  shutdownTimer = setTimeout(() => {
    if (wss.clients.size === 0 && busy === 0) {
      log("no client + idle — shutting down");
      logFile.end(() => process.exit(0));
    } else {
      log(`shutdown skipped (clients=${wss.clients.size}, busy=${busy})`);
    }
  }, GRACE_MS);
}

wss.on("connection", (ws) => {
  log("client connected");
  if (shutdownTimer) { clearTimeout(shutdownTimer); shutdownTimer = null; } // cancel any pending quit
  // Draw the plan and STOP — nothing runs on its own. The user drives every
  // action (per-step install/uninstall, or the global buttons). A refuge acts
  // only when asked.
  send(ws, { type: "plan", bundles: BUNDLES, consent: readConsent(),
    steps: STEPS.map((s, i) => ({ i, name: s.name, description: s.description, bundle: s.bundle, canUninstall: !!s.uninstall })) });
  detectAll(ws).catch((e) => log(`detect error: ${e.stack || e}`));   // ground truth → pre-check the cards

  ws.on("message", (raw) => {
    let msg; try { msg = JSON.parse(raw.toString()); } catch { return; }
    if (msg.type === "install" && typeof msg.i === "number" && STEPS[msg.i]) {
      doStep(ws, msg.i, "install").catch((e) => log(`install error: ${e.stack || e}`));
    } else if (msg.type === "uninstall" && typeof msg.i === "number" && STEPS[msg.i]) {
      doStep(ws, msg.i, "uninstall").catch((e) => log(`uninstall error: ${e.stack || e}`));
    } else if (msg.type === "apply") {
      // msg.want = array of package indices the user wants present. Apply the
      // DIFF only: install wanted-but-absent, uninstall unwanted-but-present.
      applyDiff(ws, msg.want || []).catch((e) => log(`apply error: ${e.stack || e}`));
    } else if (msg.type === "get-log") {
      send(ws, { type: "log", consent: readConsent(), history: readHistory() });
    } else if (msg.type === "set-consent") {
      writeConsent({ decided: true, share: !!msg.share });
      log(`consent set: share=${!!msg.share}`);
      send(ws, { type: "log", consent: readConsent(), history: readHistory() });
    } else if (msg.type === "clear-log") {
      clearHistory();
      log("history cleared by user");
      send(ws, { type: "log", consent: readConsent(), history: readHistory() });
    }
  });
  ws.on("close", () => { log("client disconnected"); scheduleShutdownIfIdle(); });
});

// Run a command in a pty, stream its bytes tagged by step index. Resolve to
// the process exit code. On Windows wrap in powershell so PATH lookup + .cmd
// shims (npm.cmd, code.cmd) resolve uniformly.
// `silent`: don't stream output to the panel. Used for `detect`, which is an
// internal probe ("is it there?") — its failure is plumbing, not user info.
// Only real actions (install/uninstall) are shown.
function runInPty(ws, idx, cmdline, { silent = false } = {}) {
  return new Promise((resolve) => {
    const file = isWindows ? "powershell.exe" : "/bin/sh";
    // Rebuild PATH from the registry (machine + user) before running, so tools
    // installed before OR during this session are found — the hidden node
    // process inherited a stale PATH.
    // `; exit $LASTEXITCODE` returns the WRAPPED command's exit code (winget/
    // npm), not PowerShell's own — otherwise every code reads as 0.
    const refresh = "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";
    // -ExecutionPolicy Bypass: npm ships as npm.ps1, blocked by the default
    // policy on managed machines ("running scripts is disabled"). Bypass lets
    // this invocation run it without changing the machine's global policy.
    const argv = isWindows
      ? ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", `${refresh} ${cmdline}; exit $LASTEXITCODE`]
      : ["-c", cmdline];
    let term;
    try {
      term = pty.spawn(file, argv, { name: "xterm-color", cols: 100, rows: 18, cwd: ROOT, env: process.env });
    } catch (e) {
      // pty.spawn throws if the shell isn't found — the silent killer. Show it.
      const why = `spawn failed: ${file} — ${e.message}`;
      log(`step ${idx}: ${why}`);
      if (!silent) send(ws, { type: "out", i: idx, data: Buffer.from(`\r\n\x1b[31m${why}\x1b[0m\r\n`).toString("base64") });
      resolve(-1);
      return;
    }
    term.onData((data) => { if (!silent) send(ws, { type: "out", i: idx, data: Buffer.from(data, "utf8").toString("base64") }); });
    term.onExit(({ exitCode }) => { log(`step ${idx}: exit ${exitCode}`); resolve(exitCode); });
  });
}

// Detect a package's PRESENCE (not via pty, not shown). Fast: ask PowerShell's
// Get-Command for the binary on a freshly-rebuilt PATH — returns reliably,
// silently. The binary is the first word of the package's `detect` string.
// Resolves true/false. This is the ground truth the UI pre-checks against and
// the diff compares to — never a cached flag that could lie.
function detectPresent(step) {
  return new Promise((resolve) => {
    if (!step.detect) return resolve(false);
    const bin = step.detect.trim().split(/\s+/)[0];
    if (!isWindows) return execFile("/bin/sh", ["-c", `command -v '${bin}'`], (e) => resolve(!e));
    const ps = `$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User'); if (Get-Command '${bin}' -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }`;
    execFile("powershell.exe", ["-NoProfile", "-Command", ps], (e) => resolve(!e));
  });
}

// Capture a package's installed VERSION by running its `detect` command and
// grabbing the first version-looking token. Silent, for the log — not shown.
function captureVersion(step) {
  return new Promise((resolve) => {
    if (!step.detect) return resolve(null);
    const done = (out) => {
      const m = (out || "").match(/\d+\.\d+(\.\d+)?/);
      resolve(m ? m[0] : (out || "").trim().split("\n")[0]?.slice(0, 40) || null);
    };
    if (!isWindows) return execFile("/bin/sh", ["-c", step.detect], (e, o) => done(o));
    const ps = `$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User'); ${step.detect}`;
    execFile("powershell.exe", ["-NoProfile", "-Command", ps], (e, o) => done(o));
  });
}

// Detect every package's state and send it so the panel pre-checks what's
// already installed. The diff (apply) compares the user's wishes to THIS.
async function detectAll(ws) {
  const results = await Promise.all(STEPS.map(detectPresent));
  results.forEach((present, i) => send(ws, { type: "state", i, present }));
  send(ws, { type: "state-done" });
  log(`detected: ${results.filter(Boolean).length}/${STEPS.length} present`);
}

// Serial lock: only ONE package manager runs at a time, ever. Two winget
// processes on the same package fight over the temp file ("being used by
// another process"). All actions — global Apply, a per-package apply, even a
// stray double-click — chain through this queue, so they never overlap.
let actionQueue = Promise.resolve();
function serialize(fn) {
  const run = actionQueue.then(fn, fn);   // run after whatever's ahead, success or fail
  actionQueue = run.catch(() => {});       // never let one failure break the chain
  return run;
}

// Run one step's `install` or `uninstall` command and show it. winget/npm are
// idempotent — they handle "already installed / already absent" themselves and
// report it via a benign exit code, so no separate probe is needed.
//
// selfHost (node): winget removes node fine even while node.exe is running
// (the file goes at next reboot; the live process keeps its in-memory copy).
// So we just run it inline like everything else — and afterward tell the user
// they can close the window, since relaunch won't work until node is back.
function doStep(ws, i, action) {
  return serialize(() => doStepNow(ws, i, action));   // never overlap with another action
}
async function doStepNow(ws, i, action) {
  const s = STEPS[i];
  const cmd = s[action];
  if (!cmd) return false;
  busy++;                      // hold off shutdown while this step runs
  try {
    send(ws, { type: "step", i, status: action === "uninstall" ? "uninstalling" : "installing" });
    const code = await runInPty(ws, i, cmd);
    const ok = code === 0 || BENIGN_CODES.has(code);
    log(`step ${i} ${action} ${ok ? "OK" : "FAILED"} (exit ${code}): ${s.name}`);
    send(ws, { type: "step", i, status: ok ? (action === "uninstall" ? "absent" : "ok") : "fail" });
    // Journal the outcome (version captured for installs). Location depends on
    // consent; either way the Log tab can show it.
    const version = ok && action === "install" ? await captureVersion(s) : null;
    appendHistory({ at: new Date().toISOString(), package: s.name, bundle: s.bundle, action, ok, exit: code, version });
    if (ok && action === "uninstall" && s.selfHost) {
      send(ws, { type: "overlay", title: `${s.name} removed`,
        body: "This panel runs on it, so it can't keep running. You can close this window — re-open the tool later to set things up again." });
    }
    return ok;
  } finally {
    busy--;
  }
}

// Apply the DIFF between what the user wants and what's actually on the machine.
// `want` = package indices the user wants present. We RE-DETECT (ground truth),
// then only act on differences: install wanted-but-absent, uninstall
// unwanted-but-present. A small change → a small action; unchanged → nothing.
// Uninstalls run first (reverse dep order, selfHost/node LAST), then installs.
async function applyDiff(ws, want) {
  const wanted = new Set(want);
  const present = await Promise.all(STEPS.map(detectPresent));

  const toInstall = [...STEPS.keys()].filter((i) => wanted.has(i) && !present[i]);
  let toRemove = [...STEPS.keys()].filter((i) => !wanted.has(i) && present[i] && STEPS[i].uninstall);
  toRemove.sort((a, b) => (STEPS[a].selfHost ? 1 : 0) - (STEPS[b].selfHost ? 1 : 0)); // node last

  log(`apply diff: +[${toInstall.map((i) => STEPS[i].name).join(", ") || "—"}] -[${toRemove.map((i) => STEPS[i].name).join(", ") || "—"}]`);
  if (!toInstall.length && !toRemove.length) { send(ws, { type: "done", nothing: true }); return; }

  for (const i of toRemove) await doStep(ws, i, "uninstall");
  for (const i of toInstall) await doStep(ws, i, "install");
  log("apply done");
  send(ws, { type: "done" });
}

server.listen(PORT, () => log(`listening on http://localhost:${PORT}  (log: ${join(ROOT, "panel.log")})`));
