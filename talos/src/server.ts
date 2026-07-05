// server.ts — Talos engine on Deno (Phase 2 · T1: the skeleton that boots).
//
//   Deno.serve      → HTTP (real public/ tree) + WebSocket   (replaces node http + ws)
//   @sigma/pty-ffi  → spawn commands in a real PTY, stream bytes  (replaces node-pty)
//
// Compiled with `deno compile --include public --include native/<lib>` → one exe.
// This T1 serves the REAL UI and echoes a demo command through a PTY so we can
// prove the socket end-to-end on Mac. The engine (bundles, detectRoutes,
// applyDiff, outdated, log/consent) lands in later tranches — see _PLAN.

import { instantiate, libName, Pty } from "@sigma/pty-ffi/noinit";
import { loadBundles } from "./bundles.ts";
import { detectPresent } from "./detect.ts";
import { hideConsoleIfHeadless } from "./win-console.ts";

const isWin = Deno.build.os === "windows";

// Per-machine LOCAL data dir — where we extract the native lib, the log, and
// (later) consent. NEVER on the shared OneDrive next to the exe: the exe is
// launched by N machines, so everything writable must live on each machine's own
// disk. Windows → %LOCALAPPDATA%\Talos ; Mac → ~/Library/Application Support/Talos.
function localDataDir(): string {
  if (isWin) {
    const base = Deno.env.get("LOCALAPPDATA") ??
      `${Deno.env.get("USERPROFILE")}\\AppData\\Local`;
    return `${base}\\Talos`;
  }
  return `${Deno.env.get("HOME")}/Library/Application Support/Talos`;
}
const DATA_DIR = localDataDir();

// In --no-terminal (GUI) mode there's NO console — a crash is invisible. Journal
// every step to a file in the machine-local data dir (NOT next to the exe, which
// is on the shared OneDrive). Best-effort mkdir so the very first line lands.
const LOG = `${DATA_DIR}${isWin ? "\\" : "/"}talos.log`;
function log(msg: string) {
  const line = `${new Date().toISOString()} ${msg}`;
  // Echo to the console too — invisible in a --no-terminal build (no console),
  // but the whole point of a DEBUG build (built WITHOUT --no-terminal) is to see
  // these live in a PowerShell window instead of hunting for the log file.
  try {
    console.error(line);
  } catch { /* no console attached */ }
  try {
    try {
      Deno.mkdirSync(DATA_DIR, { recursive: true });
    } catch { /* already there */ }
    Deno.writeTextFileSync(LOG, `${line}\n`, { append: true });
  } catch (_) { /* last resort: nothing to do without a console */ }
}
log(
  `--- start: standalone=${Deno.build.standalone} os=${Deno.build.os} data=${DATA_DIR}`,
);

// FIRST, before ANY child process (self-heal, detection probes): on a headless
// Windows GUI build, give ourselves a hidden console so children inherit it and
// stop flashing their own windows. No-op on Mac; leaves a real console alone.
hideConsoleIfHeadless(log);

// FFI gotcha: Deno.dlopen loads a native lib from a REAL file on disk. `--include`
// puts the lib in the binary's virtual FS — Deno.readFile CAN read it, but dlopen
// CANNOT. We deliberately do NOT use --self-extracting (it would drop the lib next
// to the exe, i.e. on the shared OneDrive). Instead: read the embedded bytes and
// write them to the machine-LOCAL dir, then dlopen from there. Idempotent: skip if
// already present, write-temp-then-rename so a concurrent instance never sees a
// half-written .dll. See memory talos-shared-exe-local-extraction.
async function loadPty() {
  const lib = libName();
  if (!Deno.build.standalone) {
    // Dev (deno run): let pty-ffi resolve/download the lib itself.
    log("instantiate: dev (auto-resolve)");
    await instantiate();
    log("instantiate OK (dev)");
    return;
  }
  // The lib is --include'd under native/, a SIBLING of src/. The embedded virtual
  // FS does NOT normalize ".." segments, so build the sibling path explicitly:
  // strip the trailing "src" from import.meta.dirname, then append "native/<lib>".
  const here = import.meta.dirname ?? ".";
  const parent = here.replace(/[/\\][^/\\]+$/, ""); // drop the final "/src"
  const embedded = `${parent}/native/${lib}`;
  const sep = isWin ? "\\" : "/";
  const destDir = `${DATA_DIR}${sep}native`;
  const dest = `${destDir}${sep}${lib}`;
  try {
    Deno.statSync(dest);
    log(`pty lib already local: ${dest}`);
  } catch {
    log(`extracting pty lib → ${dest}`);
    Deno.mkdirSync(destDir, { recursive: true });
    const bytes = Deno.readFileSync(embedded); // from the embedded virtual FS
    const tmp = `${dest}.tmp-${Deno.pid}`;
    Deno.writeFileSync(tmp, bytes);
    Deno.renameSync(tmp, dest); // atomic: no half-written .dll for a racing instance
  }
  log(`instantiate from ${dest}`);
  await instantiate(dest);
  log("instantiate OK");
}

