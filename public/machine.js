// machine.js — the "This machine" card of the Machine tab, PURE: no DOM, no server, no globals.
//
// It renders the `machine` and `scan` records EXACTLY as the journal received them
// (`machine.rs` on the engine side). The card is the answer to "what does sharing send?":
// it shows the values themselves, never a description of them, so the two cannot drift
// apart. Only bookkeeping stays in the file alone: the timestamp, the build stamp, the
// collection's own durations and the per-probe list (summarised as the slowest probe).
// Each value is plain text — app.js puts it in with textContent, because a path or an
// antivirus name is machine data, not markup.

// A shell start this slow is worth a look: an antivirus or EDR scanning every spawn is the
// usual cause on a corporate Windows machine, and Talos spawns hundreds of short processes.
export const SLOW_SHELL_MS = 1000;

export function formatMs(ms) {
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

export function formatMb(mb) {
  if (mb < 1024) return `${mb} MB`;
  return `${(mb / 1024).toFixed(1)} GB`;
}

// Where the shared copy goes, or that there is none.
export function shareLine(consent, sharedTo) {
  return consent && consent.share
    ? `Shared with your team, in ${sharedTo}`
    : "Kept on this machine only — sharing is off";
}

// The card's rows, in reading order: { label, value, warn }. Empty while the profile is
// still being collected (it is observed after the first scan).
export function machineRows(m) {
  if (!m) return [];
  // A collection that failed or hung says so, rather than leaving "collecting" up forever.
  if (m.error) return [{ label: "Profile", value: m.error, warn: true }];
  const rows = [];
  const row = (label, value, warn = false) => rows.push({ label, value, warn });
  row("System", [m.os, m.os_version, m.arch].filter(Boolean).join(" · "));
  row("CPU", `${m.cpu || "unknown"} · ${m.cores} logical cores`);
  row("Memory", `${formatMb(m.ram_available_mb)} free of ${formatMb(m.ram_total_mb)}`);
  const disk = (d) => `${formatMb(d.free_mb)} free of ${formatMb(d.total_mb)} on ${d.mount}`;
  if (m.system_disk) row("System disk", disk(m.system_disk));
  if (m.kit) row("Kit", `${m.kit.path} (${m.kit.location})`, m.kit.location !== "local");
  // Which content this launch read: the kit's archive, loose folders, or an earlier copy
  // because the archive was unusable or gone (content.rs) — the one to flag.
  if (m.kit?.content) row("Content", m.kit.content, !!m.kit.content_stale);
  row("One shell", formatMs(m.shell_ms), m.shell_ms >= SLOW_SHELL_MS);
  for (const t of m.tools || []) row(t.name, t.path || "not found", !t.path);
  if (m.winget) {
    row(
      "winget alias",
      [
        m.winget.alias ? "registered for this user" : "NOT registered for this user (App Installer)",
        m.winget.on_path ? "on PATH" : "WindowsApps NOT on PATH",
      ].join(" · "),
      !m.winget.alias || !m.winget.on_path,
    );
  }
  if ((m.antivirus || []).length || m.realtime != null) {
    const parts = [...(m.antivirus || [])];
    if (m.realtime != null) parts.push(`Defender real-time ${m.realtime ? "on" : "off"}`);
    row("Antivirus", parts.join(" · "));
  }
  if (m.power) row("Power", m.power === "ac" ? "on AC" : "on battery", m.power !== "ac");
  // The REGISTRY TYPE of the user PATH, under its registry name. Stored as REG_SZ, a
  // `%USERPROFILE%` in it is never expanded, so WindowsApps — and winget with it — is lost
  // to every program. Without a `%` in it, REG_SZ loses nothing: a fact, not a warning.
  if (m.user_path_kind) {
    const kind = { String: "REG_SZ", ExpandString: "REG_EXPAND_SZ" }[m.user_path_kind]
      || m.user_path_kind;
    const lost = m.user_path_kind === "String" && m.user_path_vars === true;
    row("User PATH", lost ? `${kind} — its %variables% are never expanded` : kind, lost);
  }
  // Policies as stored, never judged: the engine cannot know what a value blocks (measured,
  // RequirePrivateStoreOnly blocks nothing Talos does, EnableDefaultSource=0 blocks every
  // winget row). Saying which is the content's job. Absent = not read: say nothing.
  if (m.policies) {
    if (!m.policies.length) row("Policies", "none set");
    for (const p of m.policies) row("Policy", `${p.name} = ${p.value}`);
  }
  return rows;
}

// One line for the last scan: its total, its slowest probe, and any batched call that gave
// no answer — the line that says winget was not found, among others.
export function scanSummary(s) {
  if (!s) return "";
  let line = `Last scan: ${formatMs(s.total_ms)} for ${s.packages} packages`;
  const slowest = (s.probes || []).reduce((a, b) => (b.ms > (a ? a.ms : -1) ? b : a), null);
  if (slowest) line += ` · slowest: ${slowest.name} (${formatMs(slowest.ms)})`;
  for (const u of s.unavailable || []) line += ` · ${u}`;
  if (s.profile_running) line += " · measured while the machine profile was being collected";
  return line;
}

// The card as plain text, for a chat: what a colleague is asked for when something fails on
// a machine they cannot see. The header says which build wrote it and when, because the
// paste is read away from the machine; a flagged row says it with "⚠", because the colour
// does not survive a paste. Empty while there is no profile to copy.
export function machineText(m, scan) {
  if (!m) return "";
  const lines = [["Talos machine profile", m.at, m.talos].filter(Boolean).join(" · ")];
  for (const r of machineRows(m)) lines.push(`${r.warn ? "⚠ " : ""}${r.label}: ${r.value}`);
  const s = scanSummary(scan);
  if (s) lines.push(s);
  return lines.join("\n");
}

// What a version change was found at and run toward, as the journal records it ("1.141.2 →
// 1.142.0") — "" when it names no target. What tells "failed" from "succeeded and moved
// nothing", which an exit 0 alone cannot.
export function changeText(entry) {
  if (!entry?.target) return "";
  return `${entry.found || "?"} → ${entry.target}`;
}

// What a failed History line shows on hover and copies: where the change went from and to,
// the exit code, then the last lines the command printed (`tail.rs` made them readable). Empty
// for a success, and for a line journalled before the engine kept an exit. Plain text — the
// output is the tool's, not markup.
export function failureText(entry) {
  if (!entry || entry.ok || entry.exit === undefined || entry.exit === null) return "";
  const lines = [changeText(entry), `exit ${entry.exit}`, entry.tail || ""];
  return lines.filter(Boolean).join("\n");
}
