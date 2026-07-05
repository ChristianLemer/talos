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

export interface Probe {
  cmd: string;
  args: string[];
}

// PATH refresh for Windows: a winget install writes the registry but does NOT
// propagate PATH to already-running processes (the panel inherited a stale PATH),
// so a freshly-installed tool would read as absent without this.
const WIN_PATH_REFRESH =
  "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

// --- nature 1: is a BINARY on PATH? (source-agnostic) -----------------------
// Build the silent presence probe for a `detect` command's binary (its first
// token). Returns null when there's nothing to probe.
export function presenceProbe(
  detectCmd: string | null,
  isWin: boolean,
): Probe | null {
  const bin = (detectCmd ?? "").trim().split(/\s+/)[0];
  if (!bin) return null;
  if (!isWin) {
    return { cmd: "/bin/sh", args: ["-c", `command -v '${bin}'`] };
  }
  const ps =
    `${WIN_PATH_REFRESH} if (Get-Command '${bin}' -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }`;
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
    const ps =
      `${WIN_PATH_REFRESH} winget list --id ${step.wingetId} --exact --source winget --accept-source-agreements | Out-Null; exit $LASTEXITCODE`;
    return { cmd: "powershell.exe", args: ["-NoProfile", "-Command", ps] };
  }
  return null; // brew/cargo/npm/run route detection not implemented yet
}

async function runProbe(probe: Probe): Promise<boolean> {
  try {
    const { code } = await new Deno.Command(probe.cmd, {
      args: probe.args,
      stdout: "null",
      stderr: "null",
      stdin: "null",
    }).output();
    return code === 0;
  } catch {
    return false;
  }
}

// Presence of a package on this machine: true / false / null(indeterminate).
//   - has a `detect` binary → PATH probe (agnostic). true|false, never null.
//   - no binary → route probe. true|false if the route is practicable here,
//     else null (we can't know — don't claim absent).
// Never throws: a spawn failure resolves to "absent", the safe direction for a
// binary probe.
export async function detectPresent(
  step: Step,
  isWin: boolean,
): Promise<boolean | null> {
  if (step.detect) {
    const probe = presenceProbe(step.detect, isWin);
    if (!probe) return false;
    return await runProbe(probe);
  }
  const probe = routeProbe(step, isWin);
  if (!probe) return null; // no way to constate presence here → indeterminate
  return await runProbe(probe);
}
