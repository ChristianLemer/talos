# Content fixture — one file per FORM, for the engine's tests

Not a catalogue anyone installs. Every file here exists to exercise one shape the
engine knows — a route, a version form, a seed, a chain — and is named for it, with
synthetic ids (`Vendor.Two`, `vendor.ext`) so that a real product changing its id
never turns a test red for the wrong reason.

Two guards keep it honest: every key in `RawPkg::KNOWN_KEYS` appears in at least one
file here (a field added to the engine without a case fails), and `Talos --check`
reads it clean. The SHIPPED content — what a release publishes — lives at the repo
root; the property tests walk both.

`content-legacy/` beside this holds the shapes the runtime still reads for
back-compat and `--check` refuses (the multi-card `profiles.yaml`).
