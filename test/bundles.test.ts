// Tests for the bundle loader — the NAMED ROUTE TABLE and the empty-scan case.
// Pure, no pty/network. Run: deno task test  (from talos/)
import { assertEquals } from "@std/assert";
import { commandsFor, loadBundles, loadProfiles } from "../src/bundles.ts";
import { outdatedFor } from "../src/outdated.ts";

Deno.test("commandsFor: winget route on windows", () => {
  const c = commandsFor({ name: "Git", winget: "Git.Git" }, "windows");
  assertEquals(c.route, "winget");
  assertEquals(
    c.install,
    "winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements",
  );
});

Deno.test("commandsFor: brew route on darwin", () => {
  const c = commandsFor({ name: "ripgrep", brew: "ripgrep" }, "darwin");
  assertEquals(c.route, "brew");
  assertEquals(c.install, "brew install --yes ripgrep");
  assertEquals(c.upgrade, "brew upgrade --yes ripgrep");
});

Deno.test("commandsFor: winget+brew package picks the NATIVE manager per OS", () => {
  const pkg = {
    name: "ripgrep",
    winget: "BurntSushi.ripgrep.MSVC",
    brew: "ripgrep",
  };
  assertEquals(commandsFor(pkg, "windows").route, "winget");
  assertEquals(commandsFor(pkg, "darwin").route, "brew");
  assertEquals(commandsFor(pkg, "linux").route, "brew");
  assertEquals(
    commandsFor(pkg, "darwin").install,
    "brew install --yes ripgrep",
  );
});

Deno.test("commandsFor: cargo reinstalls as its upgrade", () => {
  const c = commandsFor({ name: "fd", cargo: "fd-find" }, "darwin");
  assertEquals(c.route, "cargo");
  assertEquals(c.install, c.upgrade); // cargo install == cargo upgrade to latest
});

Deno.test("commandsFor: npm carries flags", () => {
  const c = commandsFor({
    name: "Claude Code",
    npm: "@anthropic-ai/claude-code",
    npmFlags: "--foreground-scripts",
  }, "darwin");
  assertEquals(c.route, "npm");
  assertEquals(
    c.install,
    "npm install -g --foreground-scripts @anthropic-ai/claude-code",
  );
});

Deno.test("commandsFor: run escape hatch, no upgrade", () => {
  const c = commandsFor({ name: "custom", run: "curl x | sh" }, "darwin");
  assertEquals(c.route, "run");
  assertEquals(c.install, "curl x | sh");
  assertEquals(c.upgrade, null);
});

Deno.test("commandsFor: claude-plugin route (with marketplace, quoted source)", () => {
  const c = commandsFor({
    name: "Chiron",
    "claude-plugin": "chiron@tekton",
    marketplace: "github:ChristianLemer/tekton",
  }, "darwin");
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
  }, "darwin");
  assertEquals(
    c.install,
    'claude plugin marketplace add "<vault>/<zone>/Tekton" && claude plugin install chiron@tekton --scope user',
  );
});

Deno.test("commandsFor: claude-plugin without marketplace omits the add", () => {
  const c = commandsFor({ name: "X", "claude-plugin": "x@mkt" }, "darwin");
  assertEquals(c.install, "claude plugin install x@mkt --scope user");
});

Deno.test("commandsFor: skill route via npx skills", () => {
  const c = commandsFor({
    name: "astral",
    skill: "astral-sh/claude-code-plugins",
  }, "darwin");
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
  }, "darwin");
  assertEquals(c.uninstall, "npx skills remove astral -y");
});

Deno.test("commandsFor: no route → all null (a need with no way to satisfy it)", () => {
  const c = commandsFor({ name: "orphan" }, "darwin");
  assertEquals(c, {
    route: null,
    install: null,
    uninstall: null,
    upgrade: null,
    downgrade: null,
  });
});

// --- version pinning: install/upgrade target the exact pin; downgrade =
// uninstall && install-at-pin (destructive path, uniform). No version → downgrade
// null and install/upgrade unchanged. See memory talos-version-pin.
Deno.test("commandsFor: unpinned package has no downgrade (pin is the only source)", () => {
  const c = commandsFor({ name: "ripgrep", brew: "ripgrep" }, "darwin");
  assertEquals(c.downgrade, null);
  assertEquals(c.install, "brew install --yes ripgrep"); // unchanged
  assertEquals(c.upgrade, "brew upgrade --yes ripgrep"); // unchanged
});

Deno.test("commandsFor: brew pinned → versioned formula, downgrade uninstalls first", () => {
  const c = commandsFor({ name: "jq", brew: "jq", version: "1.8" }, "darwin");
  assertEquals(c.install, "brew install --yes jq@1.8");
  assertEquals(c.upgrade, "brew install --yes jq@1.8"); // upgrade-to-pin = install at pin
  assertEquals(c.downgrade, "brew uninstall jq && brew install --yes jq@1.8");
});

Deno.test("commandsFor: winget pinned → --version, downgrade uninstalls first", () => {
  const c = commandsFor(
    { name: "Git", winget: "Git.Git", version: "2.43.0" },
    "windows",
  );
  assertEquals(
    c.install,
    "winget install --id Git.Git -e --version 2.43.0 --source winget --accept-source-agreements --accept-package-agreements",
  );
  assertEquals(
    c.downgrade,
    "winget uninstall --id Git.Git -e --source winget && winget install --id Git.Git -e --version 2.43.0 --source winget --accept-source-agreements --accept-package-agreements",
  );
});

