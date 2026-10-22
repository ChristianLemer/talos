// machine.js — the "This machine" card of the Log tab, PURE: no DOM, no server, no globals.
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
  const rows = [];
  const row = (label, value, warn = false) => rows.push({ label, value, warn });
  row("System", [m.os, m.os_version, m.arch].filter(Boolean).join(" · "));
  row("CPU", `${m.cpu || "unknown"} · ${m.cores} logical cores`);
  row("Memory", `${formatMb(m.ram_available_mb)} free of ${formatMb(m.ram_total_mb)}`);
  const disk = (d) => `${formatMb(d.free_mb)} free of ${formatMb(d.total_mb)} on ${d.mount}`;
  if (m.system_disk) row("System disk", disk(m.system_disk));
  if (m.kit) row("Kit", `${m.kit.path} (${m.kit.location})`, m.kit.location !== "local");
  if (m.kit_disk && (!m.system_disk || m.kit_disk.mount !== m.system_disk.mount)) {
    row("Kit disk", disk(m.kit_disk));
  }
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
  return line;
}
