// server.ts — Talos engine on Deno (Phase 2 · T3: it acts).
//
//   Deno.serve      → HTTP (real public/ tree) + WebSocket   (replaces node http + ws)
//   @sigma/pty-ffi  → spawn commands in a real PTY, stream bytes  (replaces node-pty)
//
// Compiled with `deno compile --include public --include native/<lib>` → one exe.
// Serves the real UI, scans bundles into a plan, detects presence per route, and
// now RUNS per-row actions (install/uninstall/upgrade) live through a PTY. Still
// to land: the global Apply convergence (applyDiff), outdated scan, log/consent
// — see _PLAN.

import { instantiate, libName, Pty } from "@sigma/pty-ffi/noinit";
import { loadBundles, loadProfiles, type Step } from "./bundles.ts";
import { detectPresentDetailed } from "./detect.ts";
import { outdatedFor, scanOutdated } from "./outdated.ts";
import {
  appendHistory,
  clearHistory,
  type ConsentStore,
  readConsent,
  readHistory,
  writeConsent,
} from "./consent.ts";
import { hideConsoleIfHeadless } from "./win-console.ts";
import { makeWatcher, powershellSpawner } from "./watch-window.ts";
// The SHARED decision rule — the SAME file the browser fetches and previews with.
// Server and UI call one actionFor, so they can never drift on what Apply does.
import { actionFor, desiredState } from "../public/decision.js";
import { type DepNode, requiresReason, topoSort } from "./deps.ts";

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
// exe, so a team drops one generic Talos.exe + their own bundles/ side by side
// on the shared OneDrive and gets their installer — no recompile. This is the
// hermeticity boundary made physical: the engine ships empty of content.
//
// Two worlds, two primitives, on purpose:
//   compiled → the exe's own folder, via Deno.execPath() (the REAL path on disk;
//              import.meta.dirname would point INTO the embedded virtual FS).
//   dev      → the project's bundles/ at the REPO root: strip /src off the
//              dirname → the project root, then /bundles (src/ sits directly under
//              the repo root since Phase 3 flattened the old nested talos/talos/).
// It's a fixed name, NOT a setting: convention over configuration closes the
// "where are my bundles?" question instead of reopening it. A missing folder is
// not an error here — an exe with no bundles beside it opens inert (T2 handles
// the empty case; core = proposition only).
const BUNDLES_DIR = Deno.build.standalone
  ? `${Deno.execPath().replace(/[/\\][^/\\]+$/, "")}${
    isWin ? "\\" : "/"
  }bundles`
  : `${(import.meta.dirname ?? ".").replace(/[/\\][^/\\]+$/, "")}/bundles`;
log(`bundles dir: ${BUNDLES_DIR}`);

// Scan the bundles ONCE at startup — pure data, no pty/network, so it's safe to
// do before the engine loads. The result is the `plan` the UI renders on connect
// (bundles → accordion cards, steps → package rows). An empty scan (no bundles/
// beside the exe) yields an empty accordion, not a crash.
const { bundles: BUNDLES, steps: STEPS } = loadBundles(BUNDLES_DIR, log);
log(`plan: ${BUNDLES.length} bundle(s), ${STEPS.length} package(s)`);
// Profiles — named additive package selections (profiles.yaml beside the bundles).
// Pure data sent to the UI; the on/full/hollow logic lives in decision.js. Empty
// list if there's no profiles.yaml (the panel just shows no profile bar).
const { columns: PROFILE_COLUMNS, profiles: PROFILES } = loadProfiles(
  BUNDLES_DIR,
  log,
);

// The consent + install-journal store. Local history always lands in DATA_DIR
// (per machine); a consented copy also lands beside the exe, namespaced by host
// + user, so a team can see who set up what. exeDir = the folder BUNDLES_DIR sits
// in (strip the trailing "bundles"). host/user name the shared log; best-effort
// env reads with plain fallbacks (the shared copy is a bonus, never load-bearing).
const CONSENT: ConsentStore = {
  localDir: DATA_DIR,
  exeDir: BUNDLES_DIR.replace(/[/\\][^/\\]+$/, ""),
  host: (isWin ? Deno.env.get("COMPUTERNAME") : Deno.env.get("HOSTNAME")) ??
    Deno.hostname?.() ?? "host",
  user: (isWin ? Deno.env.get("USERNAME") : Deno.env.get("USER")) ?? "user",
  isWin,
};

