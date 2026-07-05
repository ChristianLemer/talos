// Tests for the bundle loader — the NAMED ROUTE TABLE and the empty-scan case.
// Pure, no pty/network. Run: deno task test  (from talos/)
import { assertEquals } from "@std/assert";
import { commandsFor, loadBundles } from "../src/bundles.ts";

Deno.test("commandsFor: winget route", () => {
  const c = commandsFor({ name: "Git", winget: "Git.Git" });
  assertEquals(c.route, "winget");
  assertEquals(
    c.install,
    "winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements",
  );
  assertEquals(c.uninstall, "winget uninstall --id Git.Git -e --source winget");
});

Deno.test("commandsFor: brew route (the Mac counterpart)", () => {
  const c = commandsFor({ name: "ripgrep", brew: "ripgrep" });
  assertEquals(c.route, "brew");
  assertEquals(c.install, "brew install ripgrep");
  assertEquals(c.upgrade, "brew upgrade ripgrep");
});

Deno.test("commandsFor: cargo reinstalls as its upgrade", () => {
  const c = commandsFor({ name: "fd", cargo: "fd-find" });
  assertEquals(c.route, "cargo");
  assertEquals(c.install, c.upgrade); // cargo install == cargo upgrade to latest
});

Deno.test("commandsFor: npm carries flags", () => {
  const c = commandsFor({
    name: "Claude Code",
    npm: "@anthropic-ai/claude-code",
    npmFlags: "--foreground-scripts",
  });
  assertEquals(c.route, "npm");
  assertEquals(
    c.install,
    "npm install -g --foreground-scripts @anthropic-ai/claude-code",
  );
});

Deno.test("commandsFor: run escape hatch, no upgrade", () => {
  const c = commandsFor({ name: "custom", run: "curl x | sh" });
  assertEquals(c.route, "run");
  assertEquals(c.install, "curl x | sh");
  assertEquals(c.upgrade, null);
});

Deno.test("commandsFor: no route → all null (a need with no way to satisfy it)", () => {
  const c = commandsFor({ name: "orphan" });
  assertEquals(c, {
    route: null,
    install: null,
    uninstall: null,
    upgrade: null,
  });
});

Deno.test("loadBundles: missing dir opens inert (empty plan, no throw)", () => {
  const plan = loadBundles("/nonexistent/path/bundles");
  assertEquals(plan.bundles, []);
  assertEquals(plan.steps, []);
});
