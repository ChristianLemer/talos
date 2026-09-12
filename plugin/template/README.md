# Talos content

The **content half** of a Talos kit. The engine is a published release, identical for
everyone; what makes a kit *ours* is what is in these two folders.

```
catalog/          one YAML per package — the file stem is its id, `name:` is what everything refers to
bundles/          one YAML per card — pulls packages in by name, `needs:` other bundles
.talos-version    one line: the release this kit runs
REFERENCE.md      every field, what it means, and the traps
AGENTS.md         the loop and the doctrine, for whoever picks this up next
```

## Change what Talos proposes

Edit a file in `catalog/` or `bundles/`, then:

```bash
Talos --check catalog bundles     # must say `0 errors`
```

CI runs the same check, with the engine this repo is pinned to.

## Move to a newer Talos

Edit `.talos-version` — one line, one commit — then re-run `--check` with the new binary.

## Publish

```bash
sh get-talos.sh "<kit folder>" --content .      # macOS, Linux
.\get-talos.ps1 "<kit folder>" -Content .        # Windows
```

`get-talos.sh` / `get-talos.ps1` ship with every Talos release. They fetch the pinned
launchers, put this content beside them, verify every byte, and run `--check` with the
very binary the kit ships — a kit that does not pass is not left behind.