// Build the deps.ts view of the plan: each step as a DepNode carrying whether it
// WILL be present after the Apply. `willBePresent[i]` is index-aligned and the
// caller decides how to compute it (present now OR desired-present). Kept here so
// both the connect scan and applyDiff share one shape.
function depNodes(willBePresent: boolean[]): DepNode[] {
  return STEPS.map((s, i) => ({
    name: s.name,
    requires: s.requires,
    willBePresent: willBePresent[i],
  }));
}

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
      headers: {
        "content-type": TYPES[ext] ?? "application/octet-stream",
        // no-store: the panel is served over localhost by an exe that changes on
        // every build. Edge's --app window uses the default profile (persistent
        // disk cache), so without this it serves a STALE index.html/app.js from a
        // previous run — the exe updates but the UI doesn't. Nothing to gain from
        // caching a local asset; kill the whole bug class.
        "cache-control": "no-store",
      },
    });
  } catch {
    return new Response("not found: " + rel, { status: 404 });
  }
}

// Waiting-window watcher cadence: poll the process tree every WAIT_TICK_MS; if the
// pty falls silent longer than WAIT_SILENCE_MS with no window found, suspect a UAC
// prompt (secure desktop → unenumerable). Tuned on the VM.
const WAIT_TICK_MS = 1200;
const WAIT_SILENCE_MS = 10000;

// winget exit codes that mean "nothing to do" — treat as success, not failure.
const BENIGN_CODES = new Set<number>([
  -1978335189, // NO_APPLICABLE_UPGRADE   (already at latest)
  -1978335212, // UPDATE_NOT_APPLICABLE   (already installed, no upgrade)
]);

// Stream a command through a PTY into the WebSocket, base64 per chunk, tagged by
// step index `i` so the UI writes into that row's xterm. Returns the process's
// REAL exit code (0 if the pty never reported one). On Windows we wrap the
// command so PATH is refreshed from the registry first (a tool installed earlier
// in THIS session isn't on the inherited PATH) and the WRAPPED command's exit
// code is what we read (`exit $LASTEXITCODE`), not the shell's own.
async function runInPty(
  ws: WebSocket,
  i: number,
  cmdline: string,
): Promise<number> {
  // Pass the command as an ARGUMENT (-Command / -c), NOT by writing it into an
  // interactive shell. A script given as an argument is NOT echoed — so the
  // xterm shows ONLY the tool's own output, never our PATH-refresh preamble or
  // the `exit` wrapper. We remove our noise at the SOURCE (don't emit it), we do
  // NOT filter the tool's output — that stays verbatim, always.
  // Windows: the PATH refresh + `exit $LASTEXITCODE` live inside the -Command
  // script (invisible), so a tool installed earlier this session is found and we
  // still read the WRAPPED command's exit code, not PowerShell's own.
  const script = isWin
    ? `$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User'); ${cmdline}; exit $LASTEXITCODE`
    : cmdline;
  const args = isWin
    ? ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script]
    : ["-c", script];
  const pty = new Pty(SHELL, { args });
  const b64 = (u8: Uint8Array) => btoa(String.fromCharCode(...u8));

  // Watch for a wizard/consent window popping BEHIND the panel while this step
  // runs (Windows only; inert elsewhere). lastActivity is bumped on every byte;
  // a long silence with no window found → likely UAC (wait-silent). The scan
  // roots at Deno.pid (Talos) — the pty's own pid isn't exposed by the lib.
  let lastActivity = Date.now();
  const watcher = makeWatcher({
    i,
    isWin,
    silenceMs: WAIT_SILENCE_MS,
    spawner: powershellSpawner,
    emit: (m) => {
      try {
        ws.send(JSON.stringify(m));
      } catch { /* socket closing */ }
    },
    now: () => Date.now(),
  });
  const watchTimer = isWin
    ? setInterval(() => watcher.runTick(Deno.pid, lastActivity), WAIT_TICK_MS)
    : null;

  try {
    while (true) {
      const { data, done } = pty.readBytes();
      if (done) break;
      if (data.byteLength) {
        lastActivity = Date.now(); // the tool is talking → not silently waiting
        for (let j = 0; j < data.length; j += 8192) {
          ws.send(
            JSON.stringify({
              type: "out",
              i,
              data: b64(data.subarray(j, j + 8192)),
            }),
          );
        }
      } else {
        await new Promise((r) => setTimeout(r, 10));
      }
    }
    return pty.exitCode ?? 0;
  } finally {
    if (watchTimer !== null) clearInterval(watchTimer);
    watcher.stop(); // always clears the UI banner (like state-done in finally)
  }
}

