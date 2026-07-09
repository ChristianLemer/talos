// detect.ts — is a package present on THIS machine, right now?
//
// "Detect, don't remember": we ASK the machine, never a journal. A package can
// have arrived before Talos or via another tool — the truth lives on disk, so we
// probe it live. This lights the accordion's installed/absent pills.
//
// Presence is DERIVED FROM THE ROUTE, not from a hand-written source-coupled
// command (the earlier bug: Obsidian's `detect: winget list …` reduced to its
// first token `winget`, which always exists → false "present" on Windows, and
// can't run at all on Mac). Two natures, matching the route/capability model:
//
//   1. The package names a BINARY (`detect: node --version`): presence = "is the
//      binary on PATH?" — SOURCE-AGNOSTIC, the best signal (answers "is it
//      usable?" no matter how it arrived: winget, brew, cargo, by hand).
//   2. No binary (GUI apps like Obsidian): presence = ask the PRACTICABLE ROUTE
//      here (winget list on Windows). No practicable route → INDETERMINATE (null),
//      NOT "absent" — Mac has no winget, so it can't know, and mustn't lie.
//
// Silent throughout: we read EXIT CODES only, never show output (a probe is
// plumbing). Command BUILDING is pure (…Probe fns) → unit-testable; execution is
// the thin IO shell (detectPresent). Since the headless console is hidden
// (win-console.ts) these child spawns no longer flash a window.

import type { Step } from "./bundles.ts";
import { pluginPresent, skillPresent } from "./agent-content.ts";

export interface Probe {
  cmd: string;
  args: string[];
}

// Presence plus an optional human reason (why it's indeterminate). detectPresent
// stays the boolean|null API everyone uses; detectPresentDetailed adds the reason
// for the UI to show on an indeterminate row.
export interface Presence {
  present: boolean | null;
  reason?: string;
  diag?: ProbeResult; // the probe's command + exit code + output, for the terminal
  version?: string; // the installed version the probe revealed (constated live)
  // Provenance flag (NOT a fourth presence state — the tri-state stays sacred):
  // the binary responds but winget doesn't know it → installed OUTSIDE winget, so
  // winget can't upgrade/uninstall it. present stays true; this just says HOW.
  external?: boolean;
}

// PATH refresh for Windows: a winget install writes the registry but does NOT
// propagate PATH to already-running processes (the panel inherited a stale PATH),
// so a freshly-installed tool would read as absent without this.
const WIN_PATH_REFRESH =
  "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

// --- nature 1: is a BINARY present? RUN the author's detect command ---------
// Build the presence probe by RUNNING the full `detect` command as written
// (e.g. "node --version"): exit 0 = present, and its output carries the VERSION
// (versionFrom parses it) — the author wrote "--version" on purpose, so we use
// both signals from one call instead of throwing the version away. A missing
// binary makes the command fail (non-zero / not found) → absent, the safe way.
// (Detect is only ever a "<bin> --version"-style command for binary routes;
// GUI apps with no CLI use the winget route probe, not this — see routeProbe.)
export function presenceProbe(
  detectCmd: string | null,
  isWin: boolean,
): Probe | null {
  const cmd = (detectCmd ?? "").trim();
  if (!cmd) return null;
  if (!isWin) {
    return { cmd: "/bin/sh", args: ["-c", cmd] };
  }
  // CRITICAL: a MISSING command raises CommandNotFoundException, which does NOT
  // set $LASTEXITCODE — so "cmd; exit $LASTEXITCODE" would exit 0 (the prior
  // value) and read an absent tool as PRESENT (the rg/fd/bat false-positive).
  // Wrap in try/catch with Stop so a not-found (or any failure) → exit 127.
  const ps =
    `${WIN_PATH_REFRESH} $ErrorActionPreference='Stop'; try { ${cmd}; exit $LASTEXITCODE } catch { exit 127 }`;
  return { cmd: "powershell.exe", args: ["-NoProfile", "-Command", ps] };
}

// --- nature 1b: run a package's own CHECK command (dry-run) -----------------
// For the `run` route, a config-atom carries a `check:` — a command run VERBATIM
// (not reduced to a PATH probe) whose EXIT CODE is the answer: 0 = converged/
// present, non-zero = absent or drifted (→ "apply"). The check shares the atom's
// apply-logic (it inspects only the managed block), so detection can't drift from
// application, and a user's own edits around the block never read as "present".
// Run in the platform shell exactly like install, so `nu`/tools resolve the same.
export function checkProbe(check: string | null, isWin: boolean): Probe | null {
  const cmd = (check ?? "").trim();
  if (!cmd) return null;
  if (!isWin) return { cmd: "/bin/sh", args: ["-c", cmd] };
  // Same guard as presenceProbe: a missing command must fail loud (127), not
  // inherit a stale $LASTEXITCODE=0 and read as "converged/present".
  const ps =
    `${WIN_PATH_REFRESH} $ErrorActionPreference='Stop'; try { ${cmd}; exit $LASTEXITCODE } catch { exit 127 }`;
  return { cmd: "powershell.exe", args: ["-NoProfile", "-Command", ps] };
}