// The pty engine is loaded in the BACKGROUND (after the window opens) — it's only
// needed to RUN a command, not to show the panel. This lets the window appear as
// early as possible instead of after the extraction/dlopen. Until ready, a "run"
// request gets a friendly "still starting" reply rather than a crash.
let ptyReady = false;

const PORT = 7682; // the real Talos port.
const SHELL = isWin ? "powershell.exe" : "/bin/bash";
// public/ is a SIBLING of src/. The embedded virtual FS (no --self-extracting)
// doesn't normalize "..", so build the sibling path explicitly, like the lib.
const PUBLIC = `${
  (import.meta.dirname ?? ".").replace(/[/\\][^/\\]+$/, "")
}/public`;

// bundles/ — the integrator's CONTENT, deliberately NOT compiled into the exe
// (unlike public/, which IS the engine). It lives on the REAL disk beside the
// exe, so a team drops one generic talos.exe + their own bundles/ side by side
// on the shared OneDrive and gets their installer — no recompile. This is the
// hermeticity boundary made physical: the engine ships empty of content.
//
// Two worlds, two primitives, on purpose:
//   compiled → the exe's own folder, via Deno.execPath() (the REAL path on disk;
//              import.meta.dirname would point INTO the embedded virtual FS).
//   dev      → the project's bundles/ at the REPO root: strip /src → talos/,
//              then ../bundles (src/ sits in the nested talos/talos/ that Phase 3
//              flattens away; matches server.js's join(ROOT, "..", "bundles")).
// It's a fixed name, NOT a setting: convention over configuration closes the
// "where are my bundles?" question instead of reopening it. A missing folder is
// not an error here — an exe with no bundles beside it opens inert (T2 handles
// the empty case; core = proposition only).
const BUNDLES_DIR = Deno.build.standalone
  ? `${Deno.execPath().replace(/[/\\][^/\\]+$/, "")}${
    isWin ? "\\" : "/"
  }bundles`
  : `${(import.meta.dirname ?? ".").replace(/[/\\][^/\\]+$/, "")}/../bundles`;
log(`bundles dir: ${BUNDLES_DIR}`);

// Scan the bundles ONCE at startup — pure data, no pty/network, so it's safe to
// do before the engine loads. The result is the `plan` the UI renders on connect
// (bundles → accordion cards, steps → package rows). An empty scan (no bundles/
// beside the exe) yields an empty accordion, not a crash.
const { bundles: BUNDLES, steps: STEPS } = loadBundles(BUNDLES_DIR, log);
log(`plan: ${BUNDLES.length} bundle(s), ${STEPS.length} package(s)`);

// --- static assets: serve the real public/ tree ----------------------------
// Vendored xterm lives in public/vendor/ (never a CDN — corporate firewall 403s it).
// app.js + decision.js are the panel's own ES modules; decision.js is the SHARED
// decision rule (same file the browser fetches AND the engine will import).
const TYPES: Record<string, string> = {
  html: "text/html",
  js: "text/javascript",
  css: "text/css",
  json: "application/json",
  svg: "image/svg+xml",
  png: "image/png",
};
async function serveStatic(pathname: string): Promise<Response> {
  const rel = pathname === "/" ? "/index.html" : pathname;
  // Guard against path traversal: no "..".
  if (rel.includes("..")) return new Response("bad path", { status: 400 });
  const ext = rel.split(".").pop() ?? "";
  try {
    const bytes = await Deno.readFile(`${PUBLIC}${rel}`);
    return new Response(bytes, {
      headers: { "content-type": TYPES[ext] ?? "application/octet-stream" },
    });
  } catch {
    return new Response("not found: " + rel, { status: 404 });
  }
}

