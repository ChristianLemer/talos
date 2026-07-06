import { assertEquals } from "@std/assert";
import { makeWatcher, parseScan } from "../src/watch-window.ts";

// --- parseScan: defensive parse of the PS script's JSON stdout ---------------

Deno.test("parseScan: valid JSON → object", () => {
  assertEquals(
    parseScan('{"found":true,"title":"Setup","pushed":true}'),
    { found: true, title: "Setup", pushed: true },
  );
});

Deno.test("parseScan: invalid JSON → not found (safe direction)", () => {
  assertEquals(parseScan("not json at all"), { found: false });
});

Deno.test("parseScan: missing/loose fields → coerced, never claims found", () => {
  assertEquals(parseScan("{}"), { found: false });
  assertEquals(parseScan('{"found":"yes"}'), { found: false }); // non-bool → false
});

// --- makeWatcher: the per-tick decision, spawner + emit + clock injected -----

// A fake spawner returning a scripted queue of raw stdout strings.
function fakeSpawner(queue: string[]) {
  const calls: number[] = [];
  const fn = (pid: number) => {
    calls.push(pid);
    return Promise.resolve(queue.shift() ?? "{}");
  };
  return { fn, calls };
}

function harness(
  opts: { isWin?: boolean; silenceMs?: number; queue?: string[] } = {},
) {
  const emitted: unknown[] = [];
  const sp = fakeSpawner(opts.queue ?? []);
  let clock = 1000;
  const w = makeWatcher({
    i: 2,
    isWin: opts.isWin ?? true,
    silenceMs: opts.silenceMs ?? 10_000,
    spawner: sp.fn,
    emit: (m: unknown) => emitted.push(m),
    now: () => clock,
  });
  return { w, emitted, calls: sp.calls, setClock: (t: number) => (clock = t) };
}

Deno.test("makeWatcher: off Windows is a no-op (never spawns, never emits)", async () => {
  const h = harness({ isWin: false, queue: ['{"found":true,"title":"X"}'] });
  await h.w.runTick(123, 1000);
  assertEquals(h.calls.length, 0);
  assertEquals(h.emitted.length, 0);
});

Deno.test("makeWatcher: foreign window found → one wait-window carrying pushed", async () => {
  const h = harness({
    queue: ['{"found":true,"title":"Setup","pushed":true}'],
  });
  await h.w.runTick(123, 1000);
  assertEquals(h.emitted, [
    { type: "wait-window", i: 2, title: "Setup", pushed: true },
  ]);
});

Deno.test("makeWatcher: raise failed → pushed:false carried (banner stays honest)", async () => {
  const h = harness({
    queue: ['{"found":true,"title":"Git Setup","pushed":false}'],
  });
  await h.w.runTick(123, 1000);
  assertEquals(h.emitted, [
    { type: "wait-window", i: 2, title: "Git Setup", pushed: false },
  ]);
});

Deno.test("makeWatcher: same window across ticks → deduped (signalled once)", async () => {
  const h = harness({
    queue: [
      '{"found":true,"title":"Setup"}',
      '{"found":true,"title":"Setup"}',
    ],
  });
  await h.w.runTick(123, 1000);
  await h.w.runTick(123, 1000);
  assertEquals(h.emitted, [
    { type: "wait-window", i: 2, title: "Setup", pushed: false },
  ]);
});

Deno.test("makeWatcher: a different window → re-signalled", async () => {
  const h = harness({
    queue: [
      '{"found":true,"title":"Setup"}',
      '{"found":true,"title":"License"}',
    ],
  });
  await h.w.runTick(123, 1000);
  await h.w.runTick(123, 1000);
  assertEquals(h.emitted, [
    { type: "wait-window", i: 2, title: "Setup", pushed: false },
    { type: "wait-window", i: 2, title: "License", pushed: false },
  ]);
});

Deno.test("makeWatcher: nothing found + pty silent past threshold → wait-silent", async () => {
  const h = harness({ silenceMs: 10_000, queue: ['{"found":false}'] });
  // now=1000 (from harness), lastActivity=-20000 → silent for 21s > 10s
  await h.w.runTick(123, -20_000);
  assertEquals(h.emitted, [{ type: "wait-silent", i: 2 }]);
});

Deno.test("makeWatcher: nothing found but pty still active → nothing", async () => {
  const h = harness({ silenceMs: 10_000, queue: ['{"found":false}'] });
  await h.w.runTick(123, 999); // 1ms ago → active
  assertEquals(h.emitted, []);
});

Deno.test("makeWatcher: wait-silent fires once per silence episode", async () => {
  const h = harness({
    silenceMs: 10_000,
    queue: ['{"found":false}', '{"found":false}'],
  });
  await h.w.runTick(123, -20_000);
  await h.w.runTick(123, -20_000);
  assertEquals(h.emitted, [{ type: "wait-silent", i: 2 }]); // not twice
});

Deno.test("makeWatcher: stop() emits wait-clear; a late tick is dropped", async () => {
  const h = harness({ queue: ['{"found":true,"title":"Setup"}'] });
  h.w.stop();
  assertEquals(h.emitted, [{ type: "wait-clear", i: 2 }]);
  await h.w.runTick(123, 1000); // after stop → ignored
  assertEquals(h.calls.length, 0);
  assertEquals(h.emitted, [{ type: "wait-clear", i: 2 }]);
});

Deno.test("makeWatcher: anti-overlap — a tick while one is in flight is skipped", async () => {
  // A spawner that never resolves until we let it, to hold a tick in-flight.
  let release: (v: string) => void = () => {};
  const gate = new Promise<string>((r) => (release = r));
  const emitted: unknown[] = [];
  let calls = 0;
  const w = makeWatcher({
    i: 2,
    isWin: true,
    silenceMs: 10_000,
    spawner: () => {
      calls++;
      return gate;
    },
    emit: (m: unknown) => emitted.push(m),
    now: () => 1000,
  });
  const first = w.runTick(123, 1000); // enters, awaits gate
  await w.runTick(123, 1000); // should skip (in-flight)
  assertEquals(calls, 1); // only the first spawned
  release('{"found":false}');
  await first;
});
