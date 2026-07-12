// outdated.ts — what, on THIS machine, has a newer version available?
// Delegates to the native SystemManager (winget on Windows, brew on Mac): one
// machine-wide scan, parsed by that manager. "Detect, don't remember" — the tool
// already knows the latest of everything. Never throws: no manager / any failure
// → empty map (the safe "nothing outdated" direction).
import { type Os, shellProbe } from "./platform.ts";
import {
  nativeManager,
  type Outdated,
  parseWingetUpgrade,
} from "./managers.ts";

export type { Outdated };
export { parseWingetUpgrade }; // re-export: existing tests import it from here

export async function scanOutdated(os: Os): Promise<Map<string, Outdated>> {
  const mgr = nativeManager(os);
  if (!mgr) return new Map();
  try {
    const probe = shellProbe(os, mgr.outdatedScanCommand());
    const { stdout } = await new Deno.Command(probe.cmd, {
      args: probe.args,
      stdout: "piped",
      stderr: "null",
      stdin: "null",
    }).output();
    return mgr.parseOutdated(new TextDecoder().decode(stdout));
  } catch {
    return new Map();
  }
}

// Bridge a package to the scan: its systemId (lowercased, as the map is keyed)
// → its outdated entry, or null if it has no id or isn't in the scan (up to date).
export function outdatedFor(
  systemId: string | null,
  scan: Map<string, Outdated>,
): Outdated | null {
  if (!systemId) return null;
  return scan.get(systemId.toLowerCase()) ?? null;
}