// Serial lock: only ONE package manager runs at a time, ever. Two winget
// processes fight over a temp file. All actions chain through this queue so they
// never overlap — a per-row button, a global Apply, even a stray double-click.
let actionQueue: Promise<unknown> = Promise.resolve();
function serialize<T>(fn: () => Promise<T>): Promise<T> {
  const run = actionQueue.then(fn, fn);
  actionQueue = run.catch(() => {}); // one failure never breaks the chain
  return run;
}

// Best-effort version capture for the journal: run the step's detect command
// (e.g. `git --version`) and pull the first version-like token from its output.
// No detect command, non-zero exit, or any error → "" (the journal just omits a
// version). Silent + never throws — a version string is a nicety, not load-bearing.
async function captureVersion(s: Step): Promise<string> {
  if (!s.detect) return "";
  try {
    // Run the detect command capturing stdout (the silent presenceProbe only
    // keeps the exit code — here we want the output to pull a version from it).
    const { code, stdout } = await new Deno.Command(
      isWin ? "powershell.exe" : "/bin/sh",
      {
        args: isWin ? ["-NoProfile", "-Command", s.detect] : ["-c", s.detect],
        stdout: "piped",
        stderr: "null",
        stdin: "null",
      },
    ).output();
    if (code !== 0) return "";
    const m = new TextDecoder().decode(stdout).match(/\d+\.\d+(?:\.\d+)?/);
    return m ? m[0] : ""; // 2.55 / 1.104.1 …
  } catch {
    return "";
  }
}

// Run one step's install/uninstall/upgrade command and show it live. winget/npm
// are idempotent (they handle "already there / already gone" via a benign exit
// code), so no pre-probe. Emits `step` running → settled, streams output, and a
// terminal `done` so the UI unlocks. Presence after the action is re-detected on
// the next connect / Apply (detect, don't remember).
async function doStep(
  ws: WebSocket,
  i: number,
  action: "install" | "uninstall" | "upgrade",
): Promise<boolean> {
  const s = STEPS[i];
  if (!s) return false;
  const cmd = s[action];
  if (!cmd) {
    log(`step ${i} ${action}: no command for this route — skipping`);
    return false;
  }
  busy++;
  try {
    const running = action === "uninstall"
      ? "uninstalling"
      : action === "upgrade"
      ? "upgrading"
      : "installing";
    ws.send(JSON.stringify({ type: "step", i, status: running }));
    const code = await runInPty(ws, i, cmd);
    const ok = code === 0 || BENIGN_CODES.has(code);
    log(
      `step ${i} ${action} ${ok ? "OK" : "FAILED"} (exit ${code}): ${s.name}`,
    );
    // Show the exit code in the row terminal — a "failed" with a specific winget
    // code (e.g. "already installed, no upgrade") is diagnosable at a glance, and
    // tells us which benign codes to add.
    try {
      const enc = new TextEncoder();
      const line = `\r\n\x1b[2m[${action}] exit ${code} → ${
        ok ? "ok" : "failed"
      }\x1b[0m\r\n`;
      ws.send(JSON.stringify({
        type: "out",
        i,
        data: btoa(String.fromCharCode(...enc.encode(line))),
      }));
    } catch { /* socket closing */ }
    ws.send(JSON.stringify({
      type: "step",
      i,
      status: ok ? (action === "uninstall" ? "absent" : "ok") : "fail",
    }));
    // Journal the outcome (local always; shared copy iff consented). Version is
    // best-effort — captured from the detect command after a successful
    // install/upgrade, blank otherwise. Never let a journal failure sink a step.
    const version = (ok && action !== "uninstall")
      ? await captureVersion(s)
      : "";
    appendHistory(CONSENT, {
      at: new Date().toISOString(),
      package: s.name,
      version,
      action,
      ok,
    });
    return ok;
  } finally {
    busy--;
  }
}

