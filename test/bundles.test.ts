// Tests for the bundle loader — the NAMED ROUTE TABLE and the empty-scan case.
// Pure, no pty/network. Run: deno task test  (from talos/)
import { assertEquals } from "@std/assert";
import { commandsFor, loadBundles, loadProfiles } from "../src/bundles.ts";

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

Deno.test("commandsFor: claude-plugin route (with marketplace, quoted source)", () => {
  const c = commandsFor({
    name: "Chiron",
    "claude-plugin": "chiron@tekton",
    marketplace: "github:ChristianLemer/tekton",
  });
  assertEquals(c.route, "claude-plugin");
  assertEquals(
    c.install,
    'claude plugin marketplace add "github:ChristianLemer/tekton" && claude plugin install chiron@tekton --scope user',
  );
  assertEquals(c.uninstall, "claude plugin uninstall chiron@tekton");
  assertEquals(c.upgrade, "claude plugin update chiron@tekton");
});

Deno.test("commandsFor: claude-plugin marketplace with spaces stays one quoted arg", () => {
  const c = commandsFor({
    name: "Chiron",
    "claude-plugin": "chiron@tekton",
    marketplace: "<vault>/<zone>/Tekton",
  });
  assertEquals(
    c.install,
    'claude plugin marketplace add "<vault>/<zone>/Tekton" && claude plugin install chiron@tekton --scope user',
  );
});

Deno.test("commandsFor: claude-plugin without marketplace omits the add", () => {
  const c = commandsFor({ name: "X", "claude-plugin": "x@mkt" });
  assertEquals(c.install, "claude plugin install x@mkt --scope user");
});

Deno.test("commandsFor: skill route via npx skills", () => {
  const c = commandsFor({
    name: "astral",
    skill: "astral-sh/claude-code-plugins",
  });
  assertEquals(c.route, "skill");
  assertEquals(c.install, "npx skills add astral-sh/claude-code-plugins -g -y");
  assertEquals(c.uninstall, "npx skills remove astral -y");
  assertEquals(c.upgrade, "npx skills update astral -y");
});

Deno.test("commandsFor: skill uses skillName when list-name differs", () => {
  const c = commandsFor({
    name: "Astral Python",
    skill: "astral-sh/x",
    skillName: "astral",
  });
  assertEquals(c.uninstall, "npx skills remove astral -y");
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

// steps[i] IS the visual order: sorted by their bundle's priority, and stable
// within a bundle (author's package order kept). The view no longer re-derives
// order — priority is authoritative in the MODEL. We lay the folders out so the
// filesystem-read order (dir name) DISAGREES with priority, to prove the sort.
Deno.test("loadBundles: steps come out in bundle-priority order (stable within a bundle)", () => {
  const root = Deno.makeTempDirSync();
  const write = (dir: string, yaml: string) => {
    Deno.mkdirSync(`${root}/${dir}`);
    Deno.writeTextFileSync(`${root}/${dir}/bundle.yaml`, yaml);
  };
  // dir "a-*" reads first but has the HIGHER priority → must end up LAST.
  write(
    "a-late",
    "bundle: Late\npriority: 50\npackages:\n  - name: L1\n  - name: L2\n",
  );
  write(
    "z-early",
    "bundle: Early\npriority: 0\npackages:\n  - name: E1\n  - name: E2\n",
  );

  const { steps } = loadBundles(root);
  assertEquals(
    steps.map((s) => s.name),
    ["E1", "E2", "L1", "L2"], // Early (0) before Late (50); package order kept
  );

  Deno.removeSync(root, { recursive: true });
});

Deno.test("loadProfiles: no profiles.yaml → empty list (opens fine)", () => {
  assertEquals(loadProfiles("/nonexistent/path/bundles"), []);
});

Deno.test("loadProfiles: parses profiles, defaults emoji, skips nameless", () => {
  const root = Deno.makeTempDirSync();
  Deno.writeTextFileSync(
    `${root}/profiles.yaml`,
    [
      "profiles:",
      "  - profile: Daily",
      "    description: everyday",
      "    packages: [Nushell, Claude Code]",
      "  - emoji: 👻", // no name → skipped
      "    packages: [X]",
    ].join("\n"),
  );
  const profs = loadProfiles(root);
  assertEquals(profs.length, 1);
  assertEquals(profs[0].name, "Daily");
  assertEquals(profs[0].emoji, "🎯"); // defaulted
  assertEquals(profs[0].packages, ["Nushell", "Claude Code"]);
  Deno.removeSync(root, { recursive: true });
});

Deno.test("loadProfiles: bad YAML → empty list (no throw)", () => {
  const root = Deno.makeTempDirSync();
  Deno.writeTextFileSync(`${root}/profiles.yaml`, "profiles:\n  - : : bad");
  assertEquals(loadProfiles(root), []);
  Deno.removeSync(root, { recursive: true });
});
