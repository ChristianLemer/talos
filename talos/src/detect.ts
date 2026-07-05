// detect.ts — is a package present on THIS machine, right now?
//
// "Detect, don't remember": we ASK the machine, never a journal. A package can
// have arrived before Talos or via another tool — the truth lives on disk, so we
// probe it live (same spirit as winget's idempotence). This is the presence
// check that lights the accordion's installed/absent pills.
//
// Presence = "is the package's binary on PATH?", tested by the FIRST token of the
// bundle.yaml `detect` command (e.g. "node --version" → probe "node"). Silent:
// we read the EXIT CODE only, never show output (a probe is plumbing, not user
// info). The command-BUILDING is pure (presenceProbe) so it's unit-testable;
// the execution (detectPresent) is the thin IO shell around it.
//
// NOTE — this is binary-presence, route-agnostic. Detecting BY WHICH route a
// package is present (detectRoutes, for uninstall) is a richer T3 concern; T2b
// only needs "present or not" to paint the pills and gate the splash.

export interface Probe {
  cmd: string;
  args: string[];
}

// Build the silent presence probe for a `detect` command's binary. Returns null
// when there's nothing to probe (no detect field / empty). On Windows we refresh
// PATH from the registry first — a winget install writes the registry but does
// NOT propagate PATH to already-running processes (the panel inherited a stale
// PATH), so Get-Command would miss freshly-installed tools without this.
export function presenceProbe(
  detectCmd: string | null,
  isWin: boolean,
): Probe | null {
  const bin = (detectCmd ?? "").trim().split(/\s+/)[0];
  if (!bin) return null;
  if (!isWin) {
    // command -v: POSIX "is this on PATH?", exit 0 if found. Quote to be safe.
    return { cmd: "/bin/sh", args: ["-c", `command -v '${bin}'`] };
  }
  const refresh =
    "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";
  const ps =
    `${refresh} if (Get-Command '${bin}' -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }`;
  return { cmd: "powershell.exe", args: ["-NoProfile", "-Command", ps] };
}

// Run the probe: true iff the binary is found (exit 0). Never throws — a spawn
// failure or missing shell just means "not present", the safe direction (we'd
// rather offer to install something already there than hide a real absence).
export async function detectPresent(
  detectCmd: string | null,
  isWin: boolean,
): Promise<boolean> {
  const probe = presenceProbe(detectCmd, isWin);
  if (!probe) return false;
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
