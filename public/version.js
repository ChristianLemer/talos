// version.js — compare two dotted version strings. The pure core of version
// pinning: the repaint asks "installed vs pin?" and gets below (-1) / equal (0)
// / above (1), which decides the row's action (upgrade / satisfied / downgrade).
// Lives in public/ (not src/) because the SHARED decision rule (decision.js) uses
// it, and that rule runs in BOTH the browser and the server — same discipline as
// decision.js / model.js.
//
// Rules, deliberately simple (NOT full semver — see memory talos-version-pin):
//   - split on ".", compare segment by segment NUMERICALLY (so 2.10 > 2.9, which
//     a string compare gets wrong — the whole reason this function exists).
//   - a segment's value is its LEADING numeric prefix (parseInt): "0-beta" → 0,
//     so a pre-release suffix is ignored rather than mis-ordered.
//   - a missing trailing segment counts as 0: "1.8" == "1.8.0".
//   - empty / non-numeric → 0, the harmless direction.
// Pure, no shell, no dependency — testable like decision/model.
export function compareVersions(a, b) {
  const seg = (v) => String(v).split(".").map((s) => parseInt(s, 10) || 0);
  const as = seg(a);
  const bs = seg(b);
  const n = Math.max(as.length, bs.length);
  for (let i = 0; i < n; i++) {
    const x = as[i] ?? 0;
    const y = bs[i] ?? 0;
    if (x < y) return -1;
    if (x > y) return 1;
  }
  return 0;
}

// Installed vs PIN, at the pin's own precision: the installed version is cut to as many
// segments as the author wrote, so `2.50.1.windows.1` IS the pin "2.50.1" and a pin of "1.8"
// is any 1.8.x. Since 2026-10-10 the Apply batches a downgrade (an uninstall + reinstall), so
// a suffix read as "above the pin" would reinstall the package on every Apply, and never
// converge — the probe reads the same suffix afterwards. Twin of `decision::compare_to_pin`.
export function compareToPin(installed, pin) {
  const n = String(pin).split(".").length;
  return compareVersions(String(installed).split(".").slice(0, n).join("."), pin);
}
