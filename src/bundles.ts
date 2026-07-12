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
import { MANAGERS, nativeManager } from "./managers.ts";
import type { Os } from "./platform.ts";

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

// A profile — a named, additive package selection (see profiles.yaml). `packages`
// lists package NAMES (the same key selection.json uses). Pure data; the on/full/
// hollow state and the additive pull live in decision.js.
export interface Profile {
  name: string;
  emoji: string;
  usage: string; // a "what you do with it" sentence (falls back to description)
  highlights: string[]; // 2-3 salient package names to advertise the profile
  description: string;
  packages: string[];
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
  systemId: string | null; // the ACTIVE system-manager id (winget on Win, brew on Mac) — keys the outdated scan
  detect: string | null;
  // run-route detection: a DRY-RUN command run verbatim, exit 0 = converged/present.
  // Distinct from `detect` (which probes a binary on PATH by its first token) — a
  // config-atom shares its apply-logic with this check, so detection can't drift.
  check: string | null;
  // Optional REGEX refining how the version is pulled from detect/route output.
  // Absent → the per-route default extraction is used. Present → capture group 1
  // (or the whole match) IS the version. Added only when the default gets it
  // wrong — the displayed version is the diagnostic that reveals which package
  // needs one. Pure JS regex, no shell, no dependency.
  versionRegex: string | null;
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
  "claude-plugin"?: string; // <plugin>@<marketplace>
  marketplace?: string; // source for `claude plugin marketplace add`
  skill?: string; // source for `npx skills add`
  skillName?: string; // list-name if it differs from `name`
  detect?: string;
  check?: string; // run-route: a dry-run command; exit 0 = converged/present
  "version-regex"?: string; // optional regex refining version extraction from output
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
  os: Os,
): { route: string | null } & Commands {
  // FAMILY 1 — system manager, arbitrated by OS. If the package declares an id
  // for the manager native to THIS os (pkg.winget on Windows, pkg.brew on
  // darwin/linux), use it; the non-native system id is ignored (winget on a Mac
  // is unusable). One active system route per machine.
  const mgr = nativeManager(os);
  if (mgr) {
    const id = mgr.idField === "winget" ? pkg.winget : pkg.brew;
    if (id) {
      return {
        route: mgr.route,
        install: mgr.install(id),
        uninstall: mgr.uninstall(id),
        upgrade: mgr.upgrade(id),
      };
    }
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
  if (pkg["claude-plugin"]) {
    // A Claude Code plugin (hooks/agents/commands/MCP). marketplace add chained
    // before install (idempotent); && so a failed add aborts the install.
    // Omitted when no marketplace declared (plugin from an already-known one).
    const id = pkg["claude-plugin"];
    // Quote the source: a marketplace can be a local directory path with spaces
    // (e.g. the tekton folder on OneDrive), which would otherwise split into
    // several shell args. A github owner/repo is unaffected by the quotes.
    const add = pkg.marketplace
      ? `claude plugin marketplace add "${pkg.marketplace}" && `
      : "";
    return {
      route: "claude-plugin",
      install: `${add}claude plugin install ${id} --scope user`,
      uninstall: `claude plugin uninstall ${id}`,
      upgrade: `claude plugin update ${id}`,
    };
  }
  if (pkg.skill) {
    // A cross-agent skill (SKILL.md) via npx skills. list-name may differ from
    // the display name, hence skillName for uninstall/upgrade targeting.
    const src = pkg.skill;
    const name = pkg.skillName ?? pkg.name;
    return {
      route: "skill",
      install: `npx skills add ${src} -g -y`,
      uninstall: `npx skills remove ${name} -y`,
      upgrade: `npx skills update ${name} -y`,
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
  os: Os,
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
      // `{dir}` in any command expands to the bundle's own folder (absolute), so
      // a config-atom can call a script it ships (e.g. `nu "{dir}/patch.nu" apply`)
      // instead of inlining nu with quotes that don't survive the Windows
      // powershell → nu command line. Quoted at the call site in the YAML.
      const bundleDir = `${root}/${dir.name}`;
      const sub = (s: string | null) =>
        s === null ? null : s.replaceAll("{dir}", bundleDir);
      for (const p of (b.packages ?? [])) {
        const cmd = commandsFor(p, os);
        // the id of whichever system manager won for this os — null for a non-system
        // route, or when the package declares no id for this os's native manager.
        const mgr = MANAGERS.find((m) => m.route === cmd.route);
        const winnerId = mgr && (mgr.idField === "winget" ? p.winget : p.brew);
        const systemId = winnerId || null;
        steps.push({
          bundle: meta.name,
          name: p.name,
          description: p.description || "",
          install: sub(cmd.install),
          uninstall: sub(cmd.uninstall),
          upgrade: sub(cmd.upgrade),
          route: cmd.route,
          systemId,
          detect: p.detect || null,
          check: sub(p.check || null),
          versionRegex: p["version-regex"] || null,
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

// Raw profile as read from profiles.yaml (loose, like RawPkg).
interface RawProfile {
  profile?: string;
  emoji?: string;
  usage?: string;
  highlights?: string[];
  description?: string;
  packages?: string[];
}

// Scan the single profiles.yaml at the root of the bundles dir into the panel's
// data: a `columns` count (grid width, default 2) and a list of profiles. Missing
// file or bad YAML → { columns: 2, profiles: [] } (profiles are optional; the
// panel just shows none). A profile with no name is skipped. `usage` falls back
// to `description` so the card always has a sentence; `highlights` defaults to
// []. Package names are NOT validated against the plan here — an unknown name
// simply pulls nothing, which is harmless and keeps this loader pure/decoupled
// from the step list.
export function loadProfiles(
  root: string,
  log: (msg: string) => void = () => {},
): { columns: number; profiles: Profile[] } {
  let raw: string;
  try {
    raw = Deno.readTextFileSync(`${root}/profiles.yaml`);
  } catch {
    return { columns: 2, profiles: [] }; // no profiles.yaml → opens fine
  }
  try {
    const parsed = parseYaml(raw) as {
      columns?: number;
      profiles?: RawProfile[];
    };
    const columns = parsed.columns ?? 2;
    const out: Profile[] = [];
    for (const p of (parsed.profiles ?? [])) {
      if (!p.profile) continue; // a profile needs a name
      const description = p.description || "";
      out.push({
        name: p.profile,
        emoji: p.emoji || "🎯",
        usage: p.usage || description,
        highlights: p.highlights ?? [],
        description,
        packages: p.packages ?? [],
      });
    }
    log(`profiles loaded: ${out.length}`);
    return { columns, profiles: out };
  } catch (e) {
    log(`profiles skipped (bad YAML): ${(e as Error).message}`);
    return { columns: 2, profiles: [] };
  }
}
