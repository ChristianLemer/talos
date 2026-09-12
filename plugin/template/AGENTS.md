# Working in this repo

This is a **Talos content repo**. It holds no code. `catalog/` and `bundles/` are the YAML
a Talos kit reads from disk at runtime, and `.talos-version` names the engine that reads
them. Changing what Talos proposes is a commit here.

## The loop

Write → `Talos --check catalog bundles` → fix → **until it says `0 errors`**. Never hand
back content that has not been through it, and never resolve a finding by deleting the
file that produced it. `--check` is stricter than the runtime on purpose: at runtime a
broken file is skipped so one bad row cannot take down the screen; at authoring time that
same silence is a package that quietly stopped existing.

On Windows, redirect the output — the release exe is a GUI program and prints to a file:
`Talos.exe --check catalog bundles > check.txt`. The exit code is the contract.

## The doctrine

1. **Detect, don't remember.** `detect:` is observed now. No sentinel, no state database.
2. **Every command is non-interactive.** The row terminal is display-only; a command that
   prompts deadlocks the step forever. `brew --yes`, `winget --accept-*`, `npx --yes`.
3. **Names, not ids.** `packages:`, `needs:` and `requires:` resolve against `name:` —
   never the file stem, never the route id.
4. **One route per package**, except the two system managers: declare both `winget:` and
   `brew:` and the engine picks the native one.
5. **Capability, not category.** `category:` is display only and decides nothing. Anything
   the engine acts on has its own key — `doctor:` is the worked example.
6. **MEASURED means measured.** `uac:`, `"403":`, `slow:` and `doctor.clean` are claims
   about a real machine. Declare them from an observation you actually made.
7. **A sidecar is not a package.** `catalog/` holds `.yaml` and the `.nu` scripts a
   config-atom calls. Count YAML.

## The field reference

`REFERENCE.md` — every key, what it means, and the traps. It is a **verbatim copy** from
the talos plugin; do not edit it, refresh it.

For the full authoring skill:

```bash
claude plugin marketplace add ChristianLemer/talos
claude plugin install talos@talos
```
