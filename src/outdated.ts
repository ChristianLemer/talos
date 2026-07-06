// outdated.ts — what, on THIS machine, has a newer version available?
//
// "Detect, don't remember" again: instead of a per-package `winget show` (slow,
// N calls, 403-prone), we ask winget ONCE for its full `winget upgrade` list —
// winget already knows the latest version of everything. One scan for the whole
// machine, current→available in hand.
//
// The table is FIXED-WIDTH. We slice each row by the HEADER's column offsets
// (Id / Version / Available / Source), NOT by splitting on spaces — package
// names and versions contain spaces, so a space-split corrupts the columns;
// offsets don't lie. Mirrors detect.ts: PARSING is pure (parseWingetUpgrade →
// unit-testable), EXECUTION is the thin IO shell (scanOutdated).
//
// Defensive throughout: any parse hiccup or scan failure yields an EMPTY map,
// never a throw — "nothing looks outdated" is the safe direction (Apply simply
// won't upgrade), and the scan must never block or break the UI.

export interface Outdated {
  current: string;
  available: string;
}

// PATH refresh for Windows — same reason as detect.ts: a fresh install writes
// the registry but doesn't propagate PATH to already-running processes, and the
// panel inherited a stale PATH. Prepend the live Machine+User PATH so winget
// resolves.
const WIN_PATH_REFRESH =
  "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

export interface Probe {
  cmd: string;
  args: string[];
}

// The ONE scan command: refresh PATH, then `winget upgrade` for the whole
// machine. Not in a PTY (no TTY → less spinner noise). Pure (→ testable);
// scanOutdated runs it.
export function upgradeScanProbe(): Probe {
  const ps =
    `${WIN_PATH_REFRESH} winget upgrade --accept-source-agreements --source winget`;
  return {
    cmd: "powershell.exe",
    args: ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps],
  };
}

// Run the single machine-wide scan → Map<lowercased-id, {current, available}>.
// Windows-only (winget is). Never rejects: off Windows or on any spawn failure
// (err with no stdout), resolve an empty map — the safe "nothing outdated"
// direction. Best-effort by design: the caller runs it in parallel with
// presence detection and never blocks the UI on it.
export async function scanOutdated(
  isWin: boolean,
): Promise<Map<string, Outdated>> {
  if (!isWin) return new Map(); // winget is Windows-only
  const probe = upgradeScanProbe();
  try {
    const { stdout } = await new Deno.Command(probe.cmd, {
      args: probe.args,
      stdout: "piped",
      stderr: "null",
      stdin: "null",
    }).output();
    return parseWingetUpgrade(new TextDecoder().decode(stdout));
  } catch {
    return new Map();
  }
}

// Bridge a package to the scan: its wingetId (lowercased, as the map is keyed)
// → its outdated entry, or null if it has no id or isn't in the scan (up to
// date). The single place server.ts asks "is THIS package stale?".
export function outdatedFor(
  wingetId: string | null,
  scan: Map<string, Outdated>,
): Outdated | null {
  if (!wingetId) return null;
  return scan.get(wingetId.toLowerCase()) ?? null;
}

// Parse `winget upgrade` output → Map<lowercased-id, {current, available}>.
export function parseWingetUpgrade(raw: string): Map<string, Outdated> {
  const map = new Map<string, Outdated>();
  try {
    const lines = raw
      // deno-lint-ignore no-control-regex -- \x1b (ESC) is exactly what we strip
      .replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "") // strip ANSI escapes
      .replace(/[─-╿█]/g, "") // strip box-drawing / progress glyphs
      .split(/\r?\n/)
      .map((l) => l.replace(/\r/g, "")); // drop stray carriage returns (spinner)

    // The header names the columns. winget localises these, but the English
    // "Name  Id  Version  Available  Source" is what ships on the managed
    // machines we target. Match on Id + Available — the two we slice by.
    const h = lines.findIndex((l) =>
      /\bId\b/.test(l) && /\bAvailable\b/.test(l)
    );
    if (h < 0) return map;

    const header = lines[h];
    const idPos = header.indexOf("Id");
    const verPos = header.indexOf("Version");
    const avPos = header.indexOf("Available");
    const srcPos = header.indexOf("Source");
    if (idPos < 0 || verPos < 0 || avPos < 0) return map;

    for (const line of lines.slice(h + 1)) {
      if (!line.trim()) break; // blank line = end of table
      if (/^[-\s]+$/.test(line)) continue; // the --- separator row
      if (line.length < avPos) continue; // a summary line ("12 upgrades…") has no columns
      const id = line.slice(idPos, verPos).trim();
      const current = line.slice(verPos, avPos).trim();
      const available = line
        .slice(avPos, srcPos > avPos ? srcPos : undefined)
        .trim();
      if (!id || !available) continue;
      map.set(id.toLowerCase(), { current, available });
    }
  } catch {
    // swallow — empty map is the safe "nothing outdated" direction
  }
  return map;
}
