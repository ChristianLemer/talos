// src/managers.ts
// SystemManager — the strategy for FAMILY 1 routes (system package managers).
// winget and brew are ONE route with two platform incarnations: same mechanic
// (install/uninstall/upgrade, exit-code presence, a single machine-wide outdated
// scan), differing only in binary+subcommand spelling and the OS where each
// reigns. Adding apt later = one more entry in MANAGERS. Commands are STRINGS;
// Platform.shellProbe runs them (families 1 & 3 share that substrate).
import type { Os } from "./platform.ts";

export interface Outdated {
  current: string;
  available: string;
}

export interface SystemManager {
  route: string; // "winget" | "brew"
  os: Os[]; // where this manager reigns
  idField: "winget" | "brew"; // which RawPkg field carries its id
  install(id: string): string;
  // Install at an EXACT version — the pin. winget takes `--version`; brew has no
  // such flag, so it installs the versioned formula `id@ver` (which exists ONLY
  // when the tap provides it — the documented brew wall). Used for both pinned
  // install and pinned upgrade (an exact pin never overshoots to latest).
  installPinned(id: string, version: string): string;
  uninstall(id: string): string;
  upgrade(id: string): string;
  presenceCommand(id: string): string; // exit 0 iff installed; stdout carries version
  parseVersion(id: string, output: string): string;
  outdatedScanCommand(): string;
  parseOutdated(output: string): Map<string, Outdated>;
}

export const WINGET: SystemManager = {
  route: "winget",
  os: ["windows"],
  idField: "winget",
  install: (id) =>
    `winget install --id ${id} -e --source winget --accept-source-agreements --accept-package-agreements`,
  installPinned: (id, version) =>
    `winget install --id ${id} -e --version ${version} --source winget --accept-source-agreements --accept-package-agreements`,
  uninstall: (id) => `winget uninstall --id ${id} -e --source winget`,
  upgrade: (id) =>
    `winget upgrade --id ${id} -e --source winget --accept-source-agreements --accept-package-agreements`,
  presenceCommand: (id) =>
    `winget list --id ${id} --exact --source winget --accept-source-agreements`,
  parseVersion: (id, output) => {
    const clean = output
      // deno-lint-ignore no-control-regex -- strip ANSI so tokens split cleanly
      .replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
    const lc = id.toLowerCase();
    for (const line of clean.split(/\r?\n/)) {
      const cols = line.trim().split(/\s{1,}/);
      const at = cols.findIndex((c) => c.toLowerCase() === lc);
      // Same guard as BREW: the token after the id must look like a version
      // (starts with a digit), so a "<id> version X" binary line doesn't grab the
      // word "version". versionFrom then falls back to the generic matcher.
      if (at >= 0 && /^\d/.test(cols[at + 1] ?? "")) return cols[at + 1];
    }
    return "";
  },
  outdatedScanCommand: () =>
    `winget upgrade --accept-source-agreements --source winget`,
  parseOutdated: (output) => parseWingetUpgrade(output),
};

export const BREW: SystemManager = {
  route: "brew",
  os: ["darwin", "linux"],
  idField: "brew",
  // --yes: brew 6.x asks "[y/n]" before upgrading dependencies (e.g. node pulls
  // c-ares). Talos's xterm is display-only — no stdin reaches the pty — so a
  // prompt DEADLOCKS the step forever. Clicking Apply is already the consent, so
  // we auto-answer on every mutating command. uninstall doesn't prompt. The
  // winget counterpart is --accept-*-agreements.
  install: (id) => `brew install --yes ${id}`,
  // brew has no --version flag: an exact version is a SEPARATE versioned formula
  // `id@ver` (e.g. jq@1.8), which resolves only if the tap ships it. The brew wall
  // (see memory talos-version-pin): a pin whose formula doesn't exist will fail at
  // install time — repaint-at-apply then shows the real (unchanged) state.
  installPinned: (id, version) => `brew install --yes ${id}@${version}`,
  uninstall: (id) => `brew uninstall ${id}`,
  upgrade: (id) => `brew upgrade --yes ${id}`,
  // `brew list --versions X` gives the version for a FORMULA but is EMPTY for a
  // cask; the `|| ... --cask` fallback covers casks. Uniform: exit 0 + "<id> <ver>"
  // when present (either kind), exit 1 + "" when absent. brew resolves cask-vs-
  // formula itself, so Talos never stores that distinction.
  presenceCommand: (id) =>
    `brew list --versions ${id} || brew list --cask --versions ${id}`,
  parseVersion: (id, output) => {
    for (const line of output.split(/\r?\n/)) {
      const cols = line.trim().split(/\s+/);
      // Require the token AFTER the id to look like a version (starts with a
      // digit). `brew list` prints "git 2.50.1", but the BINARY probe for a
      // brew-declared package can print "git version 2.50.1 …" (Apple git) — there
      // the next token is the word "version". Rejecting it lets versionFrom fall
      // back to the generic matcher that finds 2.50.1.
      if (
        cols[0]?.toLowerCase() === id.toLowerCase() && /^\d/.test(cols[1] ?? "")
      ) {
        return cols[1];
      }
    }
    return "";
  },
  outdatedScanCommand: () => `brew outdated --json=v2`,
  parseOutdated: (output) => {
    const map = new Map<string, Outdated>();
    try {
      const j = JSON.parse(output) as {
        formulae?: Array<
          {
            name: string;
            installed_versions?: string[];
            current_version?: string;
          }
        >;
        casks?: Array<
          {
            name: string;
            installed_versions?: string[];
            current_version?: string;
          }
        >;
      };
      for (const item of [...(j.formulae ?? []), ...(j.casks ?? [])]) {
        const current = item.installed_versions?.[0] ?? "";
        const available = item.current_version ?? "";
        if (item.name && available) {
          map.set(item.name.toLowerCase(), { current, available });
        }
      }
    } catch {
      // swallow — empty map is the safe "nothing outdated" direction
    }
    return map;
  },
};

export const MANAGERS: SystemManager[] = [WINGET, BREW];

// The native system manager for this OS — the data-driven selector. null when no
// system manager reigns here.
export function nativeManager(os: Os): SystemManager | null {
  return MANAGERS.find((m) => m.os.includes(os)) ?? null;
}

// Parse `winget upgrade` output → Map<lowercased-id, {current, available}>. The
// table is FIXED-WIDTH: slice by the HEADER's column offsets, never by splitting
// on spaces (names/versions contain spaces; offsets don't lie). Defensive: any
// hiccup → empty map ("nothing outdated" is the safe direction).
export function parseWingetUpgrade(raw: string): Map<string, Outdated> {
  const map = new Map<string, Outdated>();
  try {
    const lines = raw
      // deno-lint-ignore no-control-regex -- \x1b (ESC) is exactly what we strip
      .replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "")
      .replace(/[─-╿█]/g, "")
      .split(/\r?\n/)
      .map((l) => l.replace(/\r/g, ""));
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
      if (!line.trim()) break;
      if (/^[-\s]+$/.test(line)) continue;
      if (line.length < avPos) continue;
      const id = line.slice(idPos, verPos).trim();
      const current = line.slice(verPos, avPos).trim();
      const available = line.slice(avPos, srcPos > avPos ? srcPos : undefined)
        .trim();
      if (!id || !available) continue;
      map.set(id.toLowerCase(), { current, available });
    }
  } catch {
    // swallow — empty map is the safe "nothing outdated" direction
  }
  return map;
}
