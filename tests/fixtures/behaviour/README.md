# Behaviour fixtures — validating the Apply ladder without installing anything

The ladder is a five-detent slider (⚡ Config only · 📦 Add missing · ☕ Unattended · 👀 Stay
nearby · 🏗️ Everything) that filters an Apply by how far you want to go. Which rung admits a
row depends on three **behaviour facts** per (package, route, os): `uac`, `403`, `slow_secs`.

Validating that filter for real would mean installing and uninstalling packages in a loop —
invasive, slow, and on macOS **impossible for `uac`** (see the asymmetry below). Point Talos
at this folder instead and every rung becomes exercisable:

```bash
pkill -f Talos
TALOS_BEHAVIOUR=$PWD/tests/fixtures/behaviour TALOS_PUBLIC=$PWD/public ./target/debug/Talos
```

The startup log then says so, and says how many files it read:

```
[behaviour] TEST MODE, reading fixtures from …/tests/fixtures/behaviour
[behaviour] 4 package(s) with collected facts; 8 of 31 step(s) know something
```

`TALOS_BEHAVIOUR` moves the **READ** only. An Apply still flushes its observations to the
real share (`<exe_dir>/behaviour/`, i.e. `target/debug/behaviour/` in a dev run), so these
files stay versioned facts rather than a scratch directory — nothing you do in the app
rewrites them.

## The four files

| file | the rung it exercises | why this package |
|---|---|---|
| `aws-cli.yaml` | 🏗️ Everything — `slow_secs: 900`, well above `ladder::SLOW_SECS` (60) | slow DOMINATES in `rung_allows`, so this row is admitted nowhere below the top. `catalog/aws-cli.yaml` declares no `slow` (nothing in `catalog/` does), so a slow AWS CLI can only have come from here |
| `nushell.yaml` | 👀 Stay nearby — `uac: true` | **deliberately NOT one of the four packages that declare `uac: true` in `catalog/`** (7-Zip, AWS CLI, Node.js, VS Code) — a seeded row looks identical whether this file was read or not. And **deliberately a row Talos manages**: Git reads `external` on the dev Mac (Xcode CLT) and both config-atoms derive out of scope, and an out-of-scope row yields no action at any rung |
| `uv.yaml` | 👀 Stay nearby — `"403": true` | **not `rclone`**, which already declares `"403": true` in `catalog/` (the real a corporate network observation, seeded). `uv` declares nothing, and a real a corporate network `uv` install did meet a real 403 |
| `jq.yaml` | ☕ Unattended — measured and quick (`slow_secs: 3`) | the CONTROL. Without a row the filter lets THROUGH on its facts, the other three prove only that rows can be excluded |

**File stems are catalogue ids.** `behaviour_io::behaviour_path` joins `format!("{id}.yaml")`,
so `aws-cli.yaml` here pairs with `catalog/aws-cli.yaml`, the same correspondence the real
share uses. A fixture named `fixture-slow.yaml` matches no package, can never appear on a
row, and looks perfectly fine while proving nothing. `behaviour_io.rs`'s
`the_shipped_fixtures_parse_and_reach_every_rung` asserts every stem against the real
`catalog/`, so a typo fails the suite rather than the eye.

## ⚠️ The `"403"` key

**The YAML key is `"403"`, not `forbidden`.** The Rust field is `forbidden` (an identifier
cannot start with a digit) and carries `#[serde(rename = "403")]`, so serde matches on the
key's *text*. A file that writes

```yaml
brew/darwin:
  forbidden: true      # ← WRONG. Parses fine. The fact is OFF.
```

parses without error into a record with the fact off — the file simply reads as "we know
nothing about this package". No warning anywhere. This mistake was made once while building
these fixtures and was caught only because the UI's rendered reason disagreed with what had
been fabricated. The test asserts the parsed value for exactly that reason.

## ⚠️ `uac` and `403` are NOT in the same position on macOS

Same rung, very different standing:

| fact | on macOS | so its fixture is… |
|---|---|---|
| `uac` | **never produced.** The window watcher that sets `saw_window` is `#[cfg(target_os = "windows")]` (`src/server.rs` — the watcher thread and both relay blocks), so no macOS run can produce `uac: true` | the ONLY way to reach that half of rung 3 on this machine |
| `403` | **produced normally.** `forbidden::is403` has no platform gate — it reads the pty's streamed bytes | a convenience, not a necessity: a Mac behind a corporate firewall really does collect this |

So **`403` is a NETWORK fact, not a Windows one.** One of these fixtures substitutes for an
impossible observation; the other merely saves a trip to the office. Do not present them as
equally fabricated.

## ⚠️ What these prove, and what they do NOT

They validate the **RUNG's behaviour**: does the filter admit the right rows, does the widget
say the right thing, does the dimmed row give the right reason.

They say **nothing about the SIGNAL's truth** — whether `watch.rs`'s "a foreign window
appeared" really means "UAC asked". An installer's own window trips the same detector, and
there is no macOS implementation to compare against. A fixture proves the ladder ROUTES a
`uac` row to rung 3. That is a different question from whether a real installer would have
set the flag correctly, and only Windows can settle the second one: section 10 of
`docs/superpowers/2026-07-31-windows-smoke-test-beta13.md` — the NEGATIVE check, that a quiet
package must NOT gain `uac: true` — remains the decider.

## ⚠️ Reproducing a five-rung spread

**Rung facts only separate UPGRADES.** An install is rung 1 whatever the facts say, and
`rung_allows` gives `Uninstall` every rung. So a fixture is invisible unless its row is
present-and-outdated when you look — which no checked-in file can guarantee, since it depends
on what is installed on the day.

On a machine where everything is current, raise the `version:` pins temporarily: a pin above
the installed version makes the action an `upgrade` (`decision::action_for`). That is how the
spread below was produced (2026-08-02, dev Mac, brew) — `aws-cli` → `2.99.0`, `uv` → `0.99.0`,
`jq` → `1.99.0`, `nushell` → `0.114.1` (the last one a real available version, not a
fiction). **Revert the pins afterwards and confirm `jj st` is clean.**

⚠️ Wait for the header to read `ready` before believing any count. An unfinished scan counts
un-probed rows as absent, which has produced false readings twice.

With those four pins raised and this folder active, the widget read:

| rung | count | rows it dimmed, with the reason it showed |
|---|---|---|
| 0 ⚡ Config only | `0 items` + "Nothing to do at this level. There is more further right." | AWS CLI · jq · Nushell · uv = "an update"; 7-Zip · Espanso · Miro = "an install" |
| 1 📦 Add missing | `3 items · 3 unknown` | AWS CLI · jq · Nushell · uv = "an update" |
| 2 ☕ Unattended | `4 items · <1 min for 1 and 3 unknown` | AWS CLI = "slow — allow time" · Nushell = "needs your hand" · uv = "blocked here" |
| 3 👀 Stay nearby | `6 items · <1 min for 1 and 5 unknown` | AWS CLI = "slow — allow time" |
| 4 🏗️ Everything | `7 items · <1 min for 1 and 6 unknown` | (none) |

Rung 0 legitimately reads `0 items`: both config-atoms (Bun PATH, Starship config) derive
**out of scope** on this Mac — neither can be uninstalled — so `actionOf` returns null for
them and "config only" has nothing to do. That is not a bug; pull one in via the scope switch
if you want to see rung 0 act.

Every reason in that table is the one the corresponding fixture fabricated, which is what
makes the run evidence rather than a smoke test: the `uac` row said "needs your hand" on a
platform that cannot observe elevation at all.
