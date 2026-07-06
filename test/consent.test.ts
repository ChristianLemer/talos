// Tests for the PURE core of consent + install history (src/consent.ts). Parsing
// is pure/defensive (a corrupt line never sinks the log); the IO shell is exercised
// live below against a temp dir. Mirrors detect.ts / outdated.ts.
import { assertEquals } from "@std/assert";
import {
  appendHistory,
  clearHistory,
  type ConsentStore,
  parseConsent,
  parseHistory,
  readConsent,
  readHistory,
  sharedLogPath,
  writeConsent,
} from "../src/consent.ts";

// --- parseHistory: JSONL → entries, skipping anything malformed --------------

Deno.test("parseHistory: one entry per valid JSONL line", () => {
  const raw = [
    '{"at":"2026-07-06T10:00:00Z","package":"Git","version":"2.55","action":"install","ok":true}',
    '{"at":"2026-07-06T10:01:00Z","package":"jq","version":"","action":"uninstall","ok":true}',
  ].join("\n");
  const h = parseHistory(raw);
  assertEquals(h.length, 2);
  assertEquals(h[0].package, "Git");
  assertEquals(h[1].action, "uninstall");
});

Deno.test("parseHistory: skips blank and malformed lines (never throws)", () => {
  const raw = [
    '{"at":"t","package":"Git","action":"install","ok":true}',
    "", // blank
    "not json at all", // garbage
    "{}", // no package → dropped
    '{"at":"t2","package":"uv","action":"install","ok":false}',
  ].join("\n");
  const h = parseHistory(raw);
  assertEquals(h.map((e) => e.package), ["Git", "uv"]);
});

Deno.test("parseHistory: empty input → empty list", () => {
  assertEquals(parseHistory(""), []);
});

Deno.test("parseHistory: ok coerced to a real boolean", () => {
  const h = parseHistory(
    '{"at":"t","package":"X","action":"install","ok":"yes"}',
  );
  assertEquals(h[0].ok, false); // non-true → false, never a truthy string
});

// --- parseConsent: the decided/share state, defensive ------------------------

Deno.test("parseConsent: valid → decided true + share flag", () => {
  assertEquals(parseConsent('{"share":true}'), { decided: true, share: true });
  assertEquals(parseConsent('{"share":false}'), {
    decided: true,
    share: false,
  });
});

Deno.test("parseConsent: absent/garbage → undecided (first boot shows the dialog)", () => {
  assertEquals(parseConsent(""), { decided: false, share: false });
  assertEquals(parseConsent("not json"), { decided: false, share: false });
});

// --- sharedLogPath: where a consented copy lands, beside the exe -------------

Deno.test("sharedLogPath: logs/<host>/<user>.jsonl beside the exe (POSIX)", () => {
  assertEquals(
    sharedLogPath("/share/Talos", "PC01", "alice", false),
    "/share/Talos/logs/PC01/alice.jsonl",
  );
});

Deno.test("sharedLogPath: Windows separators", () => {
  assertEquals(
    sharedLogPath("S:\\Talos", "PC01", "alice", true),
    "S:\\Talos\\logs\\PC01\\alice.jsonl",
  );
});

// --- IO shell: round-trip against real temp dirs -----------------------------

// A store rooted at fresh temp dirs; POSIX separators (we run tests on Mac).
function tempStore(): ConsentStore & { cleanup: () => void } {
  const localDir = Deno.makeTempDirSync();
  const exeDir = Deno.makeTempDirSync();
  return {
    localDir,
    exeDir,
    host: "PC01",
    user: "alice",
    isWin: false,
    cleanup: () => {
      Deno.removeSync(localDir, { recursive: true });
      Deno.removeSync(exeDir, { recursive: true });
    },
  };
}

Deno.test("readConsent: no file yet → undecided", () => {
  const s = tempStore();
  try {
    assertEquals(readConsent(s), { decided: false, share: false });
  } finally {
    s.cleanup();
  }
});

Deno.test("writeConsent → readConsent round-trips the share flag", () => {
  const s = tempStore();
  try {
    writeConsent(s, true);
    assertEquals(readConsent(s), { decided: true, share: true });
    writeConsent(s, false);
    assertEquals(readConsent(s), { decided: true, share: false });
  } finally {
    s.cleanup();
  }
});

Deno.test("appendHistory: NOT consented → local only, no shared copy", () => {
  const s = tempStore();
  try {
    writeConsent(s, false);
    appendHistory(s, {
      at: "t",
      package: "Git",
      version: "2.55",
      action: "install",
      ok: true,
    });
    assertEquals(readHistory(s).map((e) => e.package), ["Git"]);
    // shared file must NOT exist
    let sharedExists = true;
    try {
      Deno.statSync(sharedLogPath(s.exeDir, s.host, s.user, s.isWin));
    } catch {
      sharedExists = false;
    }
    assertEquals(sharedExists, false);
  } finally {
    s.cleanup();
  }
});

Deno.test("appendHistory: consented → local AND shared copy written", () => {
  const s = tempStore();
  try {
    writeConsent(s, true);
    appendHistory(s, {
      at: "t",
      package: "Git",
      version: "2.55",
      action: "install",
      ok: true,
    });
    assertEquals(readHistory(s).map((e) => e.package), ["Git"]);
    const shared = Deno.readTextFileSync(
      sharedLogPath(s.exeDir, s.host, s.user, s.isWin),
    );
    assertEquals(parseHistory(shared).map((e) => e.package), ["Git"]);
  } finally {
    s.cleanup();
  }
});

Deno.test("appendHistory: accumulates in order; clearHistory empties local", () => {
  const s = tempStore();
  try {
    writeConsent(s, false);
    for (const p of ["Git", "uv", "jq"]) {
      appendHistory(s, {
        at: "t",
        package: p,
        version: "",
        action: "install",
        ok: true,
      });
    }
    assertEquals(readHistory(s).map((e) => e.package), ["Git", "uv", "jq"]);
    clearHistory(s);
    assertEquals(readHistory(s), []);
  } finally {
    s.cleanup();
  }
});

Deno.test("readHistory: no file yet → empty (never throws)", () => {
  const s = tempStore();
  try {
    assertEquals(readHistory(s), []);
  } finally {
    s.cleanup();
  }
});