// Stream a command through a PTY into the WebSocket, base64 per chunk.
async function runInPty(ws: WebSocket, cmdline: string) {
  const pty = new Pty(SHELL);
  const nl = isWin ? "\r\n" : "\n";
  pty.write(`${cmdline}; exit${nl}`);
  const b64 = (u8: Uint8Array) => btoa(String.fromCharCode(...u8));
  while (true) {
    const { data, done } = pty.readBytes();
    if (done) break;
    if (data.byteLength) {
      for (let i = 0; i < data.length; i += 8192) {
        ws.send(
          JSON.stringify({
            type: "out",
            data: b64(data.subarray(i, i + 8192)),
          }),
        );
      }
    } else {
      await new Promise((r) => setTimeout(r, 10));
    }
  }
  ws.send(JSON.stringify({ type: "done", code: 0 }));
}

// Open the panel as an APP window — a Chromium `--app` window is frameless: no
// address bar, no tabs, its own dock/taskbar entry. That's what makes a localhost
// page feel like a native app rather than a browser tab. We look for a Chromium
// browser (Edge, then Chrome) and launch it with --app + a fixed window size; only
// if none is found do we fall back to opening a plain browser tab.
const APP_WIDTH = 980;
const APP_HEIGHT = 720;

// Find a Chromium browser per OS and return how to launch it with --app.
// Windows: probe the usual install paths and run the .exe DIRECTLY — going
// through `cmd /c start` fails in --no-terminal mode (a GUI process has no valid
// stdio handles, so spawning cmd throws "Invalid handle").
function chromiumApp(url: string): { cmd: string; args: string[] } | null {
  const flags = [`--app=${url}`, `--window-size=${APP_WIDTH},${APP_HEIGHT}`];
  const candidates = isWin
    ? [
      `${
        Deno.env.get("ProgramFiles(x86)")
      }\\Microsoft\\Edge\\Application\\msedge.exe`,
      `${
        Deno.env.get("ProgramFiles")
      }\\Microsoft\\Edge\\Application\\msedge.exe`,
      `${
        Deno.env.get("ProgramFiles")
      }\\Google\\Chrome\\Application\\chrome.exe`,
      `${
        Deno.env.get("ProgramFiles(x86)")
      }\\Google\\Chrome\\Application\\chrome.exe`,
    ]
    : Deno.build.os === "darwin"
    ? [
      "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ]
    : [];
  for (const bin of candidates) {
    try {
      if (Deno.statSync(bin).isFile) return { cmd: bin, args: flags };
    } catch { /* not installed, try next */ }
  }
  return null; // no chromium → caller falls back
}

function openPanel() {
  const url = `http://localhost:${PORT}`;
  // --no-terminal (GUI) processes have NO valid stdio handles; inheriting them
  // when spawning makes the child fail with "Invalid handle". Give the child
  // null stdio so it never touches our (absent) console.
  const spawn = (cmd: string, args: string[]) =>
    new Deno.Command(cmd, {
      args,
      stdin: "null",
      stdout: "null",
      stderr: "null",
    }).spawn();
  try {
    const app = chromiumApp(url);
    if (app) {
      spawn(app.cmd, app.args);
      log(`openPanel: app window via ${app.cmd}`);
    } else if (isWin) {
      // No Chromium found — hand the URL to the shell's default handler.
      spawn("cmd", ["/c", "start", "", url]);
      log("openPanel: fallback to default browser");
    } else {
      spawn("open", [url]);
      log("openPanel: fallback to default browser");
    }
  } catch (e) {
    log(`openPanel FAILED: ${(e as Error).message} — open manually: ${url}`);
  }
}

// Self-heal: kill whatever is ALREADY listening on our port, surgically by PID
// (never "all deno"). This is the logic that used to live in start.bat — now
// that the exe replaces every launcher, the app owns it. It kills a stale server
// whose shutdown failed and guarantees every launch runs the latest code. Ported
// faithfully: netstat+taskkill on Windows, lsof+kill on Mac/Linux.
async function killPortHolder() {
  try {
    if (isWin) {
      const out = await new Deno.Command("netstat", {
        args: ["-ano"],
        stdin: "null",
        stderr: "null",
      }).output();
      const pids = new Set<string>();
      for (const line of new TextDecoder().decode(out.stdout).split(/\r?\n/)) {
        if (line.includes(`:${PORT} `) && line.includes("LISTENING")) {
          const pid = line.trim().split(/\s+/).pop();
          if (pid && pid !== "0") pids.add(pid);
        }
      }
      for (const pid of pids) {
        log(`self-heal: taskkill PID ${pid} (holds :${PORT})`);
        await new Deno.Command("taskkill", {
          args: ["/F", "/PID", pid],
          stdin: "null",
          stdout: "null",
          stderr: "null",
        }).output();
      }
    } else {
      const out = await new Deno.Command("lsof", {
        args: ["-ti", `tcp:${PORT}`],
        stdin: "null",
        stderr: "null",
      }).output();
      for (
        const pid of new TextDecoder().decode(out.stdout).split(/\s+/).filter(
          Boolean,
        )
      ) {
        if (pid === String(Deno.pid)) continue; // never kill ourselves
        log(`self-heal: kill PID ${pid} (holds :${PORT})`);
        await new Deno.Command("kill", {
          args: ["-9", pid],
          stdin: "null",
          stdout: "null",
          stderr: "null",
        }).output();
      }
    }
  } catch (e) {
    log(`self-heal probe failed (continuing): ${(e as Error).message}`);
  }
}

// --- lifecycle: the server lives only as long as the panel is open ----------
// Ported from server.js. When the user closes the panel window, its WebSocket
// closes; after a grace delay (tolerating a reload/reconnect), if no client is
// back AND no step is mid-run, the process exits cleanly. That is what keeps a
// closed window from leaving a hidden server behind (the zombies we saw), and
// makes every open start fresh. On Windows a --no-terminal build has no console
// to Ctrl-C, so THIS is the only clean way it ever stops.
let clients = 0;
let busy = 0; // > 0 while a pty step runs — never quit mid-action
let shutdownTimer: ReturnType<typeof setTimeout> | null = null;
const GRACE_MS = 4000;

// Live sockets, so we can tell the UI when the engine is ready (hides the splash)
// even if pty finishes loading AFTER the page connected.
const sockets = new Set<WebSocket>();
function broadcast(obj: unknown) {
  const s = JSON.stringify(obj);
  for (const ws of sockets) {
    try {
      ws.send(s);
    } catch { /* closing */ }
  }
}

// Probe every package's presence and stream one `state` per result, then
// `state-done`. Probes run in PARALLEL (Promise.all) — 13 independent reads,
// no reason to serialize. A helper for a single socket (the connecting client);
// on failure of one probe, detectPresent already resolves to "absent", so the
// scan as a whole never rejects. state-done ALWAYS fires (finally) so the UI's
// splash never hangs on a stuck probe.
async function detectAll(ws: WebSocket) {
  const send = (obj: unknown) => {
    try {
      ws.send(JSON.stringify(obj));
    } catch { /* socket closing */ }
  };
  try {
    const results = await Promise.all(
      STEPS.map((s) => detectPresent(s.detect, isWin)),
    );
    results.forEach((present, i) => send({ type: "state", i, present }));
    log(`detect: ${results.filter(Boolean).length}/${results.length} present`);
  } catch (e) {
    log(`detect error (continuing): ${(e as Error).message}`);
  } finally {
    send({ type: "state-done" });
  }
}

function scheduleShutdownIfIdle() {
  if (shutdownTimer !== null) clearTimeout(shutdownTimer);
  shutdownTimer = setTimeout(() => {
    if (clients === 0 && busy === 0) {
      log("no client + idle — shutting down");
      Deno.exit(0);
    } else {
      log(`shutdown skipped (clients=${clients}, busy=${busy})`);
    }
  }, GRACE_MS);
}

function startServer() {
  Deno.serve({
    port: PORT,
    onError: (e) => {
      log(`serve onError: ${e}`);
      return new Response("err", { status: 500 });
    },
    onListen: () => {
      log(`listening on http://localhost:${PORT}`);
      openPanel();
    },
  }, (req) => {
    const url = new URL(req.url);
    // The real UI (app.js) connects to ws://<host>/ — i.e. the ROOT path, not
    // "/ws". So upgrade on the Upgrade header, not a fixed pathname.
    if (req.headers.get("upgrade")?.toLowerCase() === "websocket") {
      const { socket, response } = Deno.upgradeWebSocket(req);
      socket.onopen = () => {
        clients++;
        sockets.add(socket);
        if (shutdownTimer !== null) { // a client is back — cancel any pending quit
          clearTimeout(shutdownTimer);
          shutdownTimer = null;
        }
        log(`client connected (clients=${clients})`);
        // Draw the plan and STOP — nothing runs on its own. The user drives every
        // action. The UI renders bundles → accordion, steps → rows (each carries
        // its index `i`, the id every later message keys off). selection/consent
        // are neutral for now (persisted selection = T3, consent = T4).
        socket.send(JSON.stringify({
          type: "plan",
          bundles: BUNDLES,
          steps: STEPS.map((s, i) => ({
            i,
            name: s.name,
            description: s.description,
            bundle: s.bundle,
            canUninstall: !!s.uninstall,
            posture: s.posture,
          })),
          selection: { pkgs: {} },
          consent: { decided: true },
        }));
        // Ground truth → pre-check the cards. "Detect, don't remember": ask the
        // machine what's present RIGHT NOW (never a journal). Each result is a
        // `state` message; `state-done` closes the scan (and hides the splash —
        // THIS is the first real wait the splash covers). Detection feeds PRESENCE
        // only, never the decision. Best-effort: a probe that throws is "absent".
        detectAll(socket);
        // If the engine is already up, tell the UI right away (hides the splash).
        if (ptyReady) socket.send(JSON.stringify({ type: "ready" }));
      };
      socket.onclose = () => {
        clients = Math.max(0, clients - 1);
        sockets.delete(socket);
        log(`client disconnected (clients=${clients})`);
        scheduleShutdownIfIdle();
      };
      socket.onmessage = (ev) => {
        const msg = JSON.parse(ev.data);
        // T1 demo: a single non-destructive command proves the socket path.
        // Real dispatch (bundles + applyDiff via decision.js) comes in T2/T3.
        if (msg.type === "run" && typeof msg.action === "string") {
          if (!ptyReady) {
            // Window opened before the engine finished loading — tell the UI,
            // don't crash. (Rare: loading is fast; only a very eager click.)
            socket.send(JSON.stringify({ type: "starting" }));
            return;
          }
          const CMDS: Record<string, string> = isWin
            ? { list: "winget list --source winget" }
            : { list: "brew list" };
          const cmd = CMDS[msg.action];
          if (cmd) {
            busy++;
            runInPty(socket, cmd)
              .catch((e) => log(`pty error: ${(e as Error).message}`))
              .finally(() => {
                busy = Math.max(0, busy - 1);
              });
          }
        }
      };
      return response;
    }
    return serveStatic(url.pathname);
  });
}

// Load the pty engine in the background, then flip ptyReady. Failure is logged
// but does NOT kill the process — the window is already open, so the UI can show
// the error instead of the app dying silently in --no-terminal mode.
function loadPtyInBackground() {
  loadPty()
    .then(() => {
      ptyReady = true;
      log("pty engine ready");
      broadcast({ type: "ready" }); // hide the splash on any connected client
    })
    .catch((e) =>
      log(`pty load FAILED (window stays open): ${(e as Error).message}`)
    );
}

// Startup order chosen so the WINDOW APPEARS AS EARLY AS POSSIBLE:
//   1. self-heal (must clear the port before we can bind) — the only unavoidable
//      blind moment, and it's short.
//   2. serve + openPanel → the window opens NOW (splash shows immediately).
//   3. load the pty engine in the background → the panel is already visible while
//      the (heavier) extraction/dlopen happens.
// Ported from start.bat's "kill the holder first" so every launch starts fresh.
log("self-heal: clearing port before start…");
await killPortHolder();
await new Promise((r) => setTimeout(r, 200)); // let the OS release the socket

log("Deno.serve starting…");
try {
  startServer();
  loadPtyInBackground();
} catch (e) {
  if (e instanceof Deno.errors.AddrInUse) {
    // A holder slipped in during the race — kill once more and retry.
    log(`port ${PORT} still busy — self-healing again…`);
    await killPortHolder();
    await new Promise((r) => setTimeout(r, 300));
    try {
      startServer();
      loadPtyInBackground();
    } catch (e2) {
      if (e2 instanceof Deno.errors.AddrInUse) {
        log(
          `port ${PORT} still busy after self-heal — deferring, re-opening window`,
        );
        openPanel();
      } else {
        log(`Deno.serve FAILED after self-heal: ${(e2 as Error).message}`);
      }
    }
  } else {
    log(`Deno.serve FAILED: ${(e as Error).message}\n${(e as Error).stack}`);
  }
}
