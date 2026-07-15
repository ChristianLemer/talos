// test/selection.test.ts
import { assertEquals } from "@std/assert";
import { parseSelection } from "../src/selection.ts";
import {
  readSelection,
  type SelectionStore,
  writeSelection,
} from "../src/selection.ts";

function tempStore(): SelectionStore & { cleanup: () => void } {
  const dir = Deno.makeTempDirSync();
  return {
    localDir: dir,
    os: "darwin",
    cleanup: () => Deno.removeSync(dir, { recursive: true }),
  };
}

Deno.test("parseSelection: valid JSON → pkgs map", () => {
  const s = parseSelection(
    '{"pkgs":{"Terminal::Starship config":"out","Editors::MarkText":"in"}}',
  );
  assertEquals(s.pkgs["Terminal::Starship config"], "out");
  assertEquals(s.pkgs["Editors::MarkText"], "in");
});

Deno.test("parseSelection: garbage / empty / missing → { pkgs: {} }", () => {
  assertEquals(parseSelection("not json").pkgs, {});
  assertEquals(parseSelection("").pkgs, {});
  assertEquals(parseSelection("{}").pkgs, {});
  assertEquals(parseSelection('{"pkgs":null}').pkgs, {});
});

Deno.test("parseSelection: drops entries whose value isn't in|out", () => {
  const s = parseSelection(
    '{"pkgs":{"A::x":"in","B::y":"maybe","C::z":123,"D::w":"out"}}',
  );
  assertEquals(s.pkgs, { "A::x": "in", "D::w": "out" });
});

Deno.test("readSelection: missing file → { pkgs: {} }", () => {
  const store = tempStore();
  try {
    assertEquals(readSelection(store).pkgs, {});
  } finally {
    store.cleanup();
  }
});

Deno.test("writeSelection → readSelection round-trips", () => {
  const store = tempStore();
  try {
    writeSelection(store, { pkgs: { "Terminal::Starship config": "out" } });
    assertEquals(readSelection(store).pkgs, {
      "Terminal::Starship config": "out",
    });
  } finally {
    store.cleanup();
  }
});

Deno.test("writeSelection: empty pkgs round-trips as empty (Reset case)", () => {
  const store = tempStore();
  try {
    writeSelection(store, { pkgs: { "A::x": "in" } });
    writeSelection(store, { pkgs: {} });
    assertEquals(readSelection(store).pkgs, {});
  } finally {
    store.cleanup();
  }
});

Deno.test("readSelection: garbage bytes on disk → { pkgs: {} } (defensive)", () => {
  const store = tempStore();
  try {
    // Write junk straight to the selection path, then read it back through the
    // full readSelection ∘ parseSelection composition — a corrupt file must
    // never sink a session, it just falls back to author defaults.
    Deno.writeTextFileSync(`${store.localDir}/selection.json`, "}{ not json");
    assertEquals(readSelection(store).pkgs, {});
  } finally {
    store.cleanup();
  }
});