Deno.test("commandsFor: cargo pinned → --version", () => {
  const c = commandsFor(
    { name: "fd", cargo: "fd-find", version: "10.1.0" },
    "darwin",
  );
  assertEquals(c.install, "cargo install fd-find --version 10.1.0");
  assertEquals(c.upgrade, "cargo install fd-find --version 10.1.0");
  assertEquals(
    c.downgrade,
    "cargo uninstall fd-find && cargo install fd-find --version 10.1.0",
  );
});

Deno.test("commandsFor: npm pinned → @version (flags preserved)", () => {
  const c = commandsFor(
    {
      name: "Claude Code",
      npm: "@anthropic-ai/claude-code",
      npmFlags: "--foreground-scripts",
      version: "1.2.3",
    },
    "darwin",
  );
  assertEquals(
    c.install,
    "npm install -g --foreground-scripts @anthropic-ai/claude-code@1.2.3",
  );
});

Deno.test("loadBundles: missing dir opens inert (empty plan, no throw)", () => {
  const plan = loadBundles("/nonexistent/path/bundles", "darwin");
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

  const { steps } = loadBundles(root, "darwin");
  assertEquals(
    steps.map((s) => s.name),
    ["E1", "E2", "L1", "L2"], // Early (0) before Late (50); package order kept
  );

  Deno.removeSync(root, { recursive: true });
});

Deno.test("loadBundles: systemId is the winget id on windows (exists today)", () => {
  const { steps } = loadBundles("bundles", "windows", () => {});
  const rg = steps.find((s) => s.name === "ripgrep");
  assertEquals(rg?.route, "winget");
  assertEquals(rg?.systemId, "BurntSushi.ripgrep.MSVC");
});

Deno.test("loadBundles: winget+brew package resolves to brew on darwin", () => {
  const { steps } = loadBundles("bundles", "darwin", () => {});
  const rg = steps.find((s) => s.name === "ripgrep");
  assertEquals(rg?.route, "brew");
  assertEquals(rg?.systemId, "ripgrep");
  assertEquals(rg?.install, "brew install --yes ripgrep");
});

Deno.test("outdated bridge: darwin systemId matches a brew-keyed scan (regression: was wingetId)", () => {
  const { steps } = loadBundles("bundles", "darwin", () => {});
  const obs = steps.find((s) => s.name === "Obsidian");
  // brew scan is keyed by the bare brew id:
  const scan = new Map([["obsidian", {
    current: "1.5.0",
    available: "1.6.0",
  }]]);
  // the correct id to look up is systemId (brew id on Mac), NOT any winget id:
  assertEquals(obs?.systemId, "obsidian");
  assertEquals(outdatedFor(obs?.systemId ?? null, scan)?.available, "1.6.0");
});

Deno.test("loadBundles: Starship config is a config-atom (isConfig true)", () => {
  const { steps } = loadBundles("bundles", "windows", () => {});
  const cfg = steps.find((s) => s.name === "Starship config");
  assertEquals(cfg?.isConfig, true);
});

Deno.test("loadBundles: an installable package is NOT a config-atom", () => {
  const { steps } = loadBundles("bundles", "windows", () => {});
  const git = steps.find((s) => s.name === "Git");
  assertEquals(git?.isConfig, false);
});

Deno.test("loadProfiles: no profiles.yaml → empty list (opens fine)", () => {
  assertEquals(loadProfiles("/nonexistent/path/bundles").profiles, []);
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
  const { profiles } = loadProfiles(root);
  assertEquals(profiles.length, 1);
  assertEquals(profiles[0].name, "Daily");
  assertEquals(profiles[0].emoji, "🎯"); // defaulted
  assertEquals(profiles[0].packages, ["Nushell", "Claude Code"]);
  Deno.removeSync(root, { recursive: true });
});

Deno.test("loadProfiles: bad YAML → empty list (no throw)", () => {
  const root = Deno.makeTempDirSync();
  Deno.writeTextFileSync(`${root}/profiles.yaml`, "profiles:\n  - : : bad");
  assertEquals(loadProfiles(root).profiles, []);
  Deno.removeSync(root, { recursive: true });
});

Deno.test("loadProfiles: parses usage, highlights, and root columns", () => {
  const root = Deno.makeTempDirSync();
  Deno.writeTextFileSync(
    `${root}/profiles.yaml`,
    [
      "columns: 3",
      "profiles:",
      "  - profile: Dev",
      "    emoji: 🛠️",
      "    usage: Build things",
      "    highlights: [jj, Helix]",
      "    packages: [jj, Helix]",
    ].join("\n"),
  );
  const { columns, profiles } = loadProfiles(root);
  assertEquals(columns, 3);
  assertEquals(profiles[0].usage, "Build things");
  assertEquals(profiles[0].highlights, ["jj", "Helix"]);
  Deno.removeSync(root, { recursive: true });
});

Deno.test("loadProfiles: usage falls back to description, columns defaults to 2", () => {
  const root = Deno.makeTempDirSync();
  Deno.writeTextFileSync(
    `${root}/profiles.yaml`,
    [
      "profiles:",
      "  - profile: Legacy",
      "    description: Old kit",
      "    packages: [X]",
    ]
      .join("\n"),
  );
  const { columns, profiles } = loadProfiles(root);
  assertEquals(columns, 2);
  assertEquals(profiles[0].usage, "Old kit");
  assertEquals(profiles[0].highlights, []);
  Deno.removeSync(root, { recursive: true });
});

Deno.test("loadProfiles: no file → columns 2, empty profiles", () => {
  const { columns, profiles } = loadProfiles("/nonexistent/path/bundles");
  assertEquals(columns, 2);
  assertEquals(profiles, []);
});
