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
import { type Os, type Probe, shellProbe } from "./platform.ts";
import { MANAGERS, nativeManager } from "./managers.ts";

export type { Probe }; // re-export for existing importers of detect.ts

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

// --- nature 1: is a BINARY present? RUN the author's detect command ---------
// Build the presence probe by RUNNING the full `detect` command as written
// (e.g. "node --version"): exit 0 = present, and its output carries the VERSION
// (versionFrom parses it) — the author wrote "--version" on purpose, so we use
// both signals from one call instead of throwing the version away. A missing
// binary makes the command fail (non-zero / not found) → absent, the safe way.
// (Detect is only ever a "<bin> --version"-style command for binary routes;
// GUI apps with no CLI use the system route probe, not this — see systemProbe.)
// Delegates the shell-wrapping (+ Windows 127 guard) to Platform.shellProbe.
export function presenceProbe(
  detectCmd: string | null,
  os: Os,
): Probe | null {
  const cmd = (detectCmd ?? "").trim();
  if (!cmd) return null;
  return shellProbe(os, cmd);
}

// --- nature 1b: run a package's own CHECK command (dry-run) -----------------
// For the `run` route, a config-atom carries a `check:` — a command run VERBATIM
// (not reduced to a PATH probe) whose EXIT CODE is the answer: 0 = converged/
// present, non-zero = absent or drifted (→ "apply"). The check shares the atom's
// apply-logic (it inspects only the managed block), so detection can't drift from
// application, and a user's own edits around the block never read as "present".
// Run in the platform shell exactly like install, so `nu`/tools resolve the same.
export function checkProbe(check: string | null, os: Os): Probe | null {
  const cmd = (check ?? "").trim();
  if (!cmd) return null;
  return shellProbe(os, cmd);
}

// --- nature 2: ask the SYSTEM-MANAGER route (packages with no CLI binary) ----
// Build the presence probe for a package's system-manager route, on THIS os.
// Delegates to the native SystemManager's presenceCommand via Platform. Returns
// null when the package's route is not the manager native here (→ indeterminate,
// never a guessed "absent"). Replaces the old winget-only routeProbe.
export function systemProbe(step: Step, os: Os): Probe | null {
  const mgr = nativeManager(os);
  if (!mgr || step.route !== mgr.route || !step.systemId) return null;
  return shellProbe(os, mgr.presenceCommand(step.systemId));
}

// The LIST command for a content-detected route (claude-plugin / skill). Pure;
// returns null for routes detected by exit code. Detection reads this command's
// STDOUT (not its exit code — `claude plugin list` exits 0 either way), so it's
// kept separate from Probe/runProbe.
export function listProbe(step: Step, _os: Os): Probe | null {
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

// CAPTURES the exit code and output, for surfacing a probe's
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
  // System-manager table/line: delegate to whichever manager could own this route.
  // A package can be route=winget yet detected via `detect` (e.g. Helix has both) —
  // then the output is "helix 25.07.1", not a manager table, so the id column isn't
  // found. Don't give up: fall through to the generic token match below.
  const mgr = MANAGERS.find((m) => m.route === step.route);
  if (mgr && step.systemId) {
    const v = mgr.parseVersion(step.systemId, clean);
    if (v) return v;
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
//   - exit-code routes (system managers: winget/brew) → system probe.
// Never throws: a spawn failure resolves to "absent"/indeterminate, the safe way.
export async function detectPresentDetailed(
  step: Step,
  os: Os,
): Promise<Presence> {
  // A `check` command (run-route config-atoms) is the definitive per-atom signal:
  // run it verbatim, exit 0 = converged/present. Takes priority over `detect`.
  if (step.check) {
    const probe = checkProbe(step.check, os);
    if (probe) {
      const d = await runProbeDetailed(probe);
      return { present: d.ok, diag: d };
    }
  }
  if (step.detect && step.route !== "claude-plugin" && step.route !== "skill") {
    // Detection and version are TWO INDEPENDENT questions — probe both sources,
    // then combine. Coupling them (the manager only if the binary responded)
    // wrongly called a manager-installed-but-not-on-PATH package "absent", and let
    // a stale "external" survive. So: ask the binary, ask the manager, then decide.
    const binProbe = presenceProbe(step.detect, os);
    const bin = binProbe ? await runProbeDetailed(binProbe) : null;
    const binOk = !!bin?.ok;

    const sp = systemProbe(step, os); // null when no native manager route here
    const sd = sp ? await runProbeDetailed(sp) : null;
    const systemOk = !!sd?.ok;

    // present = either source finds it. diag prefers the binary (source-agnostic,
    // what the user runs); falls back to the manager's output when there's no binary.
    const present = binOk || systemOk;
    const diag = bin ?? sd ?? undefined;
    if (!present) return { present: false, diag };

    // version from the manager when it manages the package; else the binary.
    // external = binary works but the native manager doesn't list it (installed
    // outside it → can't upgrade/uninstall). present stays true; flags HOW.
    if (systemOk) {
      return { present: true, diag, version: versionFrom(step, sd!.output) };
    }
    return {
      present: true,
      diag,
      version: bin ? versionFrom(step, bin.output) : "",
      external: !!sp, // there IS a native manager route, yet it doesn't list it
    };
  }
  // Content-detected routes: parse the tool's list output (exit code is useless).
  const list = listProbe(step, os);
  if (list) {
    const out = await runList(list);
    if (!out) return { present: null }; // tool absent/failed → indeterminate
    const detect = step.detect ?? "";
    const present = step.route === "claude-plugin"
      ? pluginPresent(detect, out)
      : skillPresent(detect, out);
    return { present };
  }
  // Exit-code routes (system managers). Their output lists the version — capture it
  // (versionFrom applies the bundle's version-regex override if present).
  const probe = systemProbe(step, os);
  if (!probe) return { present: null };
  const d = await runProbeDetailed(probe);
  return { present: d.ok, diag: d, version: versionFrom(step, d.output) };
}

// The boolean|null API everyone already uses — delegates to the detailed one.
export async function detectPresent(
  step: Step,
  os: Os,
): Promise<boolean | null> {
  return (await detectPresentDetailed(step, os)).present;
}