// --- nature 2: ask the ROUTE (for packages with no CLI binary) --------------
// Build the "is this installed?" probe for a package's declared route, on THIS
// os. Returns null when no route is practicable here (→ indeterminate). Today
// only the winget route is implemented (Windows-focus); brew/cargo/npm route
// detection lands with their platforms (Mac test bench) — until then those fall
// through to null rather than guessing.
export function routeProbe(step: Step, isWin: boolean): Probe | null {
  if (step.route === "winget" && step.wingetId) {
    if (!isWin) return null; // no winget off Windows → can't determine here
    // `winget list --id X --exact` exits 0 iff installed. --source winget skips
    // msstore (timed out on the VM); --accept-source-agreements avoids a prompt.
    // NO `| Out-Null`: we now READ the output (it carries the Version column), so
    // the table must reach us — only the exit code decides presence.
    const ps =
      `${WIN_PATH_REFRESH} $ErrorActionPreference='Stop'; try { winget list --id ${step.wingetId} --exact --source winget --accept-source-agreements; exit $LASTEXITCODE } catch { exit 127 }`;
    return { cmd: "powershell.exe", args: ["-NoProfile", "-Command", ps] };
  }
  return null; // brew/cargo/npm/run route detection not implemented yet
}

// The LIST command for a content-detected route (claude-plugin / skill). Pure;
// returns null for routes detected by exit code. Detection reads this command's
// STDOUT (not its exit code — `claude plugin list` exits 0 either way), so it's
// kept separate from Probe/runProbe.
export function listProbe(step: Step, _isWin: boolean): Probe | null {
  if (step.route === "claude-plugin") {
    return { cmd: "claude", args: ["plugin", "list", "--json"] };
  }
  if (step.route === "skill") {
    return { cmd: "npx", args: ["skills", "list", "-g"] };
  }
  return null;
}

// Run a list command and return its stdout ("" on any failure → indeterminate).
async function runList(probe: Probe): Promise<string> {
  try {
    const { stdout } = await new Deno.Command(probe.cmd, {
      args: probe.args,
      stdout: "piped",
      stderr: "null",
      stdin: "null",
    }).output();
    return new TextDecoder().decode(stdout);
  } catch {
    return "";
  }
}

async function runProbe(probe: Probe): Promise<boolean> {
  return (await runProbeDetailed(probe)).ok;
}

// Like runProbe but CAPTURES the exit code and output, for surfacing a probe's
// result in the row terminal (diagnosis). Merges stdout+stderr into one string.
// A spawn failure (e.g. the tool not found) → ok:false, code:-1, output:message.
export interface ProbeResult {
  ok: boolean;
  code: number;
  output: string;
  cmdline: string;
}
export async function runProbeDetailed(probe: Probe): Promise<ProbeResult> {
  const cmdline = `${probe.cmd} ${probe.args.join(" ")}`;
  try {
    const { code, stdout, stderr } = await new Deno.Command(probe.cmd, {
      args: probe.args,
      stdout: "piped",
      stderr: "piped",
      stdin: "null",
    }).output();
    const dec = new TextDecoder();
    const output = dec.decode(stdout) + dec.decode(stderr);
    return { ok: code === 0, code, output, cmdline };
  } catch (e) {
    return { ok: false, code: -1, output: (e as Error).message, cmdline };
  }
}