// Apply the tri-state DECISION against what's actually on the machine.
//   on  = indices the user wants PRESENT (yellow ☑).
//   off = indices the user wants ABSENT (yellow ☒).
// The UI sends a desired state for EVERY package (Model A): on = want present,
// off = want absent. We RE-DETECT presence now (detect, don't remember) and ask
// the SHARED actionFor what to do per package — the same rule the front previewed
// with, so server and UI cannot drift. `scope` (optional) restricts which indices
// may act (a per-bundle Apply passes its packages; a global Apply passes none →
// everything in scope).
//
// T3b: Apply re-runs the machine-wide `winget upgrade` scan alongside the
// presence re-scan (repaint-at-apply: re-constate reality before acting, don't
// trust the CONNECT scan) and feeds the fresh `outdated` flag into actionFor, so
// a present-but-stale wanted package now UPGRADES instead of being left as-is.
//
// Order: steps run in INDEX order, which is VISUAL order (steps sorted by bundle
// priority in loadBundles) — the plan follows the screen top-to-bottom. Runs
// INSIDE serialize() at the call site, so the whole convergence holds the single
// package-manager lock end to end.
async function applyDiff(
  ws: WebSocket,
  on: number[],
  off: number[],
  scope: number[] | null,
) {
  const wantOn = new Set(on);
  const wantOff = new Set(off);
  const inScope = scope ? new Set(scope) : null;
  const acts = (i: number) => !inScope || inScope.has(i);

  const [presences, scan] = await Promise.all([
    Promise.all(STEPS.map((s) => detectPresentDetailed(s, isWin))),
    scanOutdated(isWin),
  ]);

  // Future state per package for THIS Apply: present now OR the user wants it on
  // (wantOn), and never if they want it off (wantOff). Feeds requires resolution.
  const willBePresent = STEPS.map((_s, i) =>
    !wantOff.has(i) && (presences[i].present === true || wantOn.has(i))
  );
  const nodes = depNodes(willBePresent);

  // Push the fresh scan back to the screen BEFORE we act. Without this the pills
  // still show the CONNECT scan, so the plan could act on a reality the user never
  // saw (e.g. a tool they removed by hand since opening). Not a diff, no dialog —
  // just re-align: the pills correct themselves, then focus mode shows the plan.
  const sendWs = (obj: unknown) => {
    try {
      ws.send(JSON.stringify(obj));
    } catch { /* socket closing */ }
  };
  presences.forEach((r, i) => {
    try {
      const reason = requiresReason(nodes[i], nodes) ?? r.reason;
      ws.send(
        JSON.stringify({
          type: "state",
          i,
          present: r.present,
          reason,
          version: r.version,
          external: r.external,
        }),
      );
      sendDiag(sendWs, i, r.diag); // show the probe result in the row terminal
      if (r.present === true) {
        const od = outdatedFor(STEPS[i].wingetId, scan);
        if (od) ws.send(JSON.stringify({ type: "outdated", i, ...od }));
      }
    } catch { /* socket closing */ }
  });

  // Per package: desired (on→present, off→absent, neither→auto=untouched) vs
  // machine reality → the action. present:null (indeterminate) is treated as
  // "not known present" i.e. false, the safe direction for install.
  const actionOf = (i: number): string | null => {
    if (!acts(i)) return null;
    const desired = wantOn.has(i)
      ? "present"
      : wantOff.has(i)
      ? "absent"
      : null;
    if (!desired) return null; // auto → never touched
    return actionFor(desired, {
      present: presences[i].present === true,
      outdated: outdatedFor(STEPS[i].wingetId, scan) !== null,
      canUninstall: !!STEPS[i].uninstall,
    });
  };

  // Walk the steps IN INDEX ORDER — which is now VISUAL order (steps sorted by
  // bundle priority in loadBundles). The plan reads top-to-bottom exactly as the
  // user sees it on screen, instead of jumping around by action type. Each step
  // carries whatever action it needs (install / upgrade / uninstall) in place.
  // No node/selfHost exception: the Deno exe is self-contained, nothing we act on
  // hosts the panel, so any package can act in any order.
  type Act = "install" | "uninstall" | "upgrade";
  const visualPlan = [...STEPS.keys()]
    .map((i) => ({ i, action: actionOf(i) as Act | null }))
    .filter((p): p is { i: number; action: Act } => p.action !== null);
  // Order by dependency: a required package installs BEFORE its dependents.
  // Visual (bundle-priority) order breaks ties between independents, so the plan
  // still reads top-to-bottom wherever `requires` doesn't force otherwise.
  const plan = topoSort(visualPlan, nodes);

  log(
    `apply diff: ${
      plan.map((p) => `${p.action[0]}:${STEPS[p.i].name}`).join(", ") || "—"
    }`,
  );
  if (!plan.length) {
    ws.send(JSON.stringify({ type: "done", nothing: true }));
    return;
  }
  // Tell the UI the WHOLE plan up front, in execution order (= screen order), so
  // it can show every step that WILL run and follow progress straight down the
  // list.
  ws.send(JSON.stringify({ type: "apply-plan", plan }));
  for (const p of plan) await doStep(ws, p.i, p.action);
  log("apply done");
  ws.send(JSON.stringify({ type: "done" }));
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

// Surface a detection/check probe's result in the ROW terminal — the command it
// ran, its exit code, and its output. Sent as an `out` message (same channel as
// a live install), so the user can SEE why a package reads present/absent instead
// of trusting a silent pill. One dimmed header line, then the raw output.
function sendDiag(
  send: (obj: unknown) => void,
  i: number,
  diag: { cmdline: string; code: number; output: string } | undefined,
) {
  if (!diag) return;
  const enc = new TextEncoder();
  const b64 = (s: string) => btoa(String.fromCharCode(...enc.encode(s)));
  const verdict = diag.code === 0 ? "present" : "absent";
  const header =
    `\x1b[2m$ ${diag.cmdline}\r\n[check] exit ${diag.code} → ${verdict}\x1b[0m\r\n`;
  const body = diag.output.trim()
    ? diag.output.replace(/\r?\n/g, "\r\n").replace(/\r\n$/, "") + "\r\n"
    : "";
  send({ type: "out", i, data: b64(header + body) });
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
    // Presence and the outdated scan run in PARALLEL — one machine-wide
    // `winget upgrade` alongside the N presence probes. The scan is best-effort
    // (scanOutdated never rejects → empty map on any hiccup), so it can only add
    // "outdated" lights, never block or break the presence pass.
    const [results, scan] = await Promise.all([
      Promise.all(STEPS.map((s) => detectPresentDetailed(s, isWin))),
      scanOutdated(isWin),
    ]);
    // Future state per package: present now OR desired-present at rest (posture
    // default — no user toggle yet at connect). Feeds requires resolution so a
    // dependent isn't wrongly flagged for a package that WILL be installed.
    const willBePresent = STEPS.map((s, i) =>
      results[i].present === true ||
      desiredState(s.posture, null) === "present"
    );
    const nodes = depNodes(willBePresent);
    // present is true / false / null(indeterminate); reason explains an unmet
    // dependency (future state), else detection's own reason.
    results.forEach((r, i) => {
      const reason = requiresReason(nodes[i], nodes) ?? r.reason;
      send({
        type: "state",
        i,
        present: r.present,
        reason,
        version: r.version,
        external: r.external,
      });
      sendDiag(send, i, r.diag); // show the probe's command+exit+output in the row
      // A stale-but-present package lights its Apply button + shows cur→avail.
      if (r.present === true) {
        const od = outdatedFor(STEPS[i].wingetId, scan);
        if (od) send({ type: "outdated", i, ...od });
      }
    });
    const yes = results.filter((r) => r.present === true).length;
    const unknown = results.filter((r) => r.present === null).length;
    log(
      `detect: ${yes}/${results.length} present, ${unknown} indeterminate, ` +
        `${scan.size} outdated`,
    );
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
        // its index `i`, the id every later message keys off). Consent is REAL:
        // read from the machine-local store — undecided on first boot makes the UI
        // pop the share dialog. (Persisted selection = T3, still neutral.)
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
          profiles: PROFILES,
          profileColumns: PROFILE_COLUMNS,
          consent: readConsent(CONSENT),
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
        // Per-row action: install / uninstall / upgrade a single package. Each
        // runs serialized (never two package managers at once) and ALWAYS ends
        // with `done` so the UI unlocks — even on failure. Apply (the global
        // convergence) lands next; this is the single-row gesture.
        const ROW_ACTIONS = ["install", "uninstall", "upgrade"] as const;
        type RowAction = typeof ROW_ACTIONS[number];
        if (
          ROW_ACTIONS.includes(msg.type) && typeof msg.i === "number" &&
          STEPS[msg.i]
        ) {
          if (!ptyReady) {
            // Clicked before the engine finished loading — tell the UI, don't
            // crash. (Rare: loading is fast; only a very eager click.)
            socket.send(JSON.stringify({ type: "starting" }));
            return;
          }
          const action = msg.type as RowAction;
          serialize(() => doStep(socket, msg.i, action))
            .catch((e) => log(`${action} error: ${(e as Error).message}`))
            .finally(() => socket.send(JSON.stringify({ type: "done" })));
        } else if (msg.type === "apply") {
          if (!ptyReady) {
            socket.send(JSON.stringify({ type: "starting" }));
            return;
          }
          // The global (or per-bundle) convergence. on/off = the UI's desired
          // states; scope (optional) limits which indices may act. Serialized so
          // the whole run holds the package-manager lock end to end. applyDiff
          // emits its own terminal `done` (or done+nothing).
          serialize(() =>
            applyDiff(socket, msg.on ?? [], msg.off ?? [], msg.scope ?? null)
          ).catch((e) => {
            log(`apply error: ${(e as Error).message}`);
            socket.send(JSON.stringify({ type: "done" }));
          });
        } else if (msg.type === "rescan") {
          // Refresh: re-constate the machine on demand (after a manual install, a
          // crash, an external change). Same scan as connect — presence is asked
          // live, never remembered. Touches only detection, not the user's
          // selection (that's what Reset is for).
          detectAll(socket);
        } else if (msg.type === "set-consent") {
          // Record the share choice (marks consent decided). No reply needed —
          // the UI already closed its dialog / flipped its toggle optimistically.
          writeConsent(CONSENT, msg.share === true);
          log(`consent set: share=${msg.share === true}`);
        } else if (msg.type === "get-log") {
          // The Log tab asks for the current consent + local history.
          socket.send(JSON.stringify({
            type: "log",
            consent: readConsent(CONSENT),
            history: readHistory(CONSENT),
          }));
        } else if (msg.type === "clear-log") {
          // Clear the LOCAL journal only (the shared team copy is left intact),
          // then send the now-empty log back so the tab refreshes.
          clearHistory(CONSENT);
          log("local history cleared");
          socket.send(JSON.stringify({
            type: "log",
            consent: readConsent(CONSENT),
            history: readHistory(CONSENT),
          }));
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
