// server.js — the engine: an imperative host that runs DECLARATIVE recipes.
//
// It reads recipes/default.json (pure data — name/detect/install/uninstall),
// and for each step:
//   1. DETECT  — run the recipe's `detect` command; if it succeeds, the step
//                is already done (green, folded). Detection is cheap.
//   2. INSTALL — only if detect failed: run `install` in a pty so the package
//                manager's own progress bar streams through and xterm projects
//                the live redraw (winget/npm fill in place, not stacked).
//
// The browser can also ask to re-run ONE step ({type:"run", i}) — that is the
// "by hand" facility without a script: every row is individually replayable.
//
// One language owns the loop here (Node). Recipes carry no logic; if a step
// needs logic, its command is `nu -c "…"` (nushell as surgical instrument).
//
// Events over one WebSocket (pure JSON, no prefix):
//   {type:"plan", steps}              — render every accordion row first
//   {type:"step", i, status}          — waiting | checking | installing | ok | fail
//   {type:"out",  i, data}            — raw pty bytes (base64), tagged by step
//   {type:"done"}

import http from "node:http";
import { readFileSync, readdirSync, createWriteStream } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { WebSocketServer } from "ws";
import pty from "node-pty";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, "..");
const PORT = 7682;
const isWindows = process.platform === "win32";

// --- trace, since node runs hidden (no console to watch) -------------------
const logFile = createWriteStream(join(ROOT, "panel.log"), { flags: "a" });
function log(msg) {
  const line = `${new Date().toISOString()}  ${msg}`;
  logFile.write(line + "\n");
  console.log(line);
}
log(`--- panel start (pid ${process.pid}, platform ${process.platform}) ---`);

// --- recipes: pure data, no logic ------------------------------------------
// Scan recipes/*.json and merge their steps. This is the host/plugins model:
// the engine knows nothing of any specific tool — it reads whatever recipes
// are present. default.json is the core (strict minimum to run Claude) and
// loads first; other files (nushell.json now, an integrator's file later) add
// extras. Same mechanism, different content.
const RECIPES_DIR = join(ROOT, "recipes");
function loadRecipes() {
  const files = readdirSync(RECIPES_DIR).filter((f) => f.endsWith(".json"));
  files.sort((a, b) => (a === "default.json" ? -1 : b === "default.json" ? 1 : a.localeCompare(b)));
  const steps = [];
  for (const f of files) {
    try {
      const r = JSON.parse(readFileSync(join(RECIPES_DIR, f), "utf8"));
      for (const s of (r.steps || [])) steps.push(s);
      log(`recipe loaded: ${f} (${(r.steps || []).length} steps)`);
    } catch (e) {
      log(`recipe skipped (bad JSON): ${f} — ${e.message}`);
    }
  }
  return steps;
}
const STEPS = loadRecipes();

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
  send(ws, { type: "plan", steps: STEPS.map((s, i) => ({ i, name: s.name, description: s.description, canUninstall: !!s.uninstall })) });

  ws.on("message", (raw) => {
    let msg; try { msg = JSON.parse(raw.toString()); } catch { return; }
    if (msg.type === "install" && typeof msg.i === "number" && STEPS[msg.i]) {
      doStep(ws, msg.i, "install").catch((e) => log(`install error: ${e.stack || e}`));
    } else if (msg.type === "uninstall" && typeof msg.i === "number" && STEPS[msg.i]) {
      doStep(ws, msg.i, "uninstall").catch((e) => log(`uninstall error: ${e.stack || e}`));
    } else if (msg.type === "install-all") {
      runAll(ws, "install").catch((e) => log(`install-all error: ${e.stack || e}`));
    } else if (msg.type === "uninstall-all") {
      runAll(ws, "uninstall").catch((e) => log(`uninstall-all error: ${e.stack || e}`));
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

// Run one step's `install` or `uninstall` command and show it. winget/npm are
// idempotent — they handle "already installed / already absent" themselves and
// report it via a benign exit code, so no separate probe is needed.
//
// selfHost (node): winget removes node fine even while node.exe is running
// (the file goes at next reboot; the live process keeps its in-memory copy).
// So we just run it inline like everything else — and afterward tell the user
// they can close the window, since relaunch won't work until node is back.
async function doStep(ws, i, action) {
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
    if (ok && action === "uninstall" && s.selfHost) {
      send(ws, { type: "overlay", title: `${s.name} removed`,
        body: "This panel runs on it, so it can't keep running. You can close this window — re-open the tool later to set things up again." });
    }
    return ok;
  } finally {
    busy--;
  }
}

// Global: run `action` over every step. Reverse for uninstall (dependents
// before dependencies) and put selfHost (node) LAST so the panel stays alive
// while the others are removed.
async function runAll(ws, action) {
  let order = action === "uninstall" ? [...STEPS.keys()].reverse() : [...STEPS.keys()];
  if (action === "uninstall") order = order.sort((a, b) => (STEPS[a].selfHost ? 1 : 0) - (STEPS[b].selfHost ? 1 : 0));
  log(`${action}-all: ${order.map((i) => STEPS[i].name).join(" → ")}`);
  for (const i of order) await doStep(ws, i, action);
  log(`${action}-all done`);
  send(ws, { type: "done" });
}

server.listen(PORT, () => log(`listening on http://localhost:${PORT}  (log: ${join(ROOT, "panel.log")})`));
