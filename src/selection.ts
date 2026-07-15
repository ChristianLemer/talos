// selection.ts — the USER'S INTENTION (which packages they toggled in/out by
// hand), persisted machine-locally. Distinct from machine presence, which is
// always re-detected live ("detect, don't remember" governs presence, NOT
// intention: a choice can't be re-observed, so it must be remembered). Mirrors
// consent.ts: PARSING is pure/defensive (a corrupt file never sinks a session),
// the IO shell never throws. Stored beside consent/history in the LOCAL data dir,
// NEVER the shared exe folder.

import { type Os, pathSep } from "./platform.ts";

export interface Selection {
  pkgs: Record<string, "in" | "out">; // stable key "bundle::name" → toggle
}

// Pure, defensive: anything but a well-formed {pkgs:{key:"in"|"out"}} → empty.
export function parseSelection(raw: string): Selection {
  try {
    const o = JSON.parse(raw);
    const src = o && typeof o === "object" ? o.pkgs : null;
    const pkgs: Record<string, "in" | "out"> = {};
    if (src && typeof src === "object") {
      for (const [k, v] of Object.entries(src)) {
        if (typeof k === "string" && (v === "in" || v === "out")) pkgs[k] = v;
      }
    }
    return { pkgs };
  } catch {
    return { pkgs: {} };
  }
}

// The IO needs only a local dir + os (unlike ConsentStore, no host/user: the
// selection is per-machine, never namespaced into a shared file).
export interface SelectionStore {
  localDir: string;
  os: Os;
}

function selectionPath(s: SelectionStore): string {
  return `${s.localDir}${pathSep(s.os)}selection.json`;
}
function ensureDir(path: string): void {
  const dir = path.replace(/[/\\][^/\\]+$/, "");
  try {
    Deno.mkdirSync(dir, { recursive: true });
  } catch { /* already there */ }
}

// Read the persisted intention. Missing/unreadable → empty (author defaults).
export function readSelection(s: SelectionStore): Selection {
  try {
    return parseSelection(Deno.readTextFileSync(selectionPath(s)));
  } catch {
    return { pkgs: {} };
  }
}

// Persist the intention. Best-effort: a failed write must never sink a session.
export function writeSelection(s: SelectionStore, sel: Selection): void {
  try {
    ensureDir(selectionPath(s));
    Deno.writeTextFileSync(selectionPath(s), JSON.stringify(sel));
  } catch { /* best-effort — a lost write just re-asks from author defaults */ }
}
