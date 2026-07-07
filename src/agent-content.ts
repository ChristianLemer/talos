// agent-content.ts — what Claude Code plugins / cross-agent skills are present.
//
// Pure parsing of two tools' list output, defensive like parseWingetUpgrade: any
// malformed input yields "nothing installed" (the safe direction — never claim
// present when unsure). The IO (spawning the tools) lives in detect.ts.
//
//   claude plugin list --json → a JSON array of {id, enabled, ...}
//   npx skills list -g        → ANSI-coloured text, skill name in the 1st column

// Installed plugin ids, e.g. ["chiron@tekton", ...]. Malformed → [].
export function parsePluginList(json: string): string[] {
  try {
    const arr = JSON.parse(json);
    if (!Array.isArray(arr)) return [];
    return arr
      .map((p) => (p && typeof p.id === "string" ? p.id : null))
      .filter((id): id is string => id !== null);
  } catch {
    return [];
  }
}

// Is a plugin present? `detect` may be the full "plugin@marketplace" id or just
// the "plugin" part before @ (what a bundle usually declares).
export function pluginPresent(detect: string, json: string): boolean {
  const ids = parsePluginList(json);
  if (ids.includes(detect)) return true;
  return ids.some((id) => id.split("@")[0] === detect);
}

// Installed skill names from `npx skills list -g`. Strips ANSI, ignores the
// header and any npm warn/exec noise, takes the first whitespace-delimited token
// of each remaining line.
export function parseSkillList(raw: string): string[] {
  const out: string[] = [];
  for (const line of raw.split(/\r?\n/)) {
    // deno-lint-ignore no-control-regex -- \x1b (ESC) is exactly what we strip
    const clean = line.replace(/\x1b\[[0-9;]*m/g, "").trim();
    if (!clean) continue;
    if (/^npm\b/.test(clean)) continue; // npm warn/exec noise
    if (/^Global Skills/i.test(clean)) continue; // the header
    const name = clean.split(/\s+/)[0];
    if (name) out.push(name);
  }
  return out;
}

export function skillPresent(name: string, raw: string): boolean {
  return parseSkillList(raw).includes(name);
}
