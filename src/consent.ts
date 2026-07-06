// consent.ts — the install journal + the "share it" consent, machine-local.
//
// Two facts live in the per-machine LOCAL data dir (%LOCALAPPDATA%\Talos on
// Windows, ~/Library/Application Support/Talos on Mac — NEVER the shared exe
// folder): whether the user consented to SHARE their history, and the history
// itself (one JSON object per line). When (and only when) the user consents,
// each entry is ALSO mirrored to a shared file beside the exe —
// logs/<host>/<user>.jsonl — so a team can see who set up what. No consent → the
// shared copy is never written; the local journal is always kept.
//
// Same discipline as detect.ts / outdated.ts: PARSING is pure (parseHistory /
// parseConsent — unit-tested), EXECUTION is a thin IO shell that never throws (a
// journal that can't write must not sink an install).

// --- the shapes --------------------------------------------------------------

export interface HistEntry {
  at: string; // ISO timestamp
  package: string;
  version: string; // captured for installs/upgrades; "" when unknown
  action: string; // install | uninstall | upgrade
  ok: boolean;
}

export interface Consent {
  decided: boolean; // has the user answered the share question at all?
  share: boolean; // if decided: do they share their history?
}

// --- pure parsing (defensive) ------------------------------------------------

// Parse a JSONL history file → entries. Skips blank/malformed lines and any
// object with no package name, so a single corrupt line never sinks the log.
export function parseHistory(raw: string): HistEntry[] {
  const out: HistEntry[] = [];
  for (const line of raw.split(/\r?\n/)) {
    if (!line.trim()) continue;
    try {
      const o = JSON.parse(line);
      if (!o || typeof o.package !== "string" || !o.package) continue;
      out.push({
        at: typeof o.at === "string" ? o.at : "",
        package: o.package,
        version: typeof o.version === "string" ? o.version : "",
        action: typeof o.action === "string" ? o.action : "",
        ok: o.ok === true,
      });
    } catch {
      // not JSON → skip this line, keep the rest
    }
  }
  return out;
}

// Parse the consent file → {decided, share}. Missing or garbage → UNDECIDED
// (decided:false), which makes the UI show the first-boot dialog.
export function parseConsent(raw: string): Consent {
  try {
    const o = JSON.parse(raw);
    if (o && typeof o.share === "boolean") {
      return { decided: true, share: o.share };
    }
  } catch {
    // fall through
  }
  return { decided: false, share: false };
}

// Where a consented copy lands, beside the exe: logs/<host>/<user>.jsonl. Pure
// path building (isWin picks the separator) so the IO shell stays trivial.
export function sharedLogPath(
  exeDir: string,
  host: string,
  user: string,
  isWin: boolean,
): string {
  const sep = isWin ? "\\" : "/";
  return [exeDir, "logs", host, `${user}.jsonl`].join(sep);
}

// --- IO shell (never throws) -------------------------------------------------

// Everything the IO needs, injected — so the shell is testable against temp dirs
// and the server just passes its real paths/identity.
export interface ConsentStore {
  localDir: string; // per-machine LOCAL data dir (%LOCALAPPDATA%\Talos)
  exeDir: string; // the shared folder the exe sits in (OneDrive)
  host: string; // machine name — namespaces the shared log
  user: string; // user name — the shared log's filename
  isWin: boolean;
}

function sep(s: ConsentStore): string {
  return s.isWin ? "\\" : "/";
}
function consentPath(s: ConsentStore): string {
  return `${s.localDir}${sep(s)}consent.json`;
}
function localHistPath(s: ConsentStore): string {
  return `${s.localDir}${sep(s)}history.jsonl`;
}
function ensureDir(path: string): void {
  const dir = path.replace(/[/\\][^/\\]+$/, "");
  try {
    Deno.mkdirSync(dir, { recursive: true });
  } catch { /* already there */ }
}

// Read the consent state. Missing/unreadable → undecided (first-boot dialog).
export function readConsent(s: ConsentStore): Consent {
  try {
    return parseConsent(Deno.readTextFileSync(consentPath(s)));
  } catch {
    return { decided: false, share: false };
  }
}

// Record the user's share choice (this also marks consent DECIDED).
export function writeConsent(s: ConsentStore, share: boolean): void {
  try {
    ensureDir(consentPath(s));
    Deno.writeTextFileSync(consentPath(s), JSON.stringify({ share }));
  } catch { /* best-effort — a lost consent just re-asks next boot */ }
}

// Read the local install history (newest handling is the UI's job).
export function readHistory(s: ConsentStore): HistEntry[] {
  try {
    return parseHistory(Deno.readTextFileSync(localHistPath(s)));
  } catch {
    return [];
  }
}

// Journal one outcome: ALWAYS to the local file; and — only if the user has
// consented to share — ALSO append to the shared file beside the exe. Both are
// best-effort: a journal that can't write must never sink an install.
export function appendHistory(s: ConsentStore, entry: HistEntry): void {
  const line = JSON.stringify(entry) + "\n";
  try {
    ensureDir(localHistPath(s));
    Deno.writeTextFileSync(localHistPath(s), line, { append: true });
  } catch { /* best-effort */ }
  if (readConsent(s).share) {
    const shared = sharedLogPath(s.exeDir, s.host, s.user, s.isWin);
    try {
      ensureDir(shared);
      Deno.writeTextFileSync(shared, line, { append: true });
    } catch { /* shared folder may be offline — local copy still kept */ }
  }
}

// Clear the LOCAL history only. The shared team log is a record others rely on —
// clearing your own view must not erase what the team already saw.
export function clearHistory(s: ConsentStore): void {
  try {
    Deno.removeSync(localHistPath(s));
  } catch { /* nothing to clear */ }
}
