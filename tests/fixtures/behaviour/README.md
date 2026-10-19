# Behaviour fixtures — seeing the row labels without installing anything

> Since 2026-09-09 the subjects are packages of the CONTENT fixture
> (`tests/fixtures/content/catalog/`), not of the shipped socle: `chain-a`, `chain-b`,
> `chain-c` and `editor` declare no seed, so a fact read from here is provably from here.

Every row shows what a package will do to you, from three **behaviour facts** per (package,
route, os): `uac` → *asks for your password*, `403` → *was blocked*, `slow_secs` at or above
`plan_facts::SLOW_SECS` (60) → *takes a while*. Until 2026-10-10 the same facts also fed a
ladder that filtered the Apply by kind (⚡ / 🧩 / 📦, and before that five rungs sorted by these
very facts); the ladder left, the labels stayed. Nothing filters on a fact any more.

Seeing those labels for real would mean installing and uninstalling packages in a loop —
invasive, slow, and on macOS **impossible for `uac`** (see the asymmetry below). Point Talos
at this folder instead and every label becomes visible:

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

| file | the label it shows | why this package |
|---|---|---|
| `chain-a.yaml` | *takes a while* — `slow_secs: 900`, well above `plan_facts::SLOW_SECS` (60) | nothing in the content fixture declares `slow`, so a slow row can only have come from here |
| `chain-c.yaml` | *asks for your password* — `uac: true` | **deliberately a package that declares no `uac`** — a seeded row looks identical whether this file was read or not |
| `chain-b.yaml` | *was blocked* — `"403": true` | the subject must declare nothing of its own, and it MOVES as facts get promoted |
| `editor.yaml` | none — measured and quick (`slow_secs: 3`) | the CONTROL. Without a row that shows NO label, the other three prove only that labels appear, never that they are earned |

**File stems are catalogue ids.** `behaviour_io::behaviour_path` joins `format!("{id}.yaml")`,
so `chain-a.yaml` here pairs with `tests/fixtures/content/catalog/chain-a.yaml`, the same
correspondence the real share uses. A fixture named `fixture-slow.yaml` matches no package,
can never appear on a row, and looks perfectly fine while proving nothing. `behaviour_io.rs`'s
`the_shipped_fixtures_parse_and_cover_every_fact` asserts every stem, so a typo fails the
suite rather than the eye.

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

Same kind of label, very different standing:

| fact | on macOS | so its fixture is… |
|---|---|---|
| `uac` | **never produced.** The window watcher that sets `saw_window` is `#[cfg(target_os = "windows")]` (`src/server.rs` — the watcher thread and both relay blocks), so no macOS run can produce `uac: true` | the ONLY way to see that label on this machine |
| `403` | **produced normally.** `forbidden::is403` has no platform gate — it reads the pty's streamed bytes | a convenience, not a necessity: a Mac behind a corporate firewall really does collect this |

So **`403` is a NETWORK fact, not a Windows one.** One of these fixtures substitutes for an
impossible observation; the other merely saves a trip to the office. Do not present them as
equally fabricated.

## ⚠️ What these prove, and what they do NOT

They validate the **LABEL**: does the row say the right thing for the fact it was given.

They say **nothing about the SIGNAL's truth** — whether `watch.rs`'s "a foreign window
appeared" really means "UAC asked". An installer's own window trips the same detector, and
there is no macOS implementation to compare against. Only Windows can settle that: the
NEGATIVE check of the beta.13 Windows smoke test — that a quiet package must NOT gain
`uac: true` — remains the decider.
