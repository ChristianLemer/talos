// bundles.ts — scan the bundles/ folder into the plan the UI renders.
//
// A bundle is a FOLDER (bundle.yaml + an optional config/). This module reads
// every subfolder of ONE root — bundles/ beside the exe (see BUNDLES_DIR in
// server.ts) — parses each bundle.yaml, and flattens it into { bundles, steps }.
// Pure data + pure functions: NO pty, NO network, NO Deno.serve — so the whole
// thing is unit-testable on Mac (see test/bundles.test.ts), the same discipline
// that keeps decision.js a DOM-free rule.
//
// Ported from server.js (Node). One change of substance: commandsFor grows from
// winget/npm/run into a NAMED ROUTE TABLE (winget/brew/cargo/npm/run) — a package
// is a NEED satisfied by one of several named routes, and the engine will pick
// the route practicable on THIS machine (route/capability model, decided
// 2026-07-04). Still MONO-ROUTE for now: a package declares at most one today.

import { parse as parseYaml } from "@std/yaml";

export type Posture = "mandatory" | "opt-out" | "opt-in" | "forbidden";
const POSTURES: Posture[] = ["mandatory", "opt-out", "opt-in", "forbidden"];

export interface BundleMeta {
  name: string;
  emoji: string;
  description: string;
  priority: number;
  selectable: boolean;
  posture: Posture; // the bundle's DEFAULT posture, inherited by every package
}

export interface Commands {
  install: string | null;
  uninstall: string | null;
  upgrade: string | null;
}

export interface Step extends Commands {
  bundle: string;
  name: string;
  description: string;
  route: string | null; // which named route satisfies this package (winget/brew/…)
  wingetId: string | null; // the id winget reports in `winget upgrade` — matches outdated rows
  detect: string | null;
  requires: string[];
  posture: Posture;
}

// A raw package as read from YAML — every route field is optional, the engine
// keys off whichever is present. Kept loose on purpose: adding a manager means
// adding a case below, not a schema migration.
interface RawPkg {
  name: string;
  description?: string;
  winget?: string;
  brew?: string;
  cargo?: string;
  npm?: string;
  npmFlags?: string;
  run?: string;
  runUninstall?: string;
  detect?: string;
  requires?: string[];
}

interface RawBundle {
  bundle?: string;
  emoji?: string;
  description?: string;
  priority?: number;
  selectable?: boolean;
  posture?: string;
  packages?: RawPkg[];
}

// Build a package manager's install/uninstall/upgrade commands from its named
// route field. This is the NAMED ROUTE TABLE — winget stays winget, brew stays
// brew; we don't abstract into opaque command strings (the engine knows no
// specific tool, but the routes it DOES know, it knows plainly). Adding a
// manager = adding a case here; the data files stay clean.
//
// Returns the commands AND which route matched, so detection/uninstall can go
// back through the same route that owns the package.
export function commandsFor(
  pkg: RawPkg,
): { route: string | null } & Commands {
  if (pkg.winget) {
    // --source winget: pin to the winget source ONLY (never msstore — we install
    // nothing from it, and it timed out on the VM, hanging every command). See
    // server.js note. -e = exact id match.
    return {
      route: "winget",
      install:
        `winget install --id ${pkg.winget} -e --source winget --accept-source-agreements --accept-package-agreements`,
      uninstall: `winget uninstall --id ${pkg.winget} -e --source winget`,
      upgrade:
        `winget upgrade --id ${pkg.winget} -e --source winget --accept-source-agreements --accept-package-agreements`,
    };
  }
  if (pkg.brew) {
    // brew handles cask vs formula itself; --quiet trims chatter. brew is the
    // Mac/Linux counterpart of winget — the route that lets Talos run for real
    // on this dev machine.
    return {
      route: "brew",
      install: `brew install ${pkg.brew}`,
      uninstall: `brew uninstall ${pkg.brew}`,
      upgrade: `brew upgrade ${pkg.brew}`,
    };
  }
  if (pkg.cargo) {
    // cargo: cross-platform, no uninstall-by-upgrade — reinstall IS the upgrade.
    return {
      route: "cargo",
      install: `cargo install ${pkg.cargo}`,
      uninstall: `cargo uninstall ${pkg.cargo}`,
      upgrade: `cargo install ${pkg.cargo}`, // cargo reinstalls to latest
    };
  }
  if (pkg.npm) {
    const flags = pkg.npmFlags ? pkg.npmFlags + " " : "";
    return {
      route: "npm",
      install: `npm install -g ${flags}${pkg.npm}`,
      uninstall: `npm uninstall -g ${pkg.npm}`,
      // npm has no cheap "list everything outdated" we parse today, so npm
      // upgrades are never TRIGGERED — but the command is here, future-ready.
      upgrade: `npm install -g ${flags}${pkg.npm}@latest`,
    };
  }
  if (pkg.run) {
    // Escape hatch: a raw command. No upgrade notion.
    return {
      route: "run",
      install: pkg.run,
      uninstall: pkg.runUninstall ?? null,
      upgrade: null,
    };
  }
  return { route: null, install: null, uninstall: null, upgrade: null };
}

