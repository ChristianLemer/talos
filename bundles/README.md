# Authoring content — where the reference lives

These two folders — `catalog/` and `bundles/` — **are** the content Talos proposes. The
engine is generic and knows nothing about your team; what makes a kit *yours* lives here.
Edit the YAML: no build step, the exe reads both folders from disk beside it at runtime.

> New here? Copy a file from `catalog/`, rename it, edit it. Then list its `name:` in a
> bundle. Every file in this socle is written as a lesson — read the one nearest your case.

Then, before the kit ships:

```bash
Talos --check catalog bundles     # exit 1 on a broken file, an unknown name, a bad version
```

## The full field reference

Every key, what the engine does with it, and the traps:
**[`plugin/skills/talos-content/references/fields.md`](https://github.com/ChristianLemer/talos/blob/beta/plugin/skills/talos-content/references/fields.md)**

It moved out of this file so there would be exactly one copy of it: it is now the
reference *inside the `talos` plugin*, guarded by a test that fails when the engine grows
a field the reference does not mention.

## The agent that writes this for you

```bash
claude plugin marketplace add github:ChristianLemer/talos
claude plugin install talos@talos
```

You get the reference as a skill (`talos-content`), the kit and Doctor guidance
(`talos-kit`), and `/talos:init` — which scaffolds a content repo of your own with the
reference copied into it, so it stays readable with no plugin and no network.