// Extract the INSTALLED version a probe's output already reveals — the version is
// free data the presence probe reads and we'd otherwise throw away. Pure + per
// route (each probe prints differently); unknown format → "". "Detect, don't
// remember": a version constated live beats one journalled after an install.
//   bundle `version-regex` → an OPTIONAL override: the bundle supplies a regex,
//            capture group 1 (or the whole match) IS the version. Added ONLY when
//            the per-route default gets it wrong — the displayed version is the
//            diagnostic that reveals which package needs one. No shell, pure JS.
//   winget → `winget list --id X` prints "name  Id  Version"; grab the token
//            after the id (case-insensitive id match).
//   binary → `git --version` → "git version 2.43.0"; grab the first version-like
//            token (digits.dots, optionally more).
export function versionFrom(step: Step, output: string): string {
  const clean = output
    // deno-lint-ignore no-control-regex -- strip ANSI so tokens split cleanly
    .replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
  // A bundle-supplied regex wins — group 1 if present, else the whole match.
  if (step.versionRegex) {
    try {
      const m = clean.match(new RegExp(step.versionRegex));
      if (m) return (m[1] ?? m[0]).trim();
    } catch {
      // a malformed regex in a bundle → ignore, fall through to the default
    }
    return "";
  }
  // Winget table: the token after the id (when the output IS a winget list). A
  // package can be route=winget yet detected via `detect` (e.g. Helix has both) —
  // then the output is "helix 25.07.1", not a winget table, so the id column isn't
  // found. Don't give up: fall through to the generic token match below.
  if (step.route === "winget" && step.wingetId) {
    const id = step.wingetId.toLowerCase();
    for (const line of clean.split(/\r?\n/)) {
      const cols = line.trim().split(/\s{1,}/);
      const at = cols.findIndex((c) => c.toLowerCase() === id);
      if (at >= 0 && cols[at + 1]) return cols[at + 1];
    }
  }
  // Generic: first version-like token in the output (e.g. a `--version` line).
  // No leading \b — a "v" prefix (node's "v26.4.0") shares a word boundary with
  // the digit, which made \b backtrack to "4.0". Match the full dotted number.
  const m = clean.match(/\d+(?:\.\d+)+(?:[-.\w]*)?/);
  return m ? m[0] : "";
}

// Presence of a package on this machine, WITH an optional reason when it's
// indeterminate. `requires` is NOT handled here — it's a package-level FUTURE-STATE
// dependency resolved globally in deps.ts, not a present-tense per-step check.
//   - has a `detect` binary (non content-detected routes) → PATH probe.
//   - content-detected routes (claude-plugin/skill) → parse the tool's list.
//   - exit-code routes (winget) → route probe.
// Never throws: a spawn failure resolves to "absent"/indeterminate, the safe way.
export async function detectPresentDetailed(
  step: Step,
  isWin: boolean,
): Promise<Presence> {
  // A `check` command (run-route config-atoms) is the definitive per-atom signal:
  // run it verbatim, exit 0 = converged/present. Takes priority over `detect`.
  if (step.check) {
    const probe = checkProbe(step.check, isWin);
    if (probe) {
      const d = await runProbeDetailed(probe);
      return { present: d.ok, diag: d };
    }
  }
  if (step.detect && step.route !== "claude-plugin" && step.route !== "skill") {
    // Detection and version are TWO INDEPENDENT questions — probe both sources,
    // then combine. Coupling them (winget only if the binary responded) wrongly
    // called a winget-installed-but-not-on-PATH package "absent", and let a stale
    // "external" survive. So: ask the binary, ask winget, then decide.
    const binProbe = presenceProbe(step.detect, isWin);
    const bin = binProbe ? await runProbeDetailed(binProbe) : null;
    const binOk = !!bin?.ok;

    const wp = routeProbe(step, isWin); // null when no winget route practicable here
    const wd = wp ? await runProbeDetailed(wp) : null;
    const wingetOk = !!wd?.ok;

    // present = either source finds it. diag prefers the binary (source-agnostic,
    // what the user runs); falls back to winget's output when there's no binary.
    const present = binOk || wingetOk;
    const diag = bin ?? wd ?? undefined;
    if (!present) return { present: false, diag };

    // version: winget's clean table column when winget MANAGES it; else the
    // binary's own output. external = the binary works but winget doesn't know it
    // (installed outside winget → winget can't upgrade/uninstall). Not a new
    // presence state — present stays true, this flags HOW.
    if (wingetOk) {
      return { present: true, diag, version: versionFrom(step, wd!.output) };
    }
    return {
      present: true,
      diag,
      version: bin ? versionFrom(step, bin.output) : "",
      external: !!wp, // there IS a winget route, yet winget doesn't list it
    };
  }
  // Content-detected routes: parse the tool's list output (exit code is useless).
  const list = listProbe(step, isWin);
  if (list) {
    const out = await runList(list);
    if (!out) return { present: null }; // tool absent/failed → indeterminate
    const detect = step.detect ?? "";
    const present = step.route === "claude-plugin"
      ? pluginPresent(detect, out)
      : skillPresent(detect, out);
    return { present };
  }
  // Exit-code routes (winget today). Its output lists the version — capture it
  // (versionFrom applies the bundle's version-regex override if present).
  const probe = routeProbe(step, isWin);
  if (!probe) return { present: null };
  const d = await runProbeDetailed(probe);
  return { present: d.ok, diag: d, version: versionFrom(step, d.output) };
}

// The boolean|null API everyone already uses — delegates to the detailed one.
export async function detectPresent(
  step: Step,
  isWin: boolean,
): Promise<boolean | null> {
  return (await detectPresentDetailed(step, isWin)).present;
}
