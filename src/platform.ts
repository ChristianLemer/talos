// src/platform.ts
// Platform — the single source of truth for the running OS and how to act on it.
// Replaces the `isWin` boolean, which conflated a 3-value axis (windows/darwin/
// linux) into 2. Everything platform-specific either derives from `os` here, or
// tests `os === "windows"` LOCALLY at the site of a true Windows exclusivity.
export type Os = "windows" | "darwin" | "linux";

// Deno.build.os is a wide union; map to our three. Anything not windows/darwin
// (linux and the BSDs Deno may report) is treated as linux — the shell family we
// support there. Never throws.
export function currentOs(): Os {
  const o = Deno.build.os;
  if (o === "windows") return "windows";
  if (o === "darwin") return "darwin";
  return "linux";
}
