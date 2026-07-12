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

export interface Probe {
  cmd: string;
  args: string[];
}

// PATH refresh for Windows: an install writes the registry but does NOT propagate
// PATH to already-running processes (the panel inherited a stale PATH), so a
// freshly-installed tool would read as absent without this.
const WIN_PATH_REFRESH =
  "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User');";

// Wrap a command STRING into a Probe that runs it in the native shell. The single
// home of the shell-wrapping that used to be triplicated across detect.ts. The
// Windows 127 guard is CRITICAL: a missing command raises CommandNotFoundException
// which does NOT set $LASTEXITCODE — "cmd; exit $LASTEXITCODE" would exit 0 (the
// prior value) and read an absent tool as PRESENT (the rg/fd/bat false-positive).
// try/catch with Stop → exit 127 on any failure/not-found.
export function shellProbe(os: Os, command: string): Probe {
  if (os === "windows") {
    const ps =
      `${WIN_PATH_REFRESH} $ErrorActionPreference='Stop'; try { ${command}; exit $LASTEXITCODE } catch { exit 127 }`;
    return { cmd: "powershell.exe", args: ["-NoProfile", "-Command", ps] };
  }
  return { cmd: "/bin/sh", args: ["-c", command] };
}