function normPosture(p: string | undefined): Posture {
  return (p && (POSTURES as string[]).includes(p)) ? p as Posture : "mandatory";
}

export interface Plan {
  bundles: BundleMeta[];
  steps: Step[];
}

// Scan ONE root (bundles/ beside the exe). Each subfolder with a bundle.yaml is
// a bundle; a broken YAML is skipped with a logged message, not defended against
// (robust, not maternal). A missing root yields an EMPTY plan — an exe with no
// bundles beside it opens inert, it does not crash (core = proposition only).
export function loadBundles(
  root: string,
  log: (msg: string) => void = () => {},
): Plan {
  const bundles: BundleMeta[] = [];
  const steps: Step[] = [];
  let entries: Deno.DirEntry[];
  try {
    entries = [...Deno.readDirSync(root)];
  } catch {
    log(`no bundles dir at ${root} — opening inert (no content to propose)`);
    return { bundles, steps };
  }
  for (const dir of entries) {
    if (!dir.isDirectory) continue;
    const file = `${root}/${dir.name}/bundle.yaml`;
    let raw: string;
    try {
      raw = Deno.readTextFileSync(file);
    } catch {
      continue; // a subfolder without a bundle.yaml is simply not a bundle
    }
    try {
      const b = parseYaml(raw) as RawBundle;
      const meta: BundleMeta = {
        name: b.bundle || dir.name,
        emoji: b.emoji || "📦",
        description: b.description || "",
        priority: b.priority ?? 100,
        selectable: b.selectable !== false,
        // the bundle's DEFAULT posture — lets the front lean its "auto" pill.
        posture: normPosture(b.posture),
      };
      bundles.push(meta);
      // Posture is a BUNDLE-level policy — the author's intent for the whole
      // category, inherited uniformly by every package (a bundle mixing opt-in
      // and opt-out rows would make its own pill lie: want different → new bundle).
      for (const p of (b.packages ?? [])) {
        const cmd = commandsFor(p);
        steps.push({
          bundle: meta.name,
          name: p.name,
          description: p.description || "",
          install: cmd.install,
          uninstall: cmd.uninstall,
          upgrade: cmd.upgrade,
          route: cmd.route,
          wingetId: p.winget || null,
          detect: p.detect || null,
          requires: p.requires ?? [],
          posture: meta.posture,
        });
      }
      log(
        `bundle loaded: ${meta.name} (${
          (b.packages ?? []).length
        } packages) from ${dir.name}`,
      );
    } catch (e) {
      log(`bundle skipped (bad YAML): ${dir.name} — ${(e as Error).message}`);
    }
  }
  bundles.sort((a, b) => a.priority - b.priority);
  // steps[i] IS the visual order: sort by the owning bundle's priority so the
  // model's canonical order matches the screen (view no longer re-derives it),
  // and Apply can just walk indices top-to-bottom. Array.sort is STABLE, so a
  // bundle's packages keep the author's order. priority lives in the MODEL, not
  // only the render loop.
  const priorityOf = new Map(bundles.map((b) => [b.name, b.priority]));
  steps.sort((a, b) =>
    (priorityOf.get(a.bundle) ?? 100) - (priorityOf.get(b.bundle) ?? 100)
  );
  return { bundles, steps };
}
