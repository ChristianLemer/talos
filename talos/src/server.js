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
    upgrade: `winget upgrade --id ${pkg.winget} -e --accept-source-agreements --accept-package-agreements`,
  };
  if (pkg.npm) return {
    install: `npm install -g ${pkg.npmFlags ? pkg.npmFlags + " " : ""}${pkg.npm}`,
    uninstall: `npm uninstall -g ${pkg.npm}`,
    // npm has no cheap "list everything outdated" we parse today, so npm upgrades
    // are never TRIGGERED (see applyDiff) — but the command is here, future-ready.
    upgrade: `npm install -g ${pkg.npmFlags ? pkg.npmFlags + " " : ""}${pkg.npm}@latest`,
  };
  if (pkg.run) return { install: pkg.run, uninstall: pkg.runUninstall || null, upgrade: null };  // escape hatch: raw command
  return { install: null, uninstall: null, upgrade: null };
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
            install: cmd.install, uninstall: cmd.uninstall, upgrade: cmd.upgrade,
            winget: p.winget || null,   // the id winget reports in `winget upgrade` — how we match outdated rows
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
  // Ground truth → pre-check the cards (fast, offline), THEN scan for outdated
  // (online, best-effort) so stale packages light their Apply buttons at rest.
  detectAll(ws)
    .then(() => reportOutdated(ws))
    .catch((e) => log(`detect error: ${e.stack || e}`));

  ws.on("message", (raw) => {
    let msg; try { msg = JSON.parse(raw.toString()); } catch { return; }
    if (msg.type === "install" && typeof msg.i === "number" && STEPS[msg.i]) {
      // A single-row action, like Apply, ends with `done` so the UI can unlock.
      doStep(ws, msg.i, "install").catch((e) => log(`install error: ${e.stack || e}`)).finally(() => send(ws, { type: "done" }));
    } else if (msg.type === "uninstall" && typeof msg.i === "number" && STEPS[msg.i]) {
      doStep(ws, msg.i, "uninstall").catch((e) => log(`uninstall error: ${e.stack || e}`)).finally(() => send(ws, { type: "done" }));
    } else if (msg.type === "upgrade" && typeof msg.i === "number" && STEPS[msg.i]) {
      doStep(ws, msg.i, "upgrade").catch((e) => log(`upgrade error: ${e.stack || e}`)).finally(() => send(ws, { type: "done" }));
    } else if (msg.type === "apply") {
      // msg.want = the package indices the user wants present (the WHOLE desired
      // state — needed so the diff is correct). msg.scope, if present, restricts
      // which packages we ACT on (a per-bundle Apply passes just that bundle's
      // indices) — so a scoped Apply never touches packages outside it.
      applyDiff(ws, msg.want || [], msg.scope || null).catch((e) => log(`apply error: ${e.stack || e}`));
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

// --- outdated detection: ONE `winget upgrade` for the whole machine ---------
// winget knows the latest version of everything — so instead of probing each
// package (winget show ×N, slow, 403-prone) we ask ONCE for the full list of
// what has an update, with current→available. We parse the fixed-width table
// by the HEADER's column offsets (Id / Version / Available), not by splitting
// on spaces — names and versions contain spaces, offsets don't lie.
// Returns a Map: winget-id (lowercased) → { current, available }.
// Defensive throughout: any parse hiccup yields an empty map, never a throw —
// a failed scan just means "nothing looks outdated", the safe direction.
function parseWingetUpgrade(raw) {
  const map = new Map();
  try {
    const lines = raw
      .replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "")      // strip ANSI
      .replace(/[─-╿]/g, "")             // strip box-drawing / progress glyphs
      .split(/\r?\n/)
      .map((l) => l.replace(/\r/g, ""));           // drop stray carriage returns (spinner)
    // The header row is the one naming the columns. winget localises these, but
    // the English "Name  Id  Version  Available  Source" is what ships on the
    // managed machines we target. Match on Id + Available, the two we slice by.
    const h = lines.findIndex((l) => /\bId\b/.test(l) && /\bAvailable\b/.test(l));
    if (h < 0) return map;
    const header = lines[h];
    const idPos = header.indexOf("Id");
    const verPos = header.indexOf("Version");
    const avPos = header.indexOf("Available");
    const srcPos = header.indexOf("Source");
    if (idPos < 0 || verPos < 0 || avPos < 0) return map;
    for (const line of lines.slice(h + 1)) {
      if (!line.trim()) break;                     // blank line = end of table
      if (/^[-\s]+$/.test(line)) continue;         // the --- separator row
      // A trailing summary like "12 upgrades available." has no column structure.
      if (line.length < avPos) continue;
      const id = line.slice(idPos, verPos).trim();
      const current = line.slice(verPos, avPos).trim();
      const available = line.slice(avPos, srcPos > avPos ? srcPos : undefined).trim();
      if (!id || !available) continue;
      map.set(id.toLowerCase(), { current, available });
    }
  } catch (e) {
    log(`winget upgrade parse failed (treating as none outdated): ${e.message}`);
  }
  return map;
}

// Run the single scan. Not in a pty (no TTY → less spinner noise); PATH refreshed
// like everywhere else. Behind the same benign-code tolerance. Never rejects.
function scanOutdated() {
  return new Promise((resolve) => {
    if (!isWindows) return resolve(new Map());   // winget is Windows-only
    const ps = `$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User'); winget upgrade --accept-source-agreements`;
    execFile("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps],
      { maxBuffer: 4 * 1024 * 1024 }, (err, stdout) => {
        if (err && !stdout) { log(`winget upgrade scan failed: ${err.message}`); return resolve(new Map()); }
        const map = parseWingetUpgrade(stdout || "");
        log(`winget upgrade: ${map.size} package(s) with an update available`);
        resolve(map);
      });
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

// After presence (fast, offline) is reported, run the ONE online scan and tell
// the panel which present packages are outdated, with their version delta — so
// their Apply buttons light up at rest, not only after an Apply. Kept SEPARATE
// from detectAll and run AFTER state-done, so the splash + panel never wait on
// the network. A failed/empty scan (e.g. a 403 behind the firewall) simply
// reports nothing — the panel behaves exactly as before, upgrade still caught
// at Apply time.
async function reportOutdated(ws) {
  const outdated = await scanOutdated();
  if (!outdated.size) return;
  let shown = 0;
  STEPS.forEach((s, i) => {
    if (s.winget && s.upgrade && outdated.has(s.winget.toLowerCase())) {
      const o = outdated.get(s.winget.toLowerCase());
      send(ws, { type: "outdated", i, current: o.current, available: o.available });
      shown++;
    }
  });
  log(`outdated among our packages: ${shown}`);
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
    const runningStatus = action === "uninstall" ? "uninstalling" : action === "upgrade" ? "upgrading" : "installing";
    send(ws, { type: "step", i, status: runningStatus });
    const code = await runInPty(ws, i, cmd);
    const ok = code === 0 || BENIGN_CODES.has(code);
    log(`step ${i} ${action} ${ok ? "OK" : "FAILED"} (exit ${code}): ${s.name}`);
    send(ws, { type: "step", i, status: ok ? (action === "uninstall" ? "absent" : "ok") : "fail" });
    // Journal the outcome (version captured for installs/upgrades). Location
    // depends on consent; either way the Log tab can show it.
    const version = ok && action !== "uninstall" ? await captureVersion(s) : null;
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
// `want` = package indices the user wants present. We RE-DETECT presence (ground
// truth) AND scan once for outdated packages, then converge in FOUR cases:
//   wanted & absent            → install
//   wanted & present & OUTDATED → upgrade   (new: one Apply keeps you current)
//   wanted & present & current  → skip
//   unwanted & present          → uninstall
// A small change → a small action; already-correct → nothing.
// Order: uninstalls first (reverse dep order, selfHost/node LAST), then installs,
// then upgrades. Outdated detection is winget-only (npm has no cheap scan) — an
// npm package present-but-old is left as-is, not silently claimed up-to-date.
async function applyDiff(ws, want, scope) {
  const wanted = new Set(want);
  // `scope` (optional): the only indices we may act on. A per-bundle Apply passes
  // its bundle's packages; the global Apply passes nothing → every package is in
  // scope. The diff is still computed against the FULL wanted set, so presence is
  // judged correctly — we just don't ACT outside the scope.
  const inScope = scope ? new Set(scope) : null;
  const acts = (i) => !inScope || inScope.has(i);
  const [present, outdated] = await Promise.all([
    Promise.all(STEPS.map(detectPresent)),
    scanOutdated(),
  ]);

  // A present, wanted, winget package whose id is in the outdated map is stale.
  const isOutdated = (i) => {
    const s = STEPS[i];
    return !!(s.winget && s.upgrade) && outdated.has(s.winget.toLowerCase());
  };

  const toInstall = [...STEPS.keys()].filter((i) => acts(i) && wanted.has(i) && !present[i]);
  const toUpgrade = [...STEPS.keys()].filter((i) => acts(i) && wanted.has(i) && present[i] && isOutdated(i));
  let toRemove = [...STEPS.keys()].filter((i) => acts(i) && !wanted.has(i) && present[i] && STEPS[i].uninstall);
  toRemove.sort((a, b) => (STEPS[a].selfHost ? 1 : 0) - (STEPS[b].selfHost ? 1 : 0)); // node last

  const verOf = (i) => { const o = outdated.get(STEPS[i].winget.toLowerCase()); return o ? `${o.current}→${o.available}` : "?"; };
  log(`apply diff: +[${toInstall.map((i) => STEPS[i].name).join(", ") || "—"}]` +
      ` ↑[${toUpgrade.map((i) => `${STEPS[i].name} ${verOf(i)}`).join(", ") || "—"}]` +
      ` -[${toRemove.map((i) => STEPS[i].name).join(", ") || "—"}]`);
  if (!toInstall.length && !toUpgrade.length && !toRemove.length) { send(ws, { type: "done", nothing: true }); return; }

  for (const i of toRemove) await doStep(ws, i, "uninstall");
  for (const i of toInstall) await doStep(ws, i, "install");
  for (const i of toUpgrade) { send(ws, { type: "detail", i, detail: verOf(i) }); await doStep(ws, i, "upgrade"); }
  log("apply done");
  send(ws, { type: "done" });
}

server.listen(PORT, () => log(`listening on http://localhost:${PORT}  (log: ${join(ROOT, "panel.log")})`));
