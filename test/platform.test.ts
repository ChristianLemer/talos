// Tests for the Platform substrate — the single source of `os` and how to run a
// command string on this OS. Pure; no IO. Run: deno task test
import { assertEquals } from "@std/assert";
import {
  currentOs,
  hostEnvVar,
  type Os,
  pathSep,
  shellProbe,
  userEnvVar,
} from "../src/platform.ts";

Deno.test("currentOs: returns one of the three known values", () => {
  const os: Os = currentOs();
  assertEquals(["windows", "darwin", "linux"].includes(os), true);
});

Deno.test("shellProbe: POSIX wraps the command in a login /bin/sh -lc", () => {
  const p = shellProbe("darwin", "brew list jq");
  assertEquals(p.cmd, "/bin/sh");
  // -lc (login): a Finder-launched .app gets a minimal launchd PATH; the login
  // shell rebuilds it via /etc/profile → path_helper so brew tools are found.
  assertEquals(p.args, ["-lc", "brew list jq"]);
});

Deno.test("shellProbe: linux is POSIX too", () => {
  const p = shellProbe("linux", "brew list jq");
  assertEquals(p.cmd, "/bin/sh");
});

Deno.test("shellProbe: Windows wraps in powershell with the 127 guard", () => {
  const p = shellProbe("windows", "winget list --id Git.Git");
  assertEquals(p.cmd, "powershell.exe");
  assertEquals(p.args[0], "-NoProfile");
  assertEquals(p.args[1], "-Command");
  assertEquals(
    p.args[2].includes("GetEnvironmentVariable('Path','Machine')"),
    true,
  );
  assertEquals(p.args[2].includes("$ErrorActionPreference='Stop'"), true);
  assertEquals(
    p.args[2].includes(
      "try { winget list --id Git.Git; exit $LASTEXITCODE } catch { exit 127 }",
    ),
    true,
  );
});

Deno.test("pathSep: Windows backslash, POSIX forward slash", () => {
  assertEquals(pathSep("windows"), "\\");
  assertEquals(pathSep("darwin"), "/");
  assertEquals(pathSep("linux"), "/");
});

Deno.test("host/user env var names differ on Windows", () => {
  assertEquals(hostEnvVar("windows"), "COMPUTERNAME");
  assertEquals(hostEnvVar("darwin"), "HOSTNAME");
  assertEquals(userEnvVar("windows"), "USERNAME");
  assertEquals(userEnvVar("linux"), "USER");
});
