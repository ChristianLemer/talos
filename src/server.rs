use axum::{
    body::Body,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE},
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::bundles::{load_from_catalog, Plan};
use crate::consent::{
    append_history, append_record, clear_history, read_consent, read_history, share_record,
    shared_log_path, write_consent, ConsentStore, HistEntry,
};
// ⚠️ The two scans call `detect_present_detailed_with_vscode` by its full path rather than
// importing it: the name is long enough that an import would read as an alias, and the full path
// says at the call site which of the three arities is being used.
use crate::detect::detect_present_detailed;
use crate::outdated::{outdated_for, scan_outdated};
use crate::platform::{current_os, local_data_dir, Os};
use crate::profiles::{load_profiles, Profiles};
use crate::selection::{
    read_selection, read_selection_or_first_launch, write_selection, Selection,
};

/// Below this many rows to probe, the machine-wide presence listing costs more than
/// the per-package probes it replaces (~3.7 s for one `winget list` vs ~1.3 s per
/// package, measured on Windows 2026-08-09). The opening scan is always above it;
/// the Apply re-scan, scoped to the diff, is usually below.
const BULK_PRESENCE_WORTH_IT: usize = 3;

/// The value for the `appmgmt` key sent to the front (wire string).
fn appmgmt_wire(status: crate::platform::AppMgmtStatus) -> &'static str {
    status.as_str()
}

/// True when running this action on this step REQUIRES App Management that we
/// don't currently have: a brew CASK upgrade on macOS with status Missing.
/// Everything else (formulae, winget, install/uninstall, other OSes, granted or
/// n/a) is never gated.
fn cask_upgrade_blocked(
    action: crate::decision::Action,
    is_cask: bool,
    appmgmt: crate::platform::AppMgmtStatus,
) -> bool {
    use crate::decision::Action;
    use crate::platform::AppMgmtStatus;
    action == Action::Upgrade && is_cask && appmgmt == AppMgmtStatus::Missing
}

/// The reason a pinned row ABOVE its pin stays where it is: this route builds no downgrade
/// command (a manager package whose pin is `latest`-shaped, a `run:` package). Logged here and
/// shown on the row by the front, which reads `canDowngrade` off the plan — never swallowed.
const NO_DOWNGRADE_ROUTE: &str = "above its pin — this route cannot go down to it";

/// What the batch does with the rule's answer. Every action `action_for` returns is now
/// batched — the downgrade included, since the per-row button that used to be its only
/// path left on 2026-10-10 (the Apply is the one path that acts). A downgrade is an
/// uninstall + reinstall, so a route with no downgrade command cannot take it: that row
/// gets a reason instead of an empty command, which `do_step` would skip in silence.
fn batch_action(
    a: Option<crate::decision::Action>,
    can_downgrade: bool,
) -> Result<Option<crate::decision::Action>, &'static str> {
    use crate::decision::Action;
    match a {
        Some(Action::Downgrade) if !can_downgrade => Err(NO_DOWNGRADE_ROUTE),
        other => Ok(other),
    }
}

/// The action a 403 retry really runs. A downgrade is `uninstall && install@pin`; when its
/// reinstall half failed, the package is already gone, and re-running the chain fails at
/// once on the uninstall — the download is never attempted again. So a retried downgrade
/// asks the machine first: absent means install at the pin. Unknown is NOT absent.
fn retry_action(asked: crate::decision::Action, present: Option<bool>) -> crate::decision::Action {
    use crate::decision::Action;
    match (asked, present) {
        (Action::Downgrade, Some(false)) => Action::Install,
        (a, _) => a,
    }
}

/// True when this row is OUT OF SCOPE by the user's explicit hand, so no action may
/// run on it. Reads the persisted overrides only: the DERIVED half (!canUninstall /
/// external) is the front's to compute and it already declines to offer the action,
/// so refusing on it here would break every ordinary row (the map is sparse — an
/// absent entry means "follow the derivation", not "out").
///
/// WHY the server checks at all, when the front already filters: `retry_step` is a
/// SEPARATE path that never sees the `on`/`off` lists (it was the per-row button's
/// path until 2026-10-10, and a 403 retry still takes it). Git declares `brew: git`, so `canUninstall` is true and the
/// button would happily run `brew uninstall git` on an Xcode-CLT binary. A
/// front-only guard is a cosmetic guard (talos-measure-from-the-ui-not-the-socket).
fn scope_refuses(sel: &Selection, name: &str) -> bool {
    sel.scope.get(name).map(|s| s.as_str()) == Some("out")
}

/// True when this client text is a `cancel-step` aimed at the step currently running.
///
/// The index check is the whole point. A Stop click can land JUST as its step finishes —
/// the message is then read while the NEXT step is streaming, and killing on the verb
/// alone would murder an innocent row. Pure, so it is tested without a pty.
fn cancel_targets(txt: &str, running: u32) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(txt) else {
        return false;
    };
    v.get("type").and_then(|t| t.as_str()) == Some("cancel-step")
        && v.get("i").and_then(|n| n.as_u64()) == Some(running as u64)
}

/// Does this row declare `version: self` — the application updates itself?
fn is_self_updating(step: &crate::bundles::Step) -> bool {
    crate::bundles::classify_pin(step.pin.as_deref()) == Some(crate::bundles::PinKind::SelfUpdating)
}

/// Would this present row still act toward what it last ran for, so a remembered hold is worth
/// re-sending to the front? Toward an exact pin, yes while the pin is declared; otherwise only
/// while the manager still lists it behind — a row it no longer lists does nothing, and a hold
/// re-sent there made the front fabricate an outdated row.
fn hold_still_in_view(step: &crate::bundles::Step, od: Option<&crate::managers::Outdated>) -> bool {
    matches!(
        crate::bundles::classify_pin(step.pin.as_deref()),
        Some(crate::bundles::PinKind::Exact(_))
    ) || is_outdated_now(od)
}

/// Is this row re-observed before every Apply, wanted present and already there? Yes when
/// presence alone cannot say there is nothing to do: an exact pin (the VERSION decides), and
/// a tool whose own update runs at each Apply (it acts whenever present, so it must be seen
/// first — and once done for the session, a removal since must still be seen live).
fn always_probed(step: &crate::bundles::Step) -> bool {
    matches!(
        crate::bundles::classify_pin(step.pin.as_deref()),
        Some(crate::bundles::PinKind::Exact(_))
    ) || (step.update_each_apply && step.upgrade.is_some())
}

/// Is this step judged by the version it leaves (`unmoved_reason`)? Not a tool whose own update
/// runs at each Apply: "nothing to do" is its usual answer, and a success.
fn judges_version(step: &crate::bundles::Step, action: crate::decision::Action) -> bool {
    !(action == crate::decision::Action::Upgrade
        && is_self_updating(step)
        && step.update_each_apply)
}

/// Does this step, run to `ok`, make its row done for the rest of the session? Only for a tool
/// whose own update runs at each Apply until it succeeds: its exit code is the verdict
/// (`judges_version` exempts it), so a success is an exit 0 not cancelled, and a failure leaves
/// the row armed. An INSTALL arms it too — the manager just put the current build there, and a
/// row left offering its update would end the very run that installed it pending again.
fn arms_session_done(
    step: &crate::bundles::Step,
    action: crate::decision::Action,
    ok: bool,
) -> bool {
    use crate::decision::Action;
    ok && matches!(action, Action::Upgrade | Action::Install)
        && is_self_updating(step)
        && step.upgrade.is_some()
        && step.update_each_apply
}

/// Is this row done for the session — its own update exited ok, or the Apply installed it
/// (`arms_session_done`)?
fn updated_this_session(state: &AppState, step: &crate::bundles::Step) -> bool {
    state
        .updated_this_session
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&step.id)
}

fn is_outdated_now(od: Option<&crate::managers::Outdated>) -> bool {
    // Approach B: presence in the scan IS the outdated signal, for casks and
    // formulae alike. The scan uses `--greedy-auto-updates`, so a self-updating
    // cask that brew lists as behind gets a forced upgrade rather than being
    // silently left unmanaged. brew's receipt may lag the disk (so this can flag
    // an already-current cask), but Apply forces + reconciles, and it converges:
    // once the receipt catches up, the cask drops out of the scan.
    od.is_some()
}

/// For an Upgrade on a brew CASK, the forced command (`brew upgrade --cask
/// --force --yes`) that absorbs the receipt drift (else brew's anti-clobber
/// guard → exit 1). Returns None for formulae, non-brew routes, and steps with
/// an EXACT pin (an exact pin owns the direction — keep its precomputed
/// install-pinned cmd). Keywords (pending, latest) do NOT own a direction, so
/// they still need the forced command.
/// `is_cask` comes from the brew outdated scan bucket (the single source).
fn forced_cask_upgrade_cmd(
    step: &crate::bundles::Step,
    os: crate::platform::Os,
    is_cask: bool,
) -> Option<String> {
    use crate::bundles::{classify_pin, PinKind};
    use crate::managers::{native_manager, IdField};
    // An EXACT pin owns the direction (install-pinned command), so return None.
    // Keywords (pending, latest) do not own a direction, so fall through. `self` does not
    // either, but its upgrade is the tool's own `update:`, which brew must never replace.
    if !is_cask
        || matches!(
            classify_pin(step.pin.as_deref()),
            Some(PinKind::Exact(_)) | Some(PinKind::SelfUpdating)
        )
    {
        return None;
    }
    let mgr = native_manager(os)?;
    if mgr.route != "brew" || mgr.id_field != IdField::Brew {
        return None;
    }
    let id = step.system_id.as_deref()?;
    Some(mgr.upgrade(id, true))
}

/// Emit the `outdated` pill for package `i` when the greedy scan lists it
/// (is_outdated_now). Both the connect scan and the apply repaint route through
/// here so the two can't drift.
async fn emit_outdated_if(
    socket: &mut WebSocket,
    i: usize,
    od: Option<&crate::managers::Outdated>,
) {
    if let Some(od) = od.filter(|o| is_outdated_now(Some(o))) {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "outdated", "i": i, "current": od.current, "available": od.available }).to_string(),
            ))
            .await;
    }
}

/// Tell the front a row's last version change moved nothing, so it holds the row (and says
/// why) while the same version is installed and the same target in view — the rule the Apply
/// applies (`held_unmoved`).
async fn emit_unmoved(socket: &mut WebSocket, i: usize, u: &Unmoved) {
    let _ = socket
        .send(Message::Text(
            json!({
                "type": "unmoved", "i": i,
                "found": u.found, "target": u.target, "reason": u.reason, "ahead": u.ahead,
            })
            .to_string(),
        ))
        .await;
}

/// The row's held change, when the session remembers one for the version installed now and
/// the target now in view.
async fn unmoved_in_view(
    state: &AppState,
    step: &crate::bundles::Step,
    action: crate::decision::Action,
    installed: Option<&str>,
    announced: Option<&str>,
) -> Option<Unmoved> {
    let memory = state.unmoved.lock().await;
    hold_for(&memory, step, action, installed, announced)
}

/// The remembered change that holds this step, if any: only for a step judged by the version it
/// leaves (`judges_version`, the predicate the recording uses), from the version installed now
/// toward the target now in view.
fn hold_for(
    memory: &std::collections::HashMap<String, Unmoved>,
    step: &crate::bundles::Step,
    action: crate::decision::Action,
    installed: Option<&str>,
    announced: Option<&str>,
) -> Option<Unmoved> {
    if !judges_version(step, action) {
        return None;
    }
    let target = step_target(step.pin.as_deref(), announced).unwrap_or_default();
    let cmd = change_command(step);
    held_unmoved(memory, &step.id, installed.unwrap_or(""), &target, &cmd)
        .then(|| memory.get(&step.id).cloned())
        .flatten()
}

/// The server's shared state: the Plan read from disk (pure data, no pty/network),
/// plus the current OS. Cloned (Arc) into each connection.
struct AppState {
    /// The catalogue, re-read on every Refresh — a `RwLock` because it is read by
    /// every scan and every action, and replaced only by `rescan`.
    ///
    /// ⭐ It used to be read ONCE at startup, so editing `catalog/` while Talos ran
    /// changed nothing until a relaunch: a new yaml stayed invisible, a deleted one
    /// stayed on screen, an edited command kept acting on its old value. That is the
    /// wrong default for a tool whose whole doctrine is "detect, don't remember" — the
    /// catalogue is an observation of the disk exactly as presence is an observation of
    /// the machine.
    ///
    /// ⚠️ Replaced ONLY on `rescan`, never mid-Apply. `server.rs`'s startup note already
    /// stated the rule for behaviour facts — neither the plan nor the facts may shift
    /// under a running Apply — and it holds here for a harder reason: `last_seen` is a
    /// POSITIONAL Vec and the front addresses rows by index, so a plan that grew or
    /// shrank between the decision and the act would point a gesture at the wrong
    /// package. What makes that safe is not a lock but the sequence: messages on a
    /// connection are handled one at a time, so `rescan` cannot interleave with `apply`.
    /// The front also disables Refresh while `applyRunning || scanning`.
    plan: tokio::sync::RwLock<Plan>,
    /// Where the plan came from (content.rs): the two folders, the kit root, and whether the
    /// kit's archive is the one being read. Re-resolved on Refresh, because with an archive
    /// the LOCATION changes when a new version lands — a new extraction folder — not only
    /// the contents. Without an archive it resolves to the same loose folders every time.
    content: std::sync::Mutex<crate::content::Content>,
    profiles: Profiles,
    os: Os,
    /// macOS App Management permission status, probed ONCE at startup. Not
    /// re-checked in-process: macOS caches the TCC verdict per process, so the
    /// truthful signal after a grant is a quit-and-relaunch (which re-runs this
    /// probe). Wire string via AppMgmtStatus::as_str().
    appmgmt: crate::platform::AppMgmtStatus,
    /// Disk root of the front assets, IN DEV ONLY (`TALOS_PUBLIC` set):
    /// lets you edit `app.js` without recompiling. `None` in release → assets SEALED
    /// in the binary (crate::assets), independent of the cwd (shortcut #3 fixed).
    disk_root: Option<PathBuf>,
    /// Per-machine LOCAL data-dir — where selection/consent/history are persisted
    /// (NEVER the shared exe folder). INTENT is remembered, PRESENCE is
    /// re-detected: "detect, don't remember" does NOT govern intent.
    data_dir: PathBuf,
    /// The consent store + install journal (paths/identity injected).
    consent: ConsentStore,
    /// Sudo password cache — model C: asked the 1st time a step needs it,
    /// reused for the following steps of the same Apply, CLEARED at the end.
    /// RAM ONLY, never disk/log/journal. tokio Mutex (async access).
    sudo_pw: tokio::sync::Mutex<Option<String>>,
    // The Doctor holds NO state here on purpose. Its terminal lives on its own
    // socket (`/doctor`, see `handle_doctor_socket`), so its child, its stdin channel and
    // its pending bytes are locals of that handler. Nothing about the rescue can be
    // reached — or starved — by the step machinery that owns `/`.
    /// LAST OBSERVED presence per package, written by every scan (connect, Refresh,
    /// Apply re-scan), parallel to `plan.steps`. `None` = never observed.
    ///
    /// This is NOT a cache that spares a probe (a TTL cache was rejected — memory
    /// repaint-at-apply). It exists because the Apply re-scan is SCOPED to the diff:
    /// the rows it doesn't touch still need a presence to feed `will_be_present`, or
    /// every `requires` reason over them would lie. Remembering the last OBSERVATION
    /// is honest; inventing `false` is not.
    last_seen: tokio::sync::Mutex<Vec<Option<crate::detect::Presence>>>,
    /// The last READABLE machine-wide outdated map, and when it was read — so an Apply
    /// shortly after a scan does not run `winget upgrade` again. See `OutdatedSeen` for why
    /// this is not the presence cache the doctrine forbids. `None` = nothing fresh to reuse.
    outdated_seen: tokio::sync::Mutex<Option<OutdatedSeen>>,
    /// Behaviour facts observed during the CURRENT Apply, keyed by package id then
    /// `route/os`. Flushed to the share ONCE when the Apply ends, for the packages it
    /// touched only — a per-step write would be 30 chances to collide with another
    /// machine on a synchronised folder, and a cancelled Apply would leave half-formed
    /// facts behind.
    observed: tokio::sync::Mutex<std::collections::BTreeMap<String, crate::behaviour::Record>>,
    /// What the app BELIEVES about each step — the fleet's observed facts with the
    /// catalogue's declarations laid over them — index for index with `plan.steps`, like
    /// `last_seen`.
    ///
    /// Read ONCE at startup and never again, so a behaviour file that lands mid-session is
    /// not seen until the next launch. That is the mirror of the write side's "flush once
    /// at the end": neither the plan nor the facts may shift under a running Apply.
    ///
    /// Not a Mutex, unlike `observed` beside it, because nothing mutates this one.
    /// Immutable-after-boot is the property, and the type is what states it.
    ///
    /// ⚠️ These are BELIEFS of the same `Facts` type an observation has, so handing one to
    /// `remember_behaviour`/`flush_behaviour` would compile and would ratchet a declared
    /// `slow`'s sentinel onto the share permanently. `observed` is the only thing that may
    /// travel that way. Nothing enforces the separation; these two fields being neighbours
    /// is the whole reason it is said here.
    facts: Vec<crate::behaviour::Facts>,
    /// This machine's LAST-SEEN duration per (package id, `route/os`), read ONCE at startup
    /// — the snapshot the front is told about so it can show minutes. Immutable after boot,
    /// on the same clock as `facts`: one mental model, and nothing moves under a running
    /// Apply. An improved estimate appears at the NEXT launch (see the deferred table).
    timings: crate::timings::Timings,
    /// Durations measured during the CURRENT Apply, flushed beside `observed`.
    ///
    /// A SECOND accumulator rather than a reading of `observed`, deliberately: `observed`
    /// ratchets to a MAX within one Apply (a 403 + Retry runs the same step twice), and
    /// this file wants the LAST run. Same site, different semantics.
    timed: tokio::sync::Mutex<crate::timings::Timings>,
    /// Lines waiting for a row's terminal, written by do_step right AFTER the running status
    /// — the front resets the terminal on that status, so a line sent earlier is wiped.
    notes: tokio::sync::Mutex<std::collections::HashMap<usize, Vec<String>>>,
    /// Where a winget row's installed copy was found, by the probe before a version change.
    /// A fact read minutes ago in this session, kept so a retry that finds the package gone
    /// reinstalls it there; never written to disk.
    found_scope:
        tokio::sync::Mutex<std::collections::HashMap<String, crate::managers::InstallScope>>,
    /// This launch's machine profile, None while it is being collected. Observed once per
    /// launch, after the first scan (see `machine::collect`), and shown at the top of the
    /// Machine tab — the very values the journal received, never a description of them.
    machine: std::sync::Mutex<Option<crate::machine::Profile>>,
    /// Set by the first scan, so the profile is collected once per launch and a Refresh
    /// does not collect it again.
    machine_started: std::sync::atomic::AtomicBool,
    /// True while the profile is being collected — a scan that overlaps it says so.
    machine_running: std::sync::atomic::AtomicBool,
    /// This launch's profile already reached the shared log — by its own write, or by a
    /// consent turned on later. Keeps the share from receiving it twice.
    machine_shared: std::sync::atomic::AtomicBool,
    /// The last scan's timings, as journalled — for the Machine tab's one-line summary.
    last_scan: std::sync::Mutex<Option<crate::machine::ScanRecord>>,
    /// The version changes that exited ok and moved nothing, by package id (`Unmoved`). The
    /// Apply holds the row while the same version is installed and the same target in view;
    /// a change of either lifts it. Never written: a restart tries once more.
    unmoved: tokio::sync::Mutex<std::collections::HashMap<String, Unmoved>>,
    /// What brew offers for a pinned row, read when the Apply builds its plan and taken by the
    /// row's step, so one Apply asks brew once. A step that finds nothing here (a retry, a
    /// repair) asks brew itself.
    /// `None` = brew was asked and could not tell: kept, so the step does not ask again.
    /// Cleared when the Apply ends, so a later retry never acts on an old answer.
    brew_pins:
        tokio::sync::Mutex<std::collections::HashMap<String, Option<crate::managers::BrewPin>>>,
    /// The rows whose own `update:` runs at each Apply (`update-on: apply`) and is done for this
    /// session — it exited ok, or the Apply installed the row — by package id
    /// (`arms_session_done`). `action_for` reads it:
    /// such a row is done until the Refresh button re-arms it, so the end of a run can say the
    /// machine already matches. Never written: a restart re-arms it too. A std Mutex, never
    /// held across an await — `plan_message`, which is not async, reads it.
    updated_this_session: std::sync::Mutex<std::collections::HashSet<String>>,
}

/// Records one observed presence in `last_seen` (grows the vector if the plan is
/// somehow longer than it was at startup — never panics on an index).
async fn remember_presence(state: &AppState, i: usize, p: crate::detect::Presence) {
    let mut seen = state.last_seen.lock().await;
    if i >= seen.len() {
        seen.resize(i + 1, None);
    }
    seen[i] = Some(p);
}

/// Records what ONE step observed, into the in-memory accumulator. Nothing touches the
/// share here — see `flush_behaviour`, called once when the Apply ends.
///
/// `id` is the catalogue id (the yaml file's stem), which is what names the behaviour
/// file, so `behaviour/<id>.yaml` sits beside `catalog/<id>.yaml`.
async fn remember_behaviour(
    state: &AppState,
    id: &str,
    route: &str,
    facts: crate::behaviour::Facts,
) {
    let k = crate::behaviour::key(route, state.os);
    let mut obs = state.observed.lock().await;
    let rec = obs.entry(id.to_string()).or_default();
    crate::behaviour::merge_into(rec, &k, &facts);
}

/// Records how long ONE step took, for the LOCAL duration cache. Nothing touches disk here.
///
/// ⚠️ ASSIGNMENT, not a ratchet. This is the local file's semantics and it is the OPPOSITE
/// of `remember_behaviour`'s, just above — that one merges monotonically because many
/// machines write the shared file; this one has a single writer and wants the LAST run.
/// Reaching for `merge_into` here would freeze one unlucky slow run as this machine's
/// estimate forever.
async fn remember_duration(state: &AppState, id: &str, route: &str, secs: u64) {
    let at = crate::behaviour::key(route, state.os);
    let mut t = state.timed.lock().await;
    crate::timings::note_duration(&mut t, id, &at, secs);
}

/// Writes everything the Apply observed to the share, then clears the accumulator.
/// Best-effort by construction (behaviour_io swallows IO errors): an offline share
/// costs us the update, never the Apply.
///
/// It flushes TWO accumulators, despite the name: the shared behaviour facts, and the local
/// duration cache (`timed` → `timings.yaml`). One ending, because both are measured by the
/// same steps and both want the same "once, at the end" discipline — a cancelled Apply must
/// leave neither half-formed. Everything else about them differs: different file, different
/// folder, different merge rule (monotone max for the share, assignment for the local one).
/// The name is the older half's; it is kept because the two call sites read as "the Apply is
/// over" and splitting them would only make a caller choose.
///
/// ⚠️ The two halves are SEQUENTIAL, not nested, and that is deliberate on two counts. It
/// keeps `observed`'s early-out from gating the local write (they happen to be non-empty
/// together today — both are filled at the same site in `do_step` — but that is a coupling
/// nothing enforces, and a future caller of `remember_duration` alone would silently lose
/// its measurement). And it means no code path ever holds both locks, so the pair cannot
/// deadlock however a later site orders them.
///
/// Clearing matters as much as writing: the state outlives one Apply, and a second Apply
/// in the same session must not re-write the first one's facts — the ratchet makes that
/// harmless to the VALUES, but it is a write to files this Apply never touched, which is
/// exactly the collision window one-file-per-package exists to narrow.
///
/// The empty guard below buys ONLY the log line: an empty map writes nothing either way
/// (measured — removing the guard breaks no test and creates no file), so it is there to
/// avoid printing "flushed 0 package(s)" after every Apply that did nothing.
///
/// ⚠️ A package whose entry has NO facts is still written, as `route/os: {}`. That is not
/// a loss and not a bug to diagnose: it means "we acted on this and nothing was notable —
/// no elevation, no 403, under a second". Skipping such records would need a rule about
/// what counts as empty, and that rule would have to be revisited every time a fourth
/// fact lands with a meaningful default.
async fn flush_behaviour(state: &AppState) {
    {
        let mut obs = state.observed.lock().await;
        if !obs.is_empty() {
            for (id, rec) in obs.iter() {
                crate::behaviour_io::merge_and_write(&state.consent.exe_dir, id, rec);
            }
            // "flushed", not "wrote": `write_one` swallows every IO error, so on an offline
            // or read-only share this line prints after writing nothing at all. Claiming
            // the write succeeded would make the log lie exactly where someone is debugging
            // a missing file.
            println!("[behaviour] flushed facts for {} package(s)", obs.len());
            obs.clear();
        }
    }
    // The LOCAL cache, on the same ending and with the same clear-after-write discipline.
    // Its own scope, its own emptiness check — see the ⚠️ above.
    let mut fresh = state.timed.lock().await;
    if !fresh.is_empty() {
        crate::timings::merge_and_write_timings(&state.data_dir, &fresh);
        // Same honesty as the line above: `write_timings` swallows its IO errors too.
        println!("[timings] flushed durations for {} package(s)", fresh.len());
        fresh.clear();
    }
}

/// The presence vector the Apply reasons over: the FRESH probe where we have one,
/// otherwise the last observed verdict, otherwise unknown (`present: None`).
///
/// Un-probed rows must NOT read as absent. `will_be_present` feeds requires_reason
/// and topo_sort: a `false` invented for a row nobody looked at would print
/// "requires jj" on a machine that has jj, and could reorder the plan around a
/// requirement that is in fact satisfied.
fn merge_presences(
    n: usize,
    probed: &std::collections::HashMap<usize, crate::detect::Presence>,
    remembered: &[Option<crate::detect::Presence>],
) -> Vec<crate::detect::Presence> {
    (0..n)
        .map(|i| {
            probed
                .get(&i)
                .cloned()
                .or_else(|| remembered.get(i).cloned().flatten())
                .unwrap_or_default() // never seen → present: None (unknown, not absent)
        })
        .collect()
}

/// Starts the HTTP+WS server on 127.0.0.1:1420.
/// - `/`         → serves index.html (or WS upgrade if the Upgrade header is present)
/// - others      → front assets (SEALED in the binary, or disk in dev)
///
/// `disk_root`: `Some(dir)` in dev (`TALOS_PUBLIC` set) to edit the front without
/// recompiling; `None` in release → everything comes from the sealed assets (crate::assets),
/// so the `.app`/`.exe` launched by Finder/Explorer always finds its front.
///
/// `ready`: signal fired AS SOON AS the port is actually listening (bind succeeded), so
/// the webview loads the URL WITHOUT a race (shortcut #4: no more blind sleep(500ms) —
/// a slow bind, shared disk/slow machine, left the webview hitting nothing).
pub async fn serve(disk_root: Option<PathBuf>, ready: Option<tokio::sync::oneshot::Sender<()>>) {
    let os = current_os();
    // Build stamp at the top of the log — "which binary is really running?".
    println!("--- start: {}", crate::build_info::start_line());
    println!(
        "--- appmgmt: {}",
        crate::platform::app_management_status(os).as_str()
    );
    // Where the content is read from: the kit's archive, extracted locally, when it has one;
    // else loose folders next to the exe, else the cwd (DEV). One rule for the server, the
    // title and `--check` — see content::resolve.
    let sibling_dir = crate::platform::exe_sibling();
    // Off the async runtime: a resolve reads the share (a placeholder may hydrate over a slow
    // link), hashes, and may extract.
    let content = tokio::task::spawn_blocking(crate::content::resolve)
        .await
        .expect("the content resolve does not panic");
    crate::content::publish(&content);
    println!("{}", content.log_line());
    println!("[bundles] dir: {}", content.bundles.display());
    println!("[catalog] dir: {}", content.catalog.display());
    let plan = plan_from(&content, os);
    let bundles_dir = content.bundles.clone();
    println!("[plan] {} steps", plan.steps.len());
    // The fleet's behaviour facts, read ONCE here beside the catalogue — and never again,
    // so a behaviour file landing mid-session is not seen until the next launch. That is
    // the mirror of the write side, which accumulates in memory and flushes once at the end:
    // neither the plan nor the facts may shift under a running Apply.
    //
    // `TALOS_BEHAVIOUR` is a TEST MODE, not a deployment knob: it points the READ at any
    // folder of `<id>.yaml` files, so a fact can be exercised without installing anything —
    // which is the only way the `uac` label is verifiable on macOS at all, since watch.rs has
    // no macOS implementation and the signal cannot be OBSERVED here, only fabricated. Same
    // shape as `TALOS_PUBLIC`: read where the paths are already resolved and PASSED DOWN, so
    // behaviour_io stays ignorant of environment variables.
    //
    // ⚠️ It moves the READ only. An Apply still FLUSHES to the real share, so pointing this
    // at a folder of versioned fixtures leaves that folder untouched — which is what will
    // let the fixtures of a later task be facts rather than a scratch directory.
    let behaviour_override = std::env::var_os("TALOS_BEHAVIOUR").map(PathBuf::from);
    let collected = match &behaviour_override {
        Some(dir) => {
            println!(
                "[behaviour] TEST MODE, reading fixtures from {}",
                dir.display()
            );
            crate::behaviour_io::read_all_from(dir)
        }
        None => crate::behaviour_io::read_all(&sibling_dir),
    };
    let facts = crate::plan_facts::resolve_plan_facts(&plan.steps, &collected, os);
    // The second number is NOT `facts.len()` (which is just the step count): it is how many
    // rows actually believe something, collected or declared. That is the number that says
    // whether the read landed — a share whose folder is missing prints `0 of 31`.
    println!(
        "[behaviour] {} package(s) with collected facts; {} of {} step(s) know something",
        collected.len(),
        facts
            .iter()
            .filter(|f| **f != crate::behaviour::Facts::default())
            .count(),
        facts.len()
    );
    // The top-panel "needs" (profiles.yaml lives INSIDE the same bundles dir; the
    // bundle scan skips it because it only reads subfolders with a bundle.yaml).
    let profiles = load_profiles(bundles_dir.to_str().unwrap_or("bundles"));
    println!("[plan] {} profiles", profiles.items.len());
    // Per-machine local data-dir + consent store. exe_dir = the SAME folder
    // next to the exe (the one that contains bundles/) — that's where the consented
    // shared copy lands. host/user name the shared log; env best-effort.
    let data_dir = local_data_dir(os);
    // The local duration cache. A derived cache of what the journal already records, kept
    // because a bounded keyed read at startup beats walking an append-only journal
    // backwards. Deleting it is harmless. LOCAL dir, never `exe_dir`: the share is read by
    // N machines and this number is about THIS one.
    let timings = crate::timings::read_timings(&data_dir);
    println!(
        "[timings] {} package(s) with a local duration",
        timings.len()
    );
    let exe_dir = sibling_dir;
    let consent = ConsentStore {
        local_dir: data_dir.clone(),
        exe_dir,
        host: std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "host".into()),
        user: std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".into()),
    };
    let last_seen = tokio::sync::Mutex::new(vec![None; plan.steps.len()]);
    let state = Arc::new(AppState {
        plan: tokio::sync::RwLock::new(plan),
        content: std::sync::Mutex::new(content),
        profiles,
        os,
        last_seen,
        outdated_seen: tokio::sync::Mutex::new(None),
        appmgmt: crate::platform::app_management_status(os),
        disk_root,
        data_dir,
        consent,
        sudo_pw: tokio::sync::Mutex::new(None),
        observed: tokio::sync::Mutex::new(std::collections::BTreeMap::new()),
        facts,
        timings,
        timed: tokio::sync::Mutex::new(crate::timings::Timings::new()),
        notes: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        found_scope: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        machine: std::sync::Mutex::new(None),
        machine_started: std::sync::atomic::AtomicBool::new(false),
        machine_running: std::sync::atomic::AtomicBool::new(false),
        machine_shared: std::sync::atomic::AtomicBool::new(false),
        last_scan: std::sync::Mutex::new(None),
        unmoved: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        brew_pins: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        updated_this_session: std::sync::Mutex::new(std::collections::HashSet::new()),
    });

    let state_for_root = state.clone();
    let state_for_asset = state.clone();
    let app = Router::new()
        .route(
            "/",
            get(move |ws: Option<WebSocketUpgrade>| {
                let state = state_for_root.clone();
                async move { root_or_ws(ws, state).await }
            }),
        )
        // The Doctor's OWN socket. See `handle_doctor_socket` for why it is a
        // second route and not a verb on `/`.
        .route(
            "/doctor",
            get({
                // The handler reads the plan (which packages declare `doctor:`) and the
                // data dir (where a clean config dir is created — per machine, never
                // beside a shared exe, the same rule selection and consent follow).
                let state = state.clone();
                move |ws: WebSocketUpgrade| {
                    let state = state.clone();
                    async move { ws.on_upgrade(move |s| handle_doctor_socket(s, state)) }
                }
            }),
        )
        .fallback(get(move |uri: Uri| {
            let state = state_for_asset.clone();
            async move { serve_asset(uri.path(), &state) }
        }))
        // ANTI-CACHE: the webview (WKWebView macOS / WebView2 Windows) keeps app.js
        // in disk cache between two launches → an old app.js without the latest
        // handler (e.g. sudo-prompt) survived rebuilds → the message arrived but
        // fell into the switch default, without error. no-store forces the fresh one.
        .layer(axum::middleware::from_fn(no_cache));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:1420")
        .await
        .expect("bind 127.0.0.1:1420 failed");
    // The port is listening NOW — signal the main thread that it can load
    // the URL in the webview (no more race: the webview waits for this exact signal).
    if let Some(ready) = ready {
        let _ = ready.send(());
    }
    axum::serve(listener, app).await.expect("axum serve failed");
}

/// Serves a front asset (sealed or disk), with a Content-Type inferred from the extension.
/// Asset absent → 404. Replaces `ServeDir` (which read a disk path relative to the cwd).
fn serve_asset(path: &str, state: &AppState) -> Response {
    match crate::assets::resolve(state.disk_root.as_deref(), path) {
        Some(bytes) => {
            // Mime on the normalized key: `/` → `index.html` → text/html (not octet-stream).
            let mime = mime_for(&crate::assets::normalize(path));
            (
                [(CONTENT_TYPE, HeaderValue::from_static(mime))],
                Body::from(bytes.into_owned()),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Minimal Content-Type per extension (the only types served by the Talos front).
fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Middleware: adds `Cache-Control: no-store` to every response, so the webview
/// never serves a stale app.js/index.html after a rebuild (cause of the sudo modal
/// that "didn't show" in Tauri while it worked in a freshly reloaded Chrome).
async fn no_cache(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

// app.js does `new WebSocket(ws://location.host)` → root path "/". We tell
// a WS upgrade apart from a normal HTML request by the presence of the Upgrade header
// (upgrade on the Upgrade header, not on a fixed pathname).
async fn root_or_ws(ws: Option<WebSocketUpgrade>, state: Arc<AppState>) -> Response {
    match ws {
        Some(ws) => ws.on_upgrade(move |socket| handle_socket(socket, state)),
        // index.html comes from the sealed assets (or from disk in dev) — no more
        // read_to_string on a path relative to the cwd (shortcut #3 fixed).
        None => serve_asset("/", &state),
    }
}

/// Finds an executable on PATH and returns its ABSOLUTE path — without a shell.
///
/// ⭐ `which` / `where` would need one, and only `platform::shell_probe` / `pty_shell` may
/// build a shell line. Splitting PATH ourselves keeps that rule intact, and the absolute
/// path it yields is exactly what lets the Doctor start a binary while the operator's rc
/// file is broken.
fn on_path(name: &str) -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(&exe))
        .find(|p| p.is_file())
}

/// One line to the Doctor's terminal — the refusals and the "nothing to launch" cases.
async fn doctor_line(socket: &mut WebSocket, text: &str) {
    let _ = socket
        .send(Message::Text(
            json!({ "type": "doctor-out", "d": format!("\r\n{text}\r\n") }).to_string(),
        ))
        .await;
}

/// A rescue candidate as the Doctor sees it: a catalogue package with a `doctor:` block.
/// `word` is the binary to resolve on PATH — the first word of `detect:` — resolved at
/// launch, not stored, so presence is what the machine answers now.
struct DoctorCandidate {
    name: String,
    id: String,
    word: String,
    clean: Option<crate::bundles::CleanLaunch>,
}

/// The packages that declare `doctor:` AND say how to detect themselves, in catalogue
/// order. A `doctor:` without a `detect:` has no binary to launch and is not a candidate
/// (`--check` refuses it before it gets here). PURE over the steps → tested without PATH.
fn doctor_candidates(steps: &[crate::bundles::Step]) -> Vec<DoctorCandidate> {
    steps
        .iter()
        .filter_map(|s| {
            let decl = s.doctor.as_ref()?;
            let word = s.detect.as_deref()?.split_whitespace().next()?;
            Some(DoctorCandidate {
                name: s.name.clone(),
                id: s.id.clone(),
                word: word.to_string(),
                clean: decl.clean.clone(),
            })
        })
        .collect()
}

/// The Doctor terminal — an interactive rescue session, on a socket of its OWN.
///
/// ⭐ WHY IT EXISTS. Field incident: a broken Claude configuration locked the operator out
/// for a week. Everything needed to recover was on the machine; what was missing was a way
/// to reach it that did not go through the thing that was broken. So this terminal starts
/// the binary by ABSOLUTE PATH with no shell (a broken rc file cannot stop it) and, on
/// request, with a CLEAN config directory (no plugins, no hooks, no inherited settings).
///
/// ⭐ WHY A SECOND ROUTE AND NOT A VERB ON `/`. The step machinery holds `/`'s socket
/// exclusively while a row runs — that is how `obtain_sudo_pw` reads its reply off the
/// same socket. Sharing it would make Apply and the rescue mutually exclusive, and
/// exactly in the case that matters most: an Apply WEDGED on a prompt is when you most
/// want to open the Doctor, and is precisely when it would be unavailable.
///
/// So the isolation is not a workaround, it is the scope statement made structural: the
/// Doctor is NOT a step (that is why display-only does not govern it), and the socket is
/// part of the steps' machinery. A non-step does not share it. The rescue therefore
/// cannot be starved by an Apply, and — the half that matters more — cannot disturb one.
///
/// No scan, no sudo here. It reads the plan for ONE thing — which packages declare
/// `doctor:` — and the data dir for one thing: where a CLEAN session's config lives.
///
/// ⭐ THE CANDIDATES COME FROM THE CATALOGUE, NOT FROM THE CODE. `on_path("claude")` used
/// to be a coded choice, marked as a residue: Claude is a catalogue package, not an engine
/// dependency, and the hermeticity rule says the rescue command must be DECLARED. Now a
/// package with a `doctor:` block is a candidate, its binary is the first word of its
/// `detect:`, and presence is re-resolved on PATH at every launch — detect, don't remember.
/// Below every candidate sits the FLOOR: the OS shell without a profile
/// (`platform::rescue_shell`), for the machine where the agent never got installed.
async fn handle_doctor_socket(mut socket: WebSocket, state: Arc<AppState>) {
    type Killer = Box<dyn portable_pty::ChildKiller + Send + Sync>;
    /// One live child. Everything here has the child's lifetime: stdin, the pty geometry,
    /// the killer, and the UTF-8 tail a pty read may split (emit only the valid prefix and
    /// carry the rest to the next chunk, or an accented glyph arrives mangled).
    struct Session {
        input: std::sync::mpsc::Sender<Vec<u8>>,
        resize: std::sync::mpsc::Sender<(u16, u16)>,
        // ⭐ The killer, kept rather than dropped. `pty::run` blocks, so it can only hand
        // this out DURING the call. Without it, closing a tab left the child running with
        // nothing reading it. A rescue you cannot stop is a trap of its own. A plain
        // `Mutex`: held for one `kill()` syscall, never across an await.
        killer: std::sync::Arc<std::sync::Mutex<Option<Killer>>>,
        pending: Vec<u8>,
    }
    /// What a child's reader thread reports back, tagged with its session.
    enum Event {
        Out(Vec<u8>),
        Exit(i32),
    }
    // ⭐ SESSIONS, plural, on ONE socket. The front opens a tab per Launch — two agents side
    // by side, a clean one next to a normal one, a shell beside the agent that works — and
    // each is a session with its own child, addressed by the `id` the front minted. One
    // socket because the isolation that matters is from Apply's socket, not between
    // rescues; one channel for every child's output because `select!` wants one thing
    // to await, and the tag says which tab.
    let mut sessions: std::collections::HashMap<u64, Session> = std::collections::HashMap::new();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Event)>();
    let kill = |sess: &Session| {
        if let Ok(mut slot) = sess.killer.lock() {
            if let Some(mut k) = slot.take() {
                let _ = k.kill();
            }
        }
    };
    // What the dropdown offers: every declared candidate with its presence NOW, and the
    // floor. Sent once per socket; presence is resolved again at launch, so a package
    // installed while the tab was open still launches.
    let candidates = {
        let plan = state.plan.read().await;
        doctor_candidates(&plan.steps)
    };
    let shell_name = crate::platform::rescue_shell(state.os)
        .and_then(|(p, _)| p.file_name().map(|n| n.to_string_lossy().to_string()));
    let agents: Vec<serde_json::Value> = candidates
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "present": on_path(&c.word).is_some(),
                "clean": c.clean.is_some(),
            })
        })
        .collect();
    let _ = socket
        .send(Message::Text(
            json!({ "type": "doctor-agents", "agents": agents, "shell": shell_name }).to_string(),
        ))
        .await;
    loop {
        tokio::select! {
            Some((id, ev)) = out_rx.recv() => {
                match ev {
                    Event::Out(bytes) => {
                        let Some(sess) = sessions.get_mut(&id) else { continue };
                        sess.pending.extend_from_slice(&bytes);
                        let good = match std::str::from_utf8(&sess.pending) {
                            Ok(_) => sess.pending.len(),
                            Err(e) => e.valid_up_to(),
                        };
                        if good > 0 {
                            let text = String::from_utf8_lossy(&sess.pending[..good]).to_string();
                            sess.pending.drain(..good);
                            let _ = socket
                                .send(Message::Text(
                                    json!({ "type": "doctor-out", "id": id, "d": text }).to_string(),
                                ))
                                .await;
                        }
                    }
                    Event::Exit(code) => {
                        // The child is gone: the tab stays readable, the session does not.
                        sessions.remove(&id);
                        let _ = socket
                            .send(Message::Text(
                                json!({ "type": "doctor-exit", "id": id, "code": code }).to_string(),
                            ))
                            .await;
                    }
                }
            }
            msg = socket.recv() => {
                let Some(Ok(Message::Text(txt))) = msg else { break };
                let parsed: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
                let id = parsed.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                match parsed.get("type").and_then(|t| t.as_str()) {
                    // ⭐ The BINARY IS RESOLVED HERE, never taken from the wire. The front
                    // names a PACKAGE (or "shell" for the floor), never a path or an argv;
                    // the engine resolves it against the catalogue's `doctor:` blocks and
                    // refuses any other name. That closes the arbitrary-exec hole the first
                    // spike had, and it is the honest split: the panel is a view, not a
                    // launcher.
                    Some("doctor-start") => {
                        // A rescue terminal can change anything, upgrades included, so the
                        // outdated map read before it is no longer the machine's.
                        *state.outdated_seen.lock().await = None;
                        let cols = parsed
                            .get("cols")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(100)
                            .clamp(20, 500) as u16;
                        let rows = parsed
                            .get("rows")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(24)
                            .clamp(5, 200) as u16;
                        let wanted = parsed
                            .get("agent")
                            .and_then(|v| v.as_str())
                            .unwrap_or("shell");
                        let clean = parsed
                            .get("clean")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let mut env: Vec<(String, String)> = Vec::new();
                        let mut args: Vec<String> = Vec::new();
                        let mut banner = String::new();
                        let program: String = if wanted == "shell" {
                            let Some((bin, a)) = crate::platform::rescue_shell(state.os) else {
                                doctor_line(&mut socket, "[doctor] no OS shell found — nothing to rescue with.").await;
                                continue;
                            };
                            args = a;
                            bin.to_string_lossy().to_string()
                        } else {
                            let Some(c) = candidates.iter().find(|c| c.name == wanted) else {
                                doctor_line(&mut socket, &format!("[doctor] `{wanted}` is not declared as a rescue (`doctor:`) in the catalogue.")).await;
                                continue;
                            };
                            let Some(bin) = on_path(&c.word) else {
                                doctor_line(&mut socket, &format!("[doctor] `{wanted}` is not on PATH — install it from the Catalog first.")).await;
                                continue;
                            };
                            if clean {
                                let Some(cl) = c.clean.as_ref() else {
                                    doctor_line(&mut socket, &format!("[doctor] `{wanted}` declares no clean launch.")).await;
                                    continue;
                                };
                                // ⭐ WHY A FRESH CONFIG DIR AND NOT A FLAG, for the agents that
                                // have one. MEASURED on Claude: `--bare` skips plugin SYNC, not
                                // plugin loading, and stops reading the keychain — it removes
                                // the authentication and keeps the plugins, the exact opposite
                                // of a rescue. A clean `CLAUDE_CONFIG_DIR` carries no plugins,
                                // hooks, memory or trust, and auth survives because it comes
                                // from the keychain. That knowledge now lives in the catalogue
                                // entry's `clean.env`; the engine only creates the directory —
                                // here, not lazily by the child: a directory the child has to
                                // invent is a directory whose failure the operator would read
                                // as "the agent is broken too".
                                if let Some(var) = cl.env.as_deref() {
                                    let rescue_dir = state.data_dir.join(format!("rescue-{}", c.id));
                                    if let Err(e) = std::fs::create_dir_all(&rescue_dir) {
                                        println!("[doctor] rescue dir unusable: {e}");
                                    }
                                    banner.push_str(&format!(
                                        "[doctor] clean config: {var}={}  (no plugins, no hooks, no inherited settings)\r\n",
                                        rescue_dir.display()
                                    ));
                                    env.push((var.to_string(), rescue_dir.to_string_lossy().to_string()));
                                }
                                args.extend(cl.args.iter().cloned());
                            }
                            bin.to_string_lossy().to_string()
                        };
                        // A Launch on an id that is still alive replaces it: kill first, or
                        // two children share one terminal.
                        if let Some(old) = sessions.remove(&id) {
                            kill(&old);
                        }
                        let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
                        // The resize channel: the front measures its box, we forward the
                        // geometry to the master. Without it the child keeps wrapping at
                        // whatever width it started with, however wide the window becomes.
                        let (rz_tx, rz_rx) = std::sync::mpsc::channel::<(u16, u16)>();
                        let killer: std::sync::Arc<std::sync::Mutex<Option<Killer>>> =
                            std::sync::Arc::new(std::sync::Mutex::new(None));
                        sessions.insert(id, Session {
                            input: in_tx,
                            resize: rz_tx,
                            killer: killer.clone(),
                            pending: Vec::new(),
                        });
                        let sink = out_tx.clone();
                        // A rescue starts in the user's HOME — what a terminal opened for a
                        // person does — never in Talos's own cwd, which is `/` from Finder,
                        // System32 from Explorer, or the repo in dev. ⚠️ On Windows this is
                        // a knowing trade: `claude` refuses a git that sits below its cwd
                        // (`pty::safe_working_dir`), and a git under AppData IS below the
                        // home. A rescue is not an install, and a human terminal in the home
                        // is exactly the situation every Windows user of claude is in; if it
                        // bites, the banner shows the cwd and the person `cd`s.
                        let cwd = crate::platform::user_home(state.os);
                        let cwd_line = cwd
                            .as_ref()
                            .map(|d| format!("[doctor] cwd: {}\r\n", d.display()))
                            .unwrap_or_default();
                        println!(
                            "[doctor] session {id}: {program} at {cols}x{rows} clean={clean}"
                        );
                        // The banner is the trace: the operator must be able to SEE which
                        // binary answered, or a rescue that silently ran the wrong one is
                        // worse than no rescue.
                        let _ = socket
                            .send(Message::Text(
                                json!({
                                    "type": "doctor-out",
                                    "id": id,
                                    "d": format!(
                                        "[doctor] {program} {}  ({cols}x{rows}, no shell)\r\n{cwd_line}{banner}",
                                        args.join(" ")
                                    ),
                                })
                                .to_string(),
                            ))
                            .await;
                        std::thread::spawn(move || {
                            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                            let code = crate::pty::run(
                                &program,
                                &arg_refs,
                                Some(in_rx),
                                Some(crate::pty::Geometry {
                                    cols,
                                    rows,
                                    updates: Some(rz_rx),
                                    cwd,
                                }),
                                &env,
                                move |k| {
                                    if let Ok(mut guard) = killer.lock() {
                                        *guard = Some(k);
                                    }
                                },
                                |bytes| {
                                    let _ = sink.send((id, Event::Out(bytes.to_vec())));
                                },
                            )
                            .unwrap_or(-1);
                            let _ = sink.send((id, Event::Out(
                                format!("\r\n[doctor: exit {code}]\r\n").into_bytes(),
                            )));
                            let _ = sink.send((id, Event::Exit(code)));
                        });
                    }
                    // A keystroke from xterm's `onData`, straight into that child's stdin.
                    // ⭐ `pty::run` builds `CommandBuilder::new(program)` and NEVER a shell
                    // line — only `platform::shell_probe` / `pty_shell` do, upstream. An
                    // absolute path here escapes a broken rc file without becoming a third
                    // place that wraps a shell, so single-shell-wrapping is untouched.
                    Some("doctor-input") => {
                        if let (Some(d), Some(sess)) = (
                            parsed.get("d").and_then(|v| v.as_str()),
                            sessions.get(&id),
                        ) {
                            let _ = sess.input.send(d.as_bytes().to_vec());
                        }
                    }
                    // The window changed size, or another tab came to the front. Forwarding
                    // this is what makes the geometry FOLLOW rather than freeze.
                    Some("doctor-resize") => {
                        if let (Some(c), Some(r), Some(sess)) = (
                            parsed.get("cols").and_then(|v| v.as_u64()),
                            parsed.get("rows").and_then(|v| v.as_u64()),
                            sessions.get(&id),
                        ) {
                            let _ = sess.resize.send((
                                c.clamp(20, 500) as u16,
                                r.clamp(5, 200) as u16,
                            ));
                        }
                    }
                    // A tab closed: its child goes with it, now, not at socket close.
                    Some("doctor-kill") => {
                        if let Some(sess) = sessions.remove(&id) {
                            kill(&sess);
                            println!("[doctor] session {id} killed by the front");
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // The tab was closed, or the window went away. Take every child with us: nothing is
    // reading them any more, and an orphaned agent holding a pty is invisible to the
    // operator — it would only surface as a process they never started.
    for (id, sess) in sessions.drain() {
        kill(&sess);
        println!("[doctor] session {id} killed with the socket");
    }
    println!("[doctor] socket closed");
}

/// ONE row of the `plan` message, as the front reads it.
///
/// A free function of plain data rather than a closure inside `handle_socket`, so the
/// MAPPING can be pinned by a test. `handle_socket` needs a live WebSocket, which put this
/// out of every test's reach — measured, not assumed: a mutant swapping `uac` for
/// `forbidden` here and hardcoding `slow: false` passed the entire suite. The three facts
/// are the front's input for the row labels, so a silent swap would mislabel rows rather
/// than fail anything.
///
/// `uac`/`forbidden` cross as booleans and so does `slow` — the CLASSIFICATION, from the
/// shared file's worst-seen `slow_secs`. `secs` is the other half and a different question:
/// this machine's LAST-SEEN duration, from the local cache. The front needs both because it
/// answers both — what the row warns about (the labels) and how long it will take ME (the
/// estimate).
///
/// ⚠️ So a row can legitimately carry `"slow": true, "secs": 1`, and that is not a
/// contradiction to debug: see timings.rs's module doc for the measured 7-Zip case.
///
/// `facts` is indexed with `get`, not `[i]`: it is built from `plan.steps` at startup and
/// cannot be shorter, but a row with no facts and a row past the end must read the same —
/// all-false — rather than panic on a socket.
///
/// `timings` arrives as plain data, not through `&AppState`, for the reason this function
/// exists at all: `handle_socket` needs a live WebSocket, so anything reached only from there
/// is unreachable from a test — and the last mutant to hide in this mapping did exactly that.
fn step_json(
    s: &crate::bundles::Step,
    i: usize,
    facts: &[crate::behaviour::Facts],
    timings: &crate::timings::Timings,
    os: Os,
) -> Value {
    let f = facts.get(i).copied().unwrap_or_default();
    json!({
        "i": i, "name": s.name, "description": s.description, "bundle": s.bundle,
        "canUninstall": s.uninstall.is_some(), "posture": s.posture.as_str(),
        "isConfig": s.is_config, "isExtension": s.is_extension,
        // Can the Apply take this row DOWN to its pin? Only when the route builds a downgrade
        // (an uninstall + a pinned reinstall); otherwise the row shows why it stays above.
        "canDowngrade": s.downgrade.is_some(),
        // `version: self` with its own `update:`: the Apply runs that command when the tool is
        // behind what the manager announces. The front's twin of `action_for` reads it.
        "selfUpdate": is_self_updating(s) && s.upgrade.is_some(),
        "updateEachApply": is_self_updating(s) && s.upgrade.is_some() && s.update_each_apply,
        "pin": s.pin, "categories": s.categories,
        "requires": s.requires, // package names this one needs (B5 transitive pull, §8)
        "uac": f.uac, "forbidden": f.forbidden, "slow": crate::plan_facts::is_slow(&f),
        // How long this took HERE last time, for the estimate's minutes. 0 = no local
        // measurement, which the widget must COUNT and SHOW as unknown rather than silently
        // treat as free. Keyed by (id, route/os), like the file — so a package measured via
        // brew contributes nothing to a winget estimate, which is the point of the key.
        //
        // `0` can ONLY mean "absent", never "it was fast": `note_duration` floors a
        // measurement at 1s, precisely so that this sentinel stays unambiguous.
        "secs": timings
            .get(&s.id)
            .and_then(|per| per.get(&crate::behaviour::key(s.route.as_deref().unwrap_or(""), os)))
            .copied()
            .unwrap_or(0)
    })
}

/// The plan for a resolved content: `{dir}` is the catalog folder actually read (a local
/// extraction when the kit has an archive), `{kit}` the kit root on the share.
fn plan_from(content: &crate::content::Content, os: Os) -> Plan {
    load_from_catalog(
        content.catalog.to_str().unwrap_or("catalog"),
        content.kit.to_str().unwrap_or(""),
        os,
        &|m| println!("[bundles] {m}"),
    )
}

/// The `plan` message: the whole catalogue as the front needs it.
///
/// ONE builder for both the connect send and the Refresh reload. Duplicating it is how the
/// two drift — a field added for the reload but not at connect (or the reverse) is a defect
/// nobody sees until a specific gesture, since both paths feed the same renderer.
fn plan_message(state: &AppState, steps: &[crate::bundles::Step]) -> Value {
    let steps_json: Vec<_> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut row = step_json(s, i, &state.facts, &state.timings, state.os);
            // Read here, not in `step_json`, because it is a fact of this SESSION: a front that
            // connects again keeps the row done, and a Refresh — which empties the set before
            // building this message — re-arms it.
            row["updatedThisSession"] = json!(updated_this_session(state, s));
            row
        })
        .collect();
    // The top-panel "needs" (profiles.yaml), loaded at boot. Same keys the front
    // expects (renderProfiles, app.js:164): name/emoji/usage/highlights/description/packages.
    // `defaultOn` is the CLOSED set (a card a default one needs is on too), computed once
    // here for both the cards and the first-launch selection below.
    let default_cards = crate::profiles::default_active(&state.profiles);
    let profiles_json: Vec<_> = state
        .profiles
        .items
        .iter()
        .map(|p| {
            json!({
                "name": p.name, "emoji": p.emoji, "usage": p.usage,
                "highlights": p.highlights, "description": p.description,
                "packages": p.packages, "needs": p.needs,
                "defaultOn": default_cards.contains(&p.name)
            })
        })
        .collect();
    // REAL consent (read from the local store): undecided at 1st boot → the front
    // opens the sharing dialog. REAL selection: the persisted toggles the front restores
    // (yellow). Intent is remembered, presence is re-detected.
    //
    // ⭐ Re-read here rather than captured, which is what makes a reload correct: the
    // selection is keyed by NAME (`selection.rs`), so a package added to the catalogue
    // between two reads keeps whatever the user had already decided about the others. That
    // property is why reloading the catalogue is safe at all — an index-keyed selection
    // would shift under every insertion.
    let (catalog_dir, content_problem) = match state.content.lock() {
        Ok(c) => (c.catalog.clone(), c.problem()),
        Err(_) => (PathBuf::new(), None),
    };
    json!({
        "type": "plan",
        "steps": steps_json,
        // On a FIRST launch (no file yet) the author's `default: on` cards come back as the
        // active set — the engine decides, the front restores what it is handed.
        "selection": read_selection_or_first_launch(&state.data_dir, &default_cards),
        "profiles": profiles_json,
        "profileColumns": state.profiles.columns,
        "consent": read_consent(&state.consent),
        "build": crate::build_info::build_json(), // exact stamp of the source snapshot (no more hardcoding)
        // The content's own name, read beside the catalog on every build of this message —
        // the catalogue is re-read on Refresh, and the file that names it must not lag it.
        // `null` when the content has none: the front then shows the title it always did.
        "distribution": crate::distribution::read_beside(&catalog_dir).map(|d| d.json()),
        // Set when the kit's content archive is unusable or gone (content.rs): the sentence the
        // front shows above the list. `null` when the content is what the kit says it is.
        "contentProblem": content_problem,
        "appmgmt": appmgmt_wire(state.appmgmt),
        // Whether the Doctor tab exists at all. The dropdown's contents travel on the
        // Doctor's own socket, which opens only when the tab is clicked — too late to
        // decide whether to SHOW the tab. So the answer rides the first message instead.
        "doctor": doctor_declared(steps)
    })
}

/// A catalogue that declares no rescue candidate has no Doctor: the tab is not shown.
/// The floor (the OS shell) is engine-owned and always launchable, but it is a floor
/// UNDER declared candidates, not a reason to show a rescue tab to a kit whose author
/// never asked for one. Declared, not present: a candidate that is not installed still
/// gives the tab a reason to exist (it points at the Catalog row that installs it).
fn doctor_declared(steps: &[crate::bundles::Step]) -> bool {
    !doctor_candidates(steps).is_empty()
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    // A SNAPSHOT, not a borrow of the lock. Every reader below awaits repeatedly (socket
    // sends, spawn_blocking probes), and holding a read guard across those awaits would
    // block the `rescan` writer for the length of a whole scan.
    //
    // ⭐ It is also the correct SEMANTICS, which is the better reason: a scan must reason
    // about ONE version of the catalogue from its first row to its last. Reading through
    // the lock would let a reload land mid-scan and split one scan across two catalogues —
    // emitting row 7 of the old plan and row 8 of the new, with `last_seen` indices
    // straddling both. The clone is 33 steps of pure data.
    let steps = state.plan.read().await.steps.clone();
    let steps = &steps;

    // 1) REAL plan — flat steps read from the catalog (no more hardcoding). Same keys
    // the front expects (see app.js render/ws.onmessage). Bundles are the top cards,
    // sent separately as `profiles` (profiles.rs) — steps no longer carry a bundle layer.
    let _ = socket
        .send(Message::Text(
            plan_message(state.as_ref(), steps).to_string(),
        ))
        .await;

    // 2) REAL SCAN — see scan_and_emit. Done at connect AND on every `rescan` (Refresh
    // button). "Detect, don't remember".
    scan_and_emit(&mut socket, &state).await;

    // 4) Message loop from the front. The Apply is the one path that acts:
    //  - `apply` (the Apply button) → apply_diff (re-scan, plan, execution).
    //  - `retry-step` {i, action} → re-runs the action the Apply chose for one row, after
    //    a 403. Not a choice: the front sends what `actionOf` derived.
    //  - `diff` and `cancel-step` inspect or stop a row; they never install anything.
    //  The per-row install/uninstall/upgrade/downgrade verbs were retired on 2026-10-10.
    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(txt) = msg else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
        let Some(kind) = parsed.get("type").and_then(|t| t.as_str()) else {
            continue;
        };
        match kind {
            "apply" => {
                let on = json_indices(&parsed, "on");
                let off = json_indices(&parsed, "off");
                // Rows the front computed as out of scope. A THIRD list is needed,
                // not merely an omission from on/off: an omitted index is
                // indistinguishable from `auto` (see the desire match in
                // apply_diff), so the server would have no way to know.
                let unmanaged = json_indices(&parsed, "unmanaged");
                // The rows whose removal the button announced; the Apply runs no other
                // (`drop_unannounced`).
                let announced = json_indices_opt(&parsed, "removals");
                // An Apply converges the WHOLE desire. A front from before 2026-10-10 still
                // sends a `rung` (the ladder that filtered by kind); it is ignored, not parsed.
                apply_diff(&mut socket, &state, on, off, unmanaged, announced).await;
            }
            // The per-row action verbs RETIRED on 2026-10-10: the Apply is the one path that
            // acts. A stale front may still send one after locking its UI, so it is answered
            // with `done` — never acted on, never left hanging.
            v if RETIRED_ROW_VERBS.contains(&v) => {
                println!("[apply] ignored retired row verb {v}");
                let _ = socket
                    .send(Message::Text(json!({ "type": "done" }).to_string()))
                    .await;
            }
            // diff: a config-atom the user turned OFF is self-managed, so its row
            // button INSPECTS (model.js::rowGesture) — the atom
            // has no install path, and re-applying would clobber the file the user chose
            // to own. Runs the atom's `check:` (its dry-run: exit 0 = converged, 1 =
            // drifted) through the pty so the output lands in the row's own terminal,
            // which is the whole point: the user wants to SEE the difference.
            //
            // This arm was MISSING and the freeze was live: the front locks its UI
            // before sending, so `_ => {}` left the panel dead until the window was
            // closed. See LOCKING_CLIENT_MSGS and its test.
            "diff" => {
                let i = parsed.get("i").and_then(|v| v.as_u64()).map(|n| n as usize);
                if let Some(i) = i {
                    diff_step(&mut socket, &state, i).await;
                }
                // `done` unconditionally — even for a bad index or an atom with no
                // `check:`. The front is locked and only this releases it.
                let _ = socket
                    .send(Message::Text(json!({ "type": "done" }).to_string()))
                    .await;
            }
            // retry-step: re-runs the action the Apply chose for row i (after a 403 failure).
            "retry-step" => {
                let i = parsed.get("i").and_then(|v| v.as_u64()).map(|n| n as usize);
                let action = parsed.get("action").and_then(|v| v.as_str());
                if let (Some(i), Some(action)) = (i, action) {
                    retry_step(&mut socket, &state, i, action).await;
                }
            }
            // set-selection: persists intent (toggles). The client already applied
            // it optimistically → no response. Best-effort. Reset sends an
            // empty pkgs here → stored empty → next startup reloads the defaults.
            "set-selection" => {
                let sel: Selection = parsed
                    .get("selection")
                    .cloned()
                    .and_then(|s| serde_json::from_value(s).ok())
                    .unwrap_or_default();
                write_selection(&state.data_dir, &sel);
            }
            // set-consent: records the sharing choice (marks consent decided), then answers
            // with the log, so the Machine tab's share line follows the toggle.
            "set-consent" => {
                let share = parsed
                    .get("share")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                set_consent(&state, share);
                println!("[consent] set: share={share}");
                let _ = socket.send(Message::Text(log_msg(&state))).await;
            }
            // get-log: the History, Settings and Machine tabs request the consent, the local history
            // and the profile.
            "get-log" => {
                let _ = socket.send(Message::Text(log_msg(&state))).await;
            }
            // clear-log: empties the LOCAL journal only (the shared team copy is
            // left intact), then returns the emptied log to refresh the tab.
            "clear-log" => {
                clear_history(&state.consent);
                println!("[consent] local history cleared");
                let _ = socket.send(Message::Text(log_msg(&state))).await;
            }
            // rescan: Refresh button → live re-scan of presence (state + state-done).
            // Without this handler, the message fell into _ => {} and the UI stayed veiled
            // (steps-refreshing) without ever receiving a response → "refresh forever".
            "rescan" => {
                // ⭐ Refresh re-reads the CATALOGUE, then the machine. Editing `catalog/`
                // while Talos ran used to change nothing until a relaunch — a new yaml
                // invisible, a deleted one still on screen, an edited command still acting
                // on its old value. The catalogue is an observation of the disk exactly as
                // presence is an observation of the machine, so "detect, don't remember"
                // governs both.
                //
                // The reload comes FIRST and the scan after, in one gesture: scanning the
                // old plan and then swapping it would paint verdicts for rows that are
                // about to be replaced.
                // The content is RE-RESOLVED, not just re-read: an archive that changed during
                // the session is a new extraction folder. ⚠️ Only the catalogue follows it —
                // the bundle cards (profiles) are read once at startup, as they always were.
                let content = tokio::task::spawn_blocking(crate::content::resolve)
                    .await
                    .expect("the content resolve does not panic");
                crate::content::publish(&content);
                println!("{}", content.log_line());
                let reloaded = plan_from(&content, state.os);
                // The Machine tab follows the content a Refresh moved to (the journal keeps the
                // line it wrote at collection: one profile per launch).
                if let Some(p) = state
                    .machine
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                {
                    set_profile_content(p, &content);
                }
                if let Ok(mut c) = state.content.lock() {
                    *c = content;
                }
                let n = reloaded.steps.len();
                let before = {
                    let mut plan = state.plan.write().await;
                    let before = plan.steps.len();
                    *plan = reloaded;
                    before
                };
                // ⚠️ `last_seen` is POSITIONAL, so it is DISCARDED rather than remapped.
                // Its indices refer to the plan that produced them; carrying them over a
                // catalogue that gained or lost a package would point the Apply re-scan's
                // narrowing (`seeds_for_rescan`) at the wrong rows — a real change made
                // invisible, silently, which is the one failure mode that machinery exists
                // to prevent. Everything is re-probed instead: that IS what Refresh does,
                // and it is why the operator chose this over remapping by name — a carried
                // presence is a memory, and this project detects.
                {
                    let mut seen = state.last_seen.lock().await;
                    seen.clear();
                    seen.resize(n, None);
                }
                // Lines queued for a row's terminal name a row by its INDEX, which this
                // reload may have given to another package.
                state.notes.lock().await.clear();
                // ⭐ The Refresh BUTTON re-arms the tools whose update runs at each Apply — this one
                // gesture, never the Apply's own re-scan nor the probe after a step, or the end
                // of every run would find them pending again.
                state
                    .updated_this_session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clear();
                if before == n {
                    println!("[catalog] reloaded: {n} packages");
                } else {
                    // Named, not silent: a count that moved is the whole point of the
                    // gesture, and it is what the operator will want to see confirmed.
                    println!("[catalog] reloaded: {before} → {n} packages");
                }
                // The front re-renders from a fresh `plan` — rows appear, disappear, or
                // change — and the selection travels with it, keyed by NAME so the user's
                // decisions about the OTHER packages survive an insertion.
                let steps = state.plan.read().await.steps.clone();
                let _ = socket
                    .send(Message::Text(
                        plan_message(state.as_ref(), &steps).to_string(),
                    ))
                    .await;
                scan_and_emit(&mut socket, &state).await;
            }
            // open-forbidden: the "Open blocked page" button of the 403 banner → opens
            // the blocked URL in the default browser, next to the panel, so the
            // user approves the firewall access then Retry. ⚠️ This handler was once
            // absent, and the click fell into `_ => {}` and did nothing, silently —
            // the same class of defect as the `diff` freeze. See LOCKING_CLIENT_MSGS.
            "open-forbidden" => {
                if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                    if let Err(e) = crate::platform::open_url(url) {
                        println!("[forbidden] failed to open {url}: {e}");
                    } else {
                        println!("[forbidden] opening browser: {url}");
                    }
                }
            }
            // quit: close the app from a button in the panel.
            //
            // It goes over the WEBSOCKET and not through Tauri's JS API, because the
            // front is served from a real URL (http://127.0.0.1:1420) — "path A (WS, no
            // IPC)", as main.rs puts it: the front does not drive the native window, and
            // `window.__TAURI__` is not there to call. The socket is the only channel we
            // have, so the socket is the channel we use.
            //
            // The sudo password is cleared FIRST. It lives in RAM for the duration of an
            // Apply (model C) and is wiped at the end of one; quitting mid-way is another
            // kind of end, and a secret must not survive on a path that skipped its
            // cleanup. It costs one lock and removes a whole class of question.
            //
            // Then `std::process::exit`, deliberately, rather than asking Tauri to close
            // the window: this thread is inside the tokio runtime on a spawned thread, not
            // the main thread that owns the AppHandle, so there is no handle to ask. A
            // running child (a real installer, minutes long) dies with us — which is why
            // the FRONT refuses to send this while an Apply is running. The guard belongs
            // there, where the user can be told why, not here where it can only be silent.
            "quit" => {
                clear_sudo_pw(&state).await;
                println!("[quit] asked by the panel — exiting");
                std::process::exit(0);
            }
            // A cancel that arrives with no step running has nothing to kill: the step
            // that would have handled it is already over. Silently fine — the front may
            // legitimately send it as the row finishes. Listed explicitly so it does not
            // read as an unhandled verb in the log.
            "cancel-step" => {}
            // Deep-link straight to System Settings → Privacy & Security → App
            // Management, so the user can grant the permission a cask upgrade needs.
            "open-appmgmt-settings" => {
                let url = "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AppBundles";
                if let Err(e) = crate::platform::open_url(url) {
                    println!("[appmgmt] failed to open settings: {e}");
                }
            }
            // Unknown verb. If it is one the front locks its UI for, we MUST answer
            // something or the panel stays dead — no timeout exists on that side (a real
            // install can run for minutes, so a watchdog would fire mid-install or be
            // useless). Belt to the LOCKING_CLIENT_MSGS test's braces: the test catches
            // this at build time, this catches a verb someone forgot to add to the list.
            other => {
                println!("[ws] unhandled message type: {other}");
                if LOCKING_CLIENT_MSGS.contains(&other) {
                    let _ = socket
                        .send(Message::Text(json!({ "type": "done" }).to_string()))
                        .await;
                }
            }
        }
    }
}

/// Runs a config-atom's `check:` — its DRY RUN — and streams it into the row's
/// terminal. Nothing is written to the machine: this answers "what differs?", which is
/// what the row button offers for an atom the user turned off (it owns that file now).
///
/// A step with no `check:` is a no-op here rather than an error: the caller emits `done`
/// either way, so the UI never stays locked.
async fn diff_step(socket: &mut WebSocket, state: &AppState, i: usize) {
    // Cloned out of the lock rather than borrowed through it: what follows awaits on a
    // pty, and a read guard held across that would stall a Refresh for its duration.
    let Some(step) = state.plan.read().await.steps.get(i).cloned() else {
        return;
    };
    let step = &step;
    let Some(check) = step.check.clone() else {
        return;
    };
    let _ = socket
        .send(Message::Text(
            json!({ "type": "step", "i": i, "status": "checking" }).to_string(),
        ))
        .await;
    // A dry run's only verdict is its exit code: a `check:` reports drift, it does not
    // install, so there is no 403 to surface and nothing to record as behaviour.
    let code = run_in_pty(socket, state, i as u32, &check).await.code;
    // exit 0 = converged, non-zero = drifted. Say which, in the terminal, rather than
    // only colouring a pill: the operator asked to SEE the difference.
    let line = format!(
        "\r\n\x1b[2m[diff] exit {code} → {}\x1b[0m\r\n",
        if code == 0 {
            "matches what Talos ships"
        } else {
            "differs from what Talos ships (yours is kept)"
        }
    );
    {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let _ = socket
            .send(Message::Text(
                json!({ "type": "out", "i": i, "data": STANDARD.encode(line.as_bytes()) })
                    .to_string(),
            ))
            .await;
    }
    // Settle the row: a diff never changes presence, so `self` carries no verdict — the front
    // reads it as "back to the presence you had" (`presenceStatus`). It is not a label: an atom
    // with a way back, wanted absent, is still the next Apply's removal.
    let _ = socket
        .send(Message::Text(
            json!({ "type": "step", "i": i, "status": "self" }).to_string(),
        ))
        .await;
}

/// SERIAL presence SCAN, emitted as each verdict lands, then `state-done`
/// (unfreezes the UI). Called at connect AND on `rescan` (Refresh button — without
/// this handler, the message fell into the void → the UI stayed veiled "forever").
///
/// ⚠️ STRICTLY SERIAL, ON PURPOSE — do not "optimise" this back into a fan-out.
/// The previous version spawn_blocking'd every probe at once. On Windows that is
/// ~2 probes × 22 winget-routed packages = ~44 concurrent `powershell.exe`, of which
/// ~22 are `winget list` fighting over winget's shared temp/cache — the very
/// contention a "serial lock" had fixed pre-Tauri and that the Rust port silently
/// dropped (memory windows-porting-regressions). Concurrency there bought latency,
/// not speed.
///
/// Serial also makes the SCREEN honest: one probe runs at a time and its row is
/// painted the moment it answers, so the accordion fills visibly instead of showing
/// a frozen wall of "checking…" (which the old code made worse by awaiting the
/// batched outdated BEFORE emitting anything, holding back verdicts already in hand).
///
/// The batched machine-wide outdated stays ONE command for the whole machine
/// ("fewer processes, not more threads") — it runs first, serially, then the probes.
async fn scan_and_emit(socket: &mut WebSocket, state: &Arc<AppState>) {
    let os = state.os;
    // Snapshot, for the same reason as `handle_socket`: one scan, one catalogue.
    let steps = state.plan.read().await.steps.clone();
    let steps = &steps;
    let started = std::time::Instant::now();
    let scanned = tokio::task::spawn_blocking(move || scan_outdated(os))
        .await
        .unwrap_or(Err(crate::outdated::ScanFailure::CommandFailed(
            "the scan task did not finish".into(),
        )));
    println!("[scan] outdated (batched) in {:?}", started.elapsed());
    // The same timings the console lines print, kept for the journal: a Windows release has
    // no console, so without this record every one of them is lost on the fleet.
    let mut record = crate::machine::ScanRecord {
        kind: "scan".into(),
        outdated_ms: started.elapsed().as_millis() as u64,
        packages: steps.len(),
        profile_running: state
            .machine_running
            .load(std::sync::atomic::Ordering::SeqCst),
        ..Default::default()
    };

    // ⚠️ An unreadable scan is NOT "nothing is outdated". Saying so out loud is
    // the difference between a user who knows the check did not happen and one
    // who trusts a screen of green rows. The rows still work — presence is a
    // separate probe — but nothing claims to know about upgrades.
    let scan = match &scanned {
        Ok(map) => {
            // Recorded for the Apply that usually follows within minutes. Only a READABLE
            // map: an unavailable scan must be retried, not remembered as "nothing to do".
            *state.outdated_seen.lock().await = Some(OutdatedSeen {
                at: std::time::Instant::now(),
                map: map.clone(),
            });
            map.clone()
        }
        Err(reason) => {
            *state.outdated_seen.lock().await = None;
            eprintln!("[scan] outdated UNAVAILABLE: {}", reason.message());
            record
                .unavailable
                .push(format!("outdated: {}", reason.message()));
            let _ = socket
                .send(Message::Text(
                    json!({
                        "type": "outdated-unavailable",
                        "reason": reason.message(),
                    })
                    .to_string(),
                ))
                .await;
            std::collections::HashMap::new()
        }
    };
    // One listing for the whole machine, fetched beside the outdated scan and for the
    // same reason: fewer processes, not more threads. An unrecognised listing yields
    // None → every package probes as before, so the worst case is today's behaviour
    // and never a screen of false absents.
    let bulk_started = std::time::Instant::now();
    let bulk = tokio::task::spawn_blocking(move || crate::detect::fetch_bulk_presence(os))
        .await
        .unwrap_or(None);
    record.presence_ms = bulk_started.elapsed().as_millis() as u64;
    match &bulk {
        Some(t) => println!(
            "[scan] presence (batched) {} installed in {:?}",
            t.len(),
            bulk_started.elapsed()
        ),
        None => {
            eprintln!("[scan] presence listing UNAVAILABLE — probing per package");
            record
                .unavailable
                .push("presence listing: probing per package".into());
        }
    }
    // Shared read-only across the per-package tasks: cloning the table per package
    // would undo exactly the saving this is here for.
    let bulk = std::sync::Arc::new(bulk);
    // The VS Code host + its profile manifest, fetched ONCE for the whole scan beside the
    // presence listing and for the same reason: fewer processes, not more threads. Probing the
    // host per row would cost ~0.2 s of Electron start per extension on a scan that is serial on
    // purpose, and would walk into microsoft/vscode#302026 — where `code` resolved to the GUI and
    // OPENED A WINDOW instead of printing — once per row rather than once per scan.
    //
    // ⭐ NO threshold here, unlike the bulk listing (which is gated at three rows because one
    // `winget list` costs ~3.7 s against ~1.3 s for a per-package probe). One `code --version`
    // plus one file read is ~0.2 s, so there is no break-even below which it is a loss — and the
    // alternative is not "probe per row" but "no answer at all": without the snapshot every
    // extension row reads unknown. The missing threshold is the design, not an oversight.
    let vscode_started = std::time::Instant::now();
    let home = crate::detect::user_home();
    let vs = tokio::task::spawn_blocking(move || crate::vscode::snapshot(os, &home))
        .await
        .ok();
    record.vscode_ms = vscode_started.elapsed().as_millis() as u64;
    match &vs {
        Some(s) => println!(
            "[scan] vscode host={} manifest={} in {:?}",
            s.host_present,
            match &s.installed {
                Ok(v) => format!("{} extensions", v.len()),
                Err(()) => "UNREADABLE".to_string(),
            },
            vscode_started.elapsed()
        ),
        None => eprintln!("[scan] vscode snapshot UNAVAILABLE — extension rows read unknown"),
    }
    match &vs {
        None => record
            .unavailable
            .push("vscode snapshot: extension rows read unknown".into()),
        Some(s) if s.installed.is_err() => record
            .unavailable
            .push("vscode manifest unreadable: extension rows read unknown".into()),
        Some(_) => {}
    }
    // Shared read-only across the per-package probes, same as `bulk`: the manifest holds ~30
    // entries and cloning it per row would undo the saving this is here for.
    let vs = std::sync::Arc::new(vs);
    for (i, step) in steps.iter().enumerate() {
        let step_owned = step.clone();
        let bulk_c = std::sync::Arc::clone(&bulk);
        let vs_c = std::sync::Arc::clone(&vs);
        let probe_started = std::time::Instant::now();
        let p = tokio::task::spawn_blocking(move || {
            crate::detect::detect_present_detailed_with_vscode(
                &step_owned,
                os,
                bulk_c.as_ref().as_ref(),
                vs_c.as_ref().as_ref(),
            )
        })
        .await
        .unwrap_or_default();
        // Timing per package: this is the evidence for "is serial bearable?".
        record.probes.push(crate::machine::ProbeTime {
            name: step.name.clone(),
            ms: probe_started.elapsed().as_millis() as u64,
        });
        println!(
            "[scan] {}/{} {} in {:?}",
            i + 1,
            steps.len(),
            step.name,
            probe_started.elapsed()
        );
        let state_msg = json!({
            "type": "state", "i": i,
            "present": p.present, "reason": p.reason,
            "version": p.version.clone().unwrap_or_default(), "external": p.external,
            "probe": probe_json(&p.diag) // the proof: command + output + code
        });
        let _ = socket.send(Message::Text(state_msg.to_string())).await;
        if p.present == Some(true) {
            let od = outdated_for(step.system_id.as_deref(), &scan);
            emit_outdated_if(socket, i, od).await;
            let announced = od.map(|o| o.available.as_str());
            if hold_still_in_view(step, od) {
                let act = crate::decision::Action::Upgrade;
                if let Some(u) =
                    unmoved_in_view(state, step, act, p.version.as_deref(), announced).await
                {
                    emit_unmoved(socket, i, &u).await;
                }
            }
        }
        // Remember the verdict: the Apply re-scan is SCOPED to the diff, and the rows
        // it skips read their presence from here (see `last_seen`).
        remember_presence(state, i, p).await;
    }
    println!(
        "[scan] TOTAL {:?} for {} packages",
        started.elapsed(),
        steps.len()
    );
    // state-done — unfreezes the UI (removes the scan/refresh veil).
    let _ = socket
        .send(Message::Text(json!({ "type": "state-done" }).to_string()))
        .await;
    record.total_ms = started.elapsed().as_millis() as u64;
    record.at = chrono::Utc::now().to_rfc3339();
    record.profile_running |= state
        .machine_running
        .load(std::sync::atomic::Ordering::SeqCst);
    append_record(&state.consent, &record);
    *state.last_scan.lock().unwrap_or_else(|e| e.into_inner()) = Some(record);
    start_machine_profile(state, steps);
}

/// How long the profile may take before it is recorded as not finished. Measured: ~1.5 s
/// on the Windows VM; a corporate machine's Defender and Security Center queries take
/// seconds, not a minute.
const MACHINE_PROFILE_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Collects this launch's machine profile, ONCE, in the background, and journals it.
///
/// Started at the end of the first scan and not at boot: the profile spawns two shells,
/// and the scan is serial on purpose — they must not compete with its probes, nor make
/// them look slower than they are. Nothing waits on it; the Machine tab reads "collecting"
/// until it lands. A Refresh clicked meanwhile does overlap it, and its scan line says so
/// (`profile_running`).
///
/// A collection that panics or hangs (a stuck network volume, a WMI query that never
/// answers) is recorded as a failed profile after `MACHINE_PROFILE_LIMIT`, so the card says
/// what happened instead of "collecting" for the whole session. The stuck thread itself
/// cannot be killed; its late result is dropped.
fn start_machine_profile(state: &Arc<AppState>, steps: &[crate::bundles::Step]) {
    use std::sync::atomic::Ordering;
    if state.machine_started.swap(true, Ordering::SeqCst) {
        return;
    }
    state.machine_running.store(true, Ordering::SeqCst);
    let routes: Vec<String> = steps.iter().filter_map(|s| s.route.clone()).collect();
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let (os, kit) = (state.os, state.consent.exe_dir.clone());
        let job = tokio::task::spawn_blocking(move || {
            let routes: Vec<&str> = routes.iter().map(String::as_str).collect();
            crate::machine::collect(os, &kit, &routes)
        });
        let mut profile = match tokio::time::timeout(MACHINE_PROFILE_LIMIT, job).await {
            Ok(Ok(p)) => p,
            // `~` here too: a panic message may carry a path, and this line may be shared.
            Ok(Err(e)) => crate::machine::Profile::failed(&crate::machine::tilde_all(
                &format!("the collection failed: {e}"),
                &crate::detect::user_home().to_string_lossy(),
            )),
            Err(_) => crate::machine::Profile::failed(&format!(
                "the collection did not finish in {} s",
                MACHINE_PROFILE_LIMIT.as_secs()
            )),
        };
        state.machine_running.store(false, Ordering::SeqCst);
        // Which content this machine reads, journalled with its profile — so the fleet's
        // logs show which machine runs an earlier copy because its archive did not sync.
        if let Ok(c) = state.content.lock() {
            set_profile_content(&mut profile, &c);
        }
        println!(
            "[machine] {} cores, {} MB free of {} MB, one shell {} ms, collected in {} ms{}",
            profile.cores,
            profile.ram_available_mb,
            profile.ram_total_mb,
            profile.shell_ms,
            profile.collect_ms,
            profile
                .error
                .as_deref()
                .map(|e| format!(" — {e}"))
                .unwrap_or_default()
        );
        append_record(&state.consent, &profile);
        if read_consent(&state.consent).share {
            state.machine_shared.store(true, Ordering::SeqCst);
        }
        *state.machine.lock().unwrap_or_else(|e| e.into_inner()) = Some(profile);
    });
}

/// The machine profile's account of which content this launch reads (content.rs).
fn set_profile_content(profile: &mut crate::machine::Profile, content: &crate::content::Content) {
    profile.kit.content = crate::machine::tilde_all(
        &content.summary(),
        &crate::detect::user_home().to_string_lossy(),
    );
    profile.kit.content_stale = content.problem().is_some();
}

/// Records the share choice. Turning sharing ON mid-session also copies this launch's
/// machine profile to the shared log: it was journalled locally when collected, and the
/// Machine tab now says the share receives what it shows.
fn set_consent(state: &AppState, share: bool) {
    use std::sync::atomic::Ordering;
    let was = read_consent(&state.consent).share;
    write_consent(&state.consent, share);
    if share && !was {
        let profile = state
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        // Once per launch: on, off, on again must not send the same profile twice.
        if let Some(p) = profile {
            if !state.machine_shared.swap(true, Ordering::SeqCst) {
                share_record(&state.consent, &p);
            }
        }
    }
}

/// `log` message for the History, Settings and Machine tabs: consent, local history, and this launch's machine records.
fn log_msg(state: &AppState) -> String {
    json!({
        "type": "log",
        "consent": read_consent(&state.consent),
        "history": read_history(&state.consent),
        // What the journal received about this machine, this launch — shown as-is in the
        // Machine tab, so what is shared is never a description that could drift from it.
        "machine": state.machine.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        "scan": state.last_scan.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        // Where a consented copy goes, said with the same `~` the profile uses.
        "sharedTo": crate::machine::tilde(
            &shared_log_path(&state.consent.exe_dir, &state.consent.host, &state.consent.user)
                .to_string_lossy(),
            &crate::detect::user_home().to_string_lossy(),
        ),
    })
    .to_string()
}

/// Builds the `probe` field of a `state` message from the detection diag:
/// the command ACTUALLY run + its FULL output + its code, so the operator
/// sees in the row's terminal WHAT decided present/absent/version. Output
/// untruncated: when we doubt a result, we want the whole proof (detection
/// commands are short by nature; a huge output is itself
/// information).
fn probe_json(diag: &Option<crate::detect::ProbeResult>) -> serde_json::Value {
    match diag {
        Some(d) => json!({ "cmdline": d.cmdline, "output": d.output, "code": d.code, "ok": d.ok }),
        None => serde_json::Value::Null,
    }
}

/// Does this client message make the front OPTIMISTICALLY lock its UI, so that the
/// server owes it a `done` to unlock again?
///
/// The front sets `applyRunning = true` before sending a `diff`, a `retry-step` or an
/// `apply` (see app.js: the row `onclick`, `retryStep` and `applyAll`), and `refreshLiveness()`
/// then disables every button in the panel. Nothing but a server message clears it —
/// there is no timeout, by design (a real install can take minutes, so a watchdog
/// would either fire during a legitimate install or be useless).
///
/// ⚠️ THE FAILURE MODE THIS EXISTS TO PREVENT: a verb the front sends and the server's
/// match does not handle falls into `_ => {}`, no `done` is ever emitted, and the panel
/// is dead until the window is closed — no error, no recovery, indistinguishable from a
/// hang. That happened for real with `diff` (config-atom rows): the row button (then
/// `model.js::buttonAction`, now `rowGesture`) returned `{type:"diff"}` for a config-atom
/// the user turned OFF, the front sent it verbatim, and the server had no arm for it. `Starship config` ships in
/// `bundles/terminal.yaml`, so it was reachable with the shipped catalogue.
///
/// Keeping this list next to the handler — and asserting the handler covers it — makes
/// the next added verb a compile-adjacent concern rather than a silent freeze.
const LOCKING_CLIENT_MSGS: &[&str] = &["apply", "diff", "retry-step"];

/// The per-row action verbs RETIRED on 2026-10-10, when the Apply became the one path that
/// acts. A stale front may still send one after locking its UI, so `handle_socket` answers
/// each with `done` — never acted on, never left hanging.
const RETIRED_ROW_VERBS: &[&str] = &["install", "uninstall", "upgrade", "downgrade"];

/// Extracts an array of indices from a JSON field ("on"/"off").
fn json_indices(v: &serde_json::Value, key: &str) -> Vec<usize> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64().map(|n| n as usize))
                .collect()
        })
        .unwrap_or_default()
}

/// `json_indices`, but a missing key is `None` rather than an empty list: "nothing announced"
/// and "no list sent" are different answers.
fn json_indices_opt(v: &serde_json::Value, key: &str) -> Option<Vec<usize>> {
    v.get(key)?;
    Some(json_indices(v, key))
}

/// Is this row IMPOSSIBLE here because a `requires:` can never hold — the retry
/// half of the requirement gate, reusing `deps::requires_blocked` rather than
/// re-deriving the rule (one rule, three callers: model.js, apply_diff, here).
///
/// The desires are NOT knowable on this path — `retry_step` never receives the wire's
/// `on`/`off` lists, and recomputing the front's transitive pull server-side would be a
/// second implementation of `wantedNames` in Rust. So `will_be_present` is read
/// OPTIMISTICALLY (true for every node): the formula then reduces to
/// `present_now || reachable`, which is exactly "could this requirement ever be here?".
/// The narrowing is deliberate — see the call site.
async fn requirement_blocks_row(state: &AppState, step: &crate::bundles::Step) -> Option<String> {
    use crate::deps::{make_index, requires_blocked, DepNode};
    if step.requires.is_empty() {
        return None; // the overwhelming majority — no lock, no clone, no walk
    }
    let steps = state.plan.read().await.steps.clone();
    let seen = state.last_seen.lock().await;
    let nodes: Vec<DepNode> = steps
        .iter()
        .enumerate()
        .map(|(j, s)| DepNode {
            name: s.name.clone(),
            requires: s.requires.clone(),
            will_be_present: true, // unknown desire, read optimistically (see the doc)
            // ⚠️ `Some(true)`, never "not Some(false)": an UN-PROBED row reads None, and
            // treating that as present would let the gate pass on a row nobody looked at.
            present_now: seen.get(j).cloned().flatten().and_then(|p| p.present) == Some(true),
            reachable: s.uninstall.is_some(),
        })
        .collect();
    let idx = make_index(&nodes);
    let me = nodes.iter().position(|n| n.name == step.name)?;
    requires_blocked(&nodes[me], &nodes, &idx)
}

/// Re-runs the action the Apply chose for row i, after that step failed (a 403). Not a
/// choice: the front sends the action `actionOf` derived, the same rule the batch used.
/// Emits `done` whatever the attempt did: the front froze itself when it sent the retry, and
/// `done` is the only thing that unfreezes it.
async fn retry_step(socket: &mut WebSocket, state: &AppState, i: usize, action: &str) {
    retry_attempt(socket, state, i, action).await;
    let _ = socket
        .send(Message::Text(json!({ "type": "done" }).to_string()))
        .await;
}

/// One retry, which may end early (out of scope, an impossible requirement, an unknown row
/// or verb, a missing App Management permission). It never sends `done`: its caller does.
async fn retry_attempt(socket: &mut WebSocket, state: &AppState, i: usize, action: &str) {
    use crate::decision::Action;
    // Cloned out of the lock: the action below runs a pty, and holding a read guard
    // across it would block a Refresh for the whole install.
    let Some(step) = state.plan.read().await.steps.get(i).cloned() else {
        return;
    };
    let step = &step;
    // THE TEETH. A row the user put out of scope is not ours to touch — and this path
    // never sees the wire lists, so without this check a retry could act on a row the
    // panel already put out of scope.
    let sel = read_selection(&state.data_dir);
    if scope_refuses(&sel, &step.name) {
        println!("[scope] refused {} on {} (out of scope)", action, step.name);
        return;
    }
    // THE SECOND SET OF TEETH: an IMPOSSIBLE requirement. This path never sees the wire's
    // `on`/`off`, so it asks a deliberately NARROWER question than the batch does — not
    // "will the requirement be there after this Apply?" but "could it ever be here?".
    // A retry is about one row the Apply already chose, and refusing it because a
    // requirement is merely not-yet-installed would break the legitimate retry of a
    // plugin whose host is on its way in the same Apply.
    if let Some(why) = requirement_blocks_row(state, step).await {
        println!("[scope] refused {} on {} ({})", action, step.name, why);
        let _ = socket
            .send(Message::Text(
                json!({ "type": "state", "i": i, "reason": why }).to_string(),
            ))
            .await;
        return;
    }
    let act = match action {
        "install" => Action::Install,
        "uninstall" => Action::Uninstall,
        "upgrade" => Action::Upgrade,
        "downgrade" => Action::Downgrade,
        _ => return,
    };
    // ⭐ A retried version change is judged like the batch's (`unmoved_reason`): from a fresh
    // probe of what is there now, toward what the manager announces. Without them a retry gave
    // no verdict off a pin, and its exit 0 lifted a hold and journalled a success that had
    // moved nothing.
    let before = if matches!(act, Action::Upgrade | Action::Downgrade) {
        let (step_c, os) = (step.clone(), state.os);
        tokio::task::spawn_blocking(move || detect_present_detailed(&step_c, os))
            .await
            .ok()
    } else {
        None
    };
    // A retried downgrade may find the package already removed by its own first half.
    let act = if act == Action::Downgrade {
        retry_action(act, before.as_ref().and_then(|p| p.present))
    } else {
        act
    };
    // A row-upgrade is only meaningful for an outdated package: a fresh outdated scan says
    // what it runs toward and, for brew, whether it is a cask (blocking → spawn_blocking, like
    // the other sites), ONLY for Upgrade (don't scan on an install/uninstall retry). This
    // makes the retry use the SAME forced-cask path and the same target as the batch Apply.
    let od = if act == Action::Upgrade {
        let os = state.os;
        // An unreadable scan cannot tell cask from formula. Defaulting to false
        // is the safe direction here: the cask path adds --force, and forcing on
        // a guess is worse than not forcing. Nor does it announce anything.
        let scan = tokio::task::spawn_blocking(move || scan_outdated(os))
            .await
            .unwrap_or(Err(crate::outdated::ScanFailure::CommandFailed(
                "the scan task did not finish".into(),
            )))
            .unwrap_or_default();
        outdated_for(step.system_id.as_deref(), &scan).cloned()
    } else {
        None
    };
    // A `self` row's upgrade is the tool's own command, never brew's: not a cask upgrade.
    let is_cask = !is_self_updating(step) && od.as_ref().is_some_and(|o| o.is_cask);
    if cask_upgrade_blocked(act, is_cask, state.appmgmt) {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "needs-appmgmt", "i": i }).to_string(),
            ))
            .await;
        return;
    }
    let aim = Aim {
        before: before.as_ref(),
        announced: od.as_ref().map(|o| o.available.as_str()),
    };
    do_step(socket, state, i, act, step, is_cask, aim).await;
    // A retry is an Apply of ONE step, and what it observed is just as real. So the
    // flush belongs on THIS ending as much as on the batch one.
    //
    // There are two other endings, and neither flushes. `run_in_pty`'s sudo refusal is
    // not an ending at all — the step is still running there and its duration would be
    // wrong. The `quit` handler is a real one, and the omission is safe for a reason
    // worth stating: while an Apply runs, the handler loop is blocked inside `apply_diff`
    // or here, so a `quit` message is consumed by `run_in_pty`'s own `select!` and never
    // reaches that handler. `quit` is therefore only ever HANDLED between Applies, when
    // the accumulator is already empty and there is nothing to flush.
    flush_behaviour(state).await;
    clear_sudo_pw(state).await; // the retry finished: clear the cached password
}

/// Writes a dimmed line into row `i`'s terminal, now — for a line about a command that has
/// ended, which nothing will clear any more.
async fn say(socket: &mut WebSocket, i: usize, line: &str) {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let line = format!("\x1b[2m{line}\x1b[0m\r\n");
    let _ = socket
        .send(Message::Text(
            json!({ "type": "out", "i": i, "data": STANDARD.encode(line.as_bytes()) }).to_string(),
        ))
        .await;
}

/// Queues a line for row `i`'s terminal; do_step writes it after the running status.
async fn queue_note(state: &AppState, i: usize, line: String) {
    state.notes.lock().await.entry(i).or_default().push(line);
}

/// The lines queued for row `i`, emptied.
async fn take_notes(state: &AppState, i: usize) -> Vec<String> {
    state.notes.lock().await.remove(&i).unwrap_or_default()
}

/// A winget version change, rebuilt for where the package must live — or None, and the
/// stored command stands. PURE: `listed` is (name, version) from `winget list` (None = not
/// installed), `found` the scope the uninstall entries show (None = unreadable), `remembered`
/// the scope an earlier probe of this session found. Returns the command and the row's line.
///
/// - Downgrade: the uninstall removes every copy; the reinstall goes to the declared scope,
///   else the one found. Already gone (a retried reinstall half) → only the install: an
///   uninstall of an absent package exits -1978335212, which `benign_code` accepts, and the
///   chain would stop there reading ok with nothing reinstalled (MEASURED).
/// - Upgrade, scope declared: in place only when the copy is already there; otherwise it is
///   removed first. MEASURED: `winget install --scope user` over a machine-wide copy added a
///   per-user one beside it.
/// - Install, nothing declared: the scope remembered from the downgrade that removed it.
fn winget_plan(
    action: crate::decision::Action,
    step: &crate::bundles::Step,
    listed: Option<(&str, &str)>,
    found: Option<crate::managers::InstallScope>,
    remembered: Option<crate::managers::InstallScope>,
) -> Option<(String, String)> {
    use crate::decision::Action;
    use crate::managers::InstallScope;
    if step.route.as_deref() != Some("winget") {
        return None;
    }
    let mgr = crate::managers::native_manager(Os::Windows)?;
    let id = step.system_id.as_deref()?;
    let declared = step.scope;
    let who = |sc: InstallScope| match sc {
        InstallScope::User => "for this user",
        InstallScope::Machine => "for every user",
    };
    let flag = |sc: Option<InstallScope>| match sc {
        Some(sc) => format!(" (--scope {})", sc.as_str()),
        None => " in winget's default scope".to_string(),
    };
    match action {
        Action::Downgrade => {
            let pin = step.pin.as_deref()?;
            let Some((name, installed)) = listed else {
                let sc = declared.or(remembered);
                return Some((
                    mgr.install_pinned_in(id, pin, sc),
                    format!(
                        "{id} is not installed any more; {pin} is installed{}",
                        flag(sc)
                    ),
                ));
            };
            let note = match (declared, found) {
                (Some(sc), _) => format!(
                    "{pin} is reinstalled where the catalogue puts it (--scope {})",
                    sc.as_str()
                ),
                (None, Some(sc)) => format!(
                    "{name} {installed} is installed {}; {pin} is reinstalled there (--scope {})",
                    who(sc),
                    sc.as_str()
                ),
                (None, None) => format!(
                    "where {name} {installed} is installed could not be read; \
                     reinstalling in winget's default scope"
                ),
            };
            Some((mgr.downgrade_in(id, pin, declared.or(found)), note))
        }
        Action::Upgrade => {
            // `self`: the upgrade is the tool's own command, never a winget reinstall.
            if is_self_updating(step) {
                return None;
            }
            let sc = declared?;
            let (name, installed) = listed?;
            if found == Some(sc) {
                return None;
            }
            let up = step.upgrade.as_deref()?;
            let note = match found {
                Some(f) => format!(
                    "{name} {installed} is installed {}; it is moved to --scope {} as it is \
                     upgraded",
                    who(f),
                    sc.as_str()
                ),
                None => format!(
                    "where {name} {installed} is installed could not be read; it is removed \
                     and installed with --scope {} rather than risk a second copy",
                    sc.as_str()
                ),
            };
            Some((format!("{} && {up}", mgr.uninstall(id)), note))
        }
        Action::Install => {
            if declared.is_some() {
                return None;
            }
            let sc = remembered?;
            let cmd = match crate::bundles::classify_pin(step.pin.as_deref()) {
                Some(crate::bundles::PinKind::Exact(p)) => mgr.install_pinned_in(id, &p, Some(sc)),
                _ => mgr.install_in(id, Some(sc)),
            };
            Some((
                cmd,
                format!(
                    "{id} was installed {}; it is installed there again (--scope {})",
                    who(sc),
                    sc.as_str()
                ),
            ))
        }
        Action::Uninstall => None,
    }
}

/// A brew version change against an exact pin, decided from what brew offers — PURE: the
/// verdict comes from `managers::brew_pin_check`, run just before acting.
///
/// - `Ok(Some((command, line)))`: the pin IS brew's current version, so plain brew does it; the
///   stored command looks for `id@pin`, which then exists nowhere (Nushell 0.116.1, 2026-10-22).
/// - `Ok(None)`: the stored command stands — a versioned formula exists, brew could not be
///   read, the pin is not exact, the route is not brew, or the action is a removal.
/// - `Err(reason)`: brew cannot serve the pin. Nothing runs, and the row says what brew offers:
///   a command certain to fail would fail again on every Apply.
fn brew_plan(
    action: crate::decision::Action,
    step: &crate::bundles::Step,
    verdict: Option<&crate::managers::BrewPin>,
) -> Result<Option<(String, String)>, String> {
    use crate::decision::Action;
    use crate::managers::BrewPin;
    if step.route.as_deref() != Some("brew") || action == Action::Uninstall {
        return Ok(None);
    }
    let Some(crate::bundles::PinKind::Exact(pin)) =
        crate::bundles::classify_pin(step.pin.as_deref())
    else {
        return Ok(None);
    };
    let (Some(id), Some(mgr)) = (
        step.system_id.as_deref(),
        crate::managers::managers()
            .into_iter()
            .find(|m| m.route == "brew"),
    ) else {
        return Ok(None);
    };
    match verdict {
        Some(BrewPin::Current) => {
            // `install` for an upgrade too, as the stored `id@ver` form: it upgrades an outdated
            // formula, and `brew upgrade` refuses one brew did not install itself.
            let cmd = match action {
                Action::Downgrade => format!("{} && {}", mgr.uninstall(id), mgr.install(id)),
                _ => mgr.install(id),
            };
            Ok(Some((
                cmd,
                format!("{pin} is brew's current {id}: plain brew, no versioned formula"),
            )))
        }
        Some(BrewPin::Unreachable { offered }) => Err(format!("brew offers {offered}, not {pin}")),
        Some(BrewPin::Versioned) | None => Ok(None),
    }
}

/// Asks brew what it offers for a row's exact pin, only when `brew_plan` needs the answer: a
/// brew route, an exact pin, an action that installs. One `brew info`, two when the pin is not
/// brew's current version. Never in the scan: an Apply pays it, for its pinned brew rows only.
async fn ask_brew(
    action: crate::decision::Action,
    step: &crate::bundles::Step,
    os: Os,
) -> Option<crate::managers::BrewPin> {
    if step.route.as_deref() != Some("brew") || action == crate::decision::Action::Uninstall {
        return None;
    }
    let Some(crate::bundles::PinKind::Exact(pin)) =
        crate::bundles::classify_pin(step.pin.as_deref())
    else {
        return None;
    };
    let id = step.system_id.clone()?;
    tokio::task::spawn_blocking(move || {
        crate::managers::brew_pin_check(&id, &pin, &|cmd| {
            let r = crate::detect::run_probe_detailed(&crate::platform::shell_probe(os, cmd));
            (r.ok, r.output)
        })
    })
    .await
    .ok()?
}

/// Rows held absent at plan time (brew cannot serve the pin) will not be there after the
/// Apply, so what requires them cannot run either: it leaves the plan, saying what it waits
/// for, and a chain goes with it. `nodes` was built before any hold was known. A removal stays:
/// it never needs its requirement.
fn drop_requiring_held(
    plan: &mut Vec<(usize, crate::decision::Action)>,
    nodes: &mut [crate::deps::DepNode],
    idx: &std::collections::HashMap<String, usize>,
    held_absent: &[usize],
) -> Vec<(usize, String)> {
    for &h in held_absent {
        nodes[h].will_be_present = false;
    }
    let mut dropped = Vec::new();
    loop {
        let blocked = plan.iter().position(|&(i, a)| {
            a != crate::decision::Action::Uninstall
                && crate::deps::requires_blocked(&nodes[i], nodes, idx).is_some()
        });
        let Some(at) = blocked else { break };
        let (i, _) = plan.remove(at);
        let why = crate::deps::requires_blocked(&nodes[i], nodes, idx).unwrap_or_default();
        if !nodes[i].present_now {
            nodes[i].will_be_present = false;
        }
        dropped.push((i, why));
    }
    dropped.sort();
    dropped
}

/// The reason a removal the Apply button did not name stays undone (`drop_unannounced`).
const UNANNOUNCED_REMOVAL: &str = "not announced before the click — Apply again to run it";

/// The removals the button did not announce leave the plan, and are returned for their trace.
/// A removal is a destructive act, and the doctrine is that it is named on the button before
/// the click; the front sends the rows whose uninstall or reinstall it counted (`removals`).
/// The two sides decide with one rule but from facts read at two moments: the scan the button
/// counted from, and the re-probe here. When they disagree on a removal, the button's count is
/// what the user consented to, so the plan is cut down to it. The row's facts are repainted
/// before the plan, so the next click counts the removal and runs it.
///
/// `None` (a front that sends no list) keeps the plan whole. Installs and updates are never
/// cut: nothing is lost if one was not counted.
fn drop_unannounced(
    plan: &mut Vec<(usize, crate::decision::Action)>,
    announced: Option<&std::collections::HashSet<usize>>,
) -> Vec<usize> {
    use crate::decision::Action;
    let Some(announced) = announced else {
        return Vec::new();
    };
    let mut dropped = Vec::new();
    plan.retain(|&(i, a)| {
        let removal = matches!(a, Action::Uninstall | Action::Downgrade);
        if removal && !announced.contains(&i) {
            dropped.push(i);
            return false;
        }
        true
    });
    dropped
}

/// Tells the front that the Apply left a removal its button did not name. Not a `held`: the
/// front keeps a hold until the next plan, and this one answers ONE click — the repaint before
/// the plan already gave the front the facts, so its next count names the removal.
async fn send_unannounced(socket: &mut WebSocket, i: usize) {
    let _ = socket
        .send(Message::Text(
            json!({ "type": "unannounced", "i": i, "reason": UNANNOUNCED_REMOVAL }).to_string(),
        ))
        .await;
}

/// Tells the front that the Apply leaves row `i` and why. The front keeps it as the row's hold
/// until the next plan (a Refresh), so the Apply button stops offering a change brew cannot
/// make. The server keeps nothing: every Apply that wants the row asks brew again.
async fn send_held(socket: &mut WebSocket, i: usize, why: &str) {
    let _ = socket
        .send(Message::Text(
            json!({ "type": "held", "i": i, "reason": why }).to_string(),
        ))
        .await;
}

/// The probes behind `winget_plan`, run just before acting: where the installed copy is is a
/// fact of the machine, which the command stored at load cannot hold. Off winget, or when no
/// decision needs them, nothing runs. The scope found is remembered for the row.
async fn winget_in_place(
    state: &AppState,
    action: crate::decision::Action,
    step: &crate::bundles::Step,
    os: Os,
) -> Option<(String, String)> {
    use crate::decision::Action;
    use crate::managers::{arp_probe_command, scope_from_arp, winget_listing, Listing};
    if os != Os::Windows || step.route.as_deref() != Some("winget") {
        return None;
    }
    let remembered = state.found_scope.lock().await.get(&step.id).copied();
    let probes = match action {
        Action::Downgrade => step.pin.is_some(),
        // A `self` row upgrades by its own `update:`, never by a rebuilt winget line.
        Action::Upgrade => step.scope.is_some() && !is_self_updating(step),
        _ => false,
    };
    let (listed, found) = if probes {
        let id = step.system_id.clone()?;
        let mgr = crate::managers::native_manager(os)?;
        tokio::task::spawn_blocking(move || {
            let probe = |cmd: &str| {
                crate::detect::run_probe_detailed(&crate::platform::shell_probe(os, cmd))
            };
            // Absent only when winget SAYS so: a failed or unreadable listing is unknown,
            // and unknown leaves the stored command alone (outer None).
            let (name, installed) =
                match winget_listing(&id, &probe(&mgr.presence_command(&id)).output) {
                    Listing::Installed { name, version } => (name, version),
                    Listing::Absent => return Some((None, None)),
                    Listing::Unknown => return None,
                };
            let arp = probe(&arp_probe_command(&name));
            let found = if arp.ok {
                scope_from_arp(&arp.output, &name, &installed)
            } else {
                None
            };
            Some((Some((name, installed)), found))
        })
        .await
        .ok()??
    } else {
        (None, None)
    };
    if let Some(sc) = found {
        state.found_scope.lock().await.insert(step.id.clone(), sc);
    }
    let listed = listed.as_ref().map(|(n, v)| (n.as_str(), v.as_str()));
    winget_plan(action, step, listed, found, remembered)
}

/// Decide whether an outdated row is worth re-probing. A row is outdated AND an upgrade
/// is reachable when:
/// - The pin is `latest` (or absent): `action_for` will return `Upgrade`.
/// - The pin is an exact version: `action_for` ignores `outdated` entirely; the row seeds
///   via the `pinned` path instead, so this path is irrelevant (but harmless if true).
/// - ⚠️ The pin is `pending`: `action_for` returns `None` for a present pending row, so
///   being outdated produces no action at all. Must return `false` to avoid spending a
///   process on a row that will do nothing.
///
/// ⭐ Measurement: 4 rows on this machine right now (AWS CLI, Obsidian, uv, VS Code) are
/// outdated + pending, so all four would be seeded for nothing. That number grows as the
/// estate drifts — up to 24 in the worst case. The Apply re-scan is serial on purpose, so
/// each one is a real process.
///
/// ⭐ The seventh of a family: every guard written when "a pin is a number" or "outdated ⇒
/// upgrade" was always true has silently inherited the keyword pins.
fn should_seed_outdated_row(pin: Option<&str>, outdated: bool) -> bool {
    if !outdated {
        return false;
    }
    // If there is a pin, classify it to see if an upgrade is reachable.
    if let Some(kind) = crate::bundles::classify_pin(pin) {
        match kind {
            // `pending` → held out of the batch, `action_for` returns None → no upgrade.
            crate::bundles::PinKind::Pending => false,
            // `latest` behaves as declaring nothing → outdated DOES mean upgrade.
            crate::bundles::PinKind::Latest => true,
            // `self` with `update:` → the tool's own command may run; the caller passes
            // `outdated` false for a row with no upgrade command (`self` without `update:`).
            crate::bundles::PinKind::SelfUpdating => true,
            // Exact pin → `action_for` ignores `outdated`, seeds via `is_pinned_row` instead.
            // Returning `true` here is harmless (the row seeds anyway), but the outdated path
            // is irrelevant for exact pins.
            crate::bundles::PinKind::Exact(_) => true,
            // Invalid → treat as `latest` (the safer default: probe rather than skip).
            crate::bundles::PinKind::Invalid(_) => true,
        }
    } else {
        // No pin declared → behaves as `latest` → outdated means upgrade.
        true
    }
}

/// WHICH rows the Apply re-scan must actually probe — the seeds, before `requires`
/// pulls transitively.
///
/// The front sends EVERY index (model A: every package has a desired state, so
/// `on ∪ off` is the whole catalogue on every Apply — see applyScoped in app.js).
/// That contract is right: "send every desired state, the server converges" is what
/// makes the model declarative. So the NARROWING belongs here, not in the UI.
///
/// A row is a seed when something could actually happen to it:
///   · its desired state differs from the presence we last observed
///   · we have NEVER observed it (`None`, or past the end of the vector) — unknown
///     is not "satisfied"; nobody looked
///   · its observed presence is indeterminate (`present: None`) — same reason
///   · it is flagged OUTDATED by the batched scan — desired present + observed
///     present is still an Upgrade, which is an action
///   · it is PINNED and wanted present — see below
///
/// A pinned row is always a seed, and that needs saying because it is the one case
/// presence cannot answer. A pin REPLACES "latest" as the reference, so `action_for`
/// ignores the machine-wide `outdated` flag entirely when a pin is set. What decides
/// is the installed VERSION against the pin. The pin cannot go stale (it is a literal
/// in the YAML); the remembered version can. Upgrade a pinned package by hand between
/// the connect scan and an Apply and we would answer "satisfied" against the old
/// number — and in the other direction a remembered version above the pin yields
/// Downgrade, the only destructive path (uninstall + reinstall), decided on data
/// nobody re-checked.
///
/// ⚠️ The trade-off, stated rather than hidden: a row that is present, wanted
/// present and current is no longer re-probed, so a MANUAL UNINSTALL between the
/// connect scan and the Apply stops being caught for that row. That is the literal
/// price of "check only what needs doing". Refresh still re-scans everything, and
/// the row's own button re-probes after acting. The real answer is BATCH (one
/// `winget list`/`brew list` for the whole machine, parsed in memory): then
/// re-probing everything costs ~2 s and the trade-off dissolves. Do NOT
/// re-parallelise the probes — that rule is unchanged.
fn seeds_for_rescan(
    on: &[usize],
    off: &[usize],
    last_seen: &[Option<crate::detect::Presence>],
    outdated: &dyn Fn(usize) -> bool,
    pinned: &dyn Fn(usize) -> bool,
) -> Vec<usize> {
    let could_act = |i: usize, want_present: bool| -> bool {
        // A pin is decided by the installed VERSION, which presence cannot report.
        // Only for want_present: an uninstall does not care what version is there,
        // and that case is already a seed via the desire mismatch.
        if want_present && pinned(i) {
            return true;
        }
        match last_seen.get(i).and_then(|p| p.as_ref()) {
            // Never observed, or observed indeterminate → we don't know, so look.
            None => true,
            Some(p) => match p.present {
                None => true,
                // Presence already matches the desire: only an upgrade is left to do.
                Some(present) => present != want_present || (want_present && outdated(i)),
            },
        }
    };
    let mut seeds: Vec<usize> = on
        .iter()
        .filter(|&&i| could_act(i, true))
        .chain(off.iter().filter(|&&i| could_act(i, false)))
        .copied()
        .collect();
    seeds.sort_unstable();
    seeds.dedup();
    seeds
}

/// How long the machine-wide outdated map read by a scan stays good enough for the Apply.
///
/// Ten minutes because that is how long people really spend between the scan and Apply —
/// reading, scrolling, choosing (the operator's call, 2026-10-08). Measured on a Windows 11
/// VM the same day: `winget upgrade` costs 3.3 s cold and 0.65 s while winget's own source
/// cache is warm, which lasts five minutes by default. So past five minutes the scan this
/// skips is the COLD one, the network refresh — the very part that drags behind a corporate
/// firewall. The price of the longer window is unchanged in kind: an upgrade published or
/// applied outside Talos in the meantime, worst case one step answered "already current".
const OUTDATED_FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(600);

/// The outdated map a scan read, and when.
///
/// ⚠️ A TTL on OUTDATED-NESS, never on presence — "No TTL cache on detection" stands. What
/// can change inside the window is an upgrade done OUTSIDE Talos, and its worst case is one
/// upgrade step the manager answers "already current". What Talos does itself clears it —
/// every step (`do_step`) and every rescue terminal (`doctor-start`) — so the window never
/// outlives an action of ours; a Refresh replaces it.
struct OutdatedSeen {
    at: std::time::Instant,
    map: std::collections::HashMap<String, crate::managers::Outdated>,
}

/// The recorded outdated map if it is still fresh at `now`, else `None` (scan again).
fn reusable_outdated(
    seen: Option<&OutdatedSeen>,
    now: std::time::Instant,
) -> Option<&OutdatedSeen> {
    let seen = seen?;
    let age = now.checked_duration_since(seen.at)?;
    (age < OUTDATED_FRESH_FOR).then_some(seen)
}

/// The heart: applies the tri-state decision against the machine reality.
///   on  = indices wanted PRESENT; off = indices wanted ABSENT.
/// Re-detects presence NOW (repaint-at-apply: re-observes before acting,
/// does not trust the connection scan), repaints the pills, asks the
/// SHARED rule action_for what to do, orders by dependencies (topo_sort), emits
/// `apply-plan` (⚠️ which REMOVES the "Plotting the gallop…" veil — shortcut #1 from
/// the spike fixed), then runs each step.
async fn apply_diff(
    socket: &mut WebSocket,
    state: &AppState,
    on: Vec<usize>,
    off: Vec<usize>,
    unmanaged: Vec<usize>,
    announced: Option<Vec<usize>>,
) {
    use crate::decision::{action_for, Action, Desired, MachineFacts};
    use crate::deps::{
        index_of_names, make_index, requires_blocked, requires_reason, topo_sort, DepNode,
    };
    use std::collections::HashSet;

    let os = state.os;
    // Snapshot: an Apply must act on the plan it DECIDED from, start to finish. This is
    // the site where a mid-flight reload would be worst — `last_seen` is positional and
    // the front sends indices, so a plan that grew between the decision and the act would
    // aim a gesture at the wrong package.
    let steps = state.plan.read().await.steps.clone();
    let steps = &steps;
    let want_on: HashSet<usize> = on.iter().copied().collect();
    let want_off: HashSet<usize> = off.iter().copied().collect();
    let out_of_scope: HashSet<usize> = unmanaged.iter().copied().collect();
    let announced: Option<HashSet<usize>> = announced.map(|v| v.into_iter().collect());

    // The batched outdated scan runs FIRST, because the seed selection needs it: a
    // row that is present and wanted present is still an ACTION when it is behind.
    // It is ONE command for the whole machine (~1-2 s), and naming it matters — the
    // veil would otherwise open on a frozen phrase for exactly as long as the wait
    // people complained about. No `total` yet (the count belongs to the probes), so
    // the bar stays indeterminate here.
    let started = std::time::Instant::now();
    // ⭐ Reused when the scan before this Apply read it less than OUTDATED_FRESH_FOR ago and
    // no step has acted since — the usual case: people connect, choose, press Apply. The
    // presence of the rows about to be acted on is still re-probed below, live.
    let fresh = reusable_outdated(state.outdated_seen.lock().await.as_ref(), started)
        .map(|s| (s.map.clone(), s.at));
    let scan = if let Some((map, at)) = fresh {
        println!(
            "[apply] outdated reused from the scan {:?} ago ({} entries)",
            started.duration_since(at),
            map.len()
        );
        map
    } else {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "rescan-progress", "name": "what's out of date" }).to_string(),
            ))
            .await;
        // If the scan is unreadable, `outdated(i)` is false for every row — which
        // would NARROW the re-probe set on an assumption. seeds_for_rescan already
        // re-probes anything it has no reliable knowledge of, so the safe reading of
        // a failed scan is an empty map, never a claim.
        let scanned = tokio::task::spawn_blocking(move || scan_outdated(os))
            .await
            .unwrap_or(Err(crate::outdated::ScanFailure::CommandFailed(
                "the scan task did not finish".into(),
            )));
        println!("[apply] outdated (batched) in {:?}", started.elapsed());
        match scanned {
            Ok(map) => {
                *state.outdated_seen.lock().await = Some(OutdatedSeen {
                    at: std::time::Instant::now(),
                    map: map.clone(),
                });
                map
            }
            Err(reason) => {
                eprintln!("[apply] outdated UNAVAILABLE: {}", reason.message());
                let _ = socket
                    .send(Message::Text(
                        json!({ "type": "outdated-unavailable", "reason": reason.message() })
                            .to_string(),
                    ))
                    .await;
                std::collections::HashMap::new()
            }
        }
    };

    // WHAT to re-probe: the rows where something COULD happen, plus what their
    // `requires` pull, transitively. Not all 30.
    //
    // ⚠️ The subtlety that made the first version of this a no-op in production: the
    // front sends EVERY index (model A — every package has a desired state), so
    // `on ∪ off` IS the whole catalogue and using it directly as the seeds scoped
    // nothing at all. seeds_for_rescan does the narrowing here, server-side, against
    // `last_seen` + the outdated scan; the front's "send everything, the server
    // converges" contract stays untouched, because that contract is what makes the
    // model declarative.
    //
    // The repaint-at-apply guarantee (re-observe before acting) is only MEANINGFUL
    // for rows we are about to touch: probing Miro to install Bun buys nothing and
    // costs a process. Requirements are in, because will_be_present decides both the
    // `requires` reasons and the execution order. Dependents are out — untouched.
    //
    // ⚠️ An outdated row is only a seed when an upgrade is actually reachable. A `pending`
    // pin makes it unreachable (`action_for` returns None), so being outdated produces no
    // action at all. Measured: 4 rows here (AWS CLI, Obsidian, uv, VS Code) are outdated +
    // pending, so all four would be seeded for nothing — up to 24 as the estate drifts.
    // The seventh of a family: every guard written when "outdated ⇒ upgrade" was always
    // true has silently inherited the keyword pins.
    let is_outdated_row = |i: usize| -> bool {
        steps.get(i).is_some_and(|s| {
            // A row with no upgrade command cannot be upgraded however far behind it is: a
            // `self` row without `update:` (VS Code, Orca) is left to the application.
            let outdated =
                is_outdated_now(outdated_for(s.system_id.as_deref(), &scan)) && s.upgrade.is_some();
            should_seed_outdated_row(s.pin.as_deref(), outdated)
        })
    };
    // An EXACT pin makes the installed VERSION the deciding fact, and presence cannot
    // report it — so an exact-pinned row is always probed. Keywords (pending, latest)
    // do NOT make version the deciding fact, so they must NOT seed — seeding them
    // would re-introduce the serial scan cost the batching work removed. `trim`
    // reasoning stays (the pin is author-written YAML), but is now inside classify_pin.
    let is_pinned_row = |i: usize| -> bool { steps.get(i).is_some_and(always_probed) };
    // The guard is BOUND, not passed as a temporary: a temporary would live to the end
    // of the statement while the closures run, which is one refactor away from a
    // self-deadlock if a closure ever needs `state`.
    let seeds = {
        let seen = state.last_seen.lock().await;
        seeds_for_rescan(&on, &off, &seen, &is_outdated_row, &is_pinned_row)
    };
    let name_idx = index_of_names(steps.iter().map(|s| s.name.as_str()));
    let requires: Vec<Vec<String>> = steps.iter().map(|s| s.requires.clone()).collect();
    let scope = crate::deps::rescan_scope(&requires, &name_idx, &seeds);
    // Catalogue order, so the narration counts up the screen and not at random
    // (a HashSet iterates in whatever order it likes).
    let mut to_probe: Vec<usize> = scope.into_iter().collect();
    to_probe.sort_unstable();

    // SERIAL live re-scan (presence).
    // ⚠️ Serial on purpose — same reason as scan_and_emit: concurrent winget probes
    // contend over winget's shared state. Do not fan this back out. If the scoped
    // scan is ever still too slow, the fix is BATCH (one list command), never threads.
    let mut probed: std::collections::HashMap<usize, crate::detect::Presence> =
        std::collections::HashMap::with_capacity(to_probe.len());
    // ⭐ The batched listing pays only above a few rows, and this scan is SCOPED to
    // the diff — measured at 0-2 packages on a converged machine
    // (`talos-apply-rescan-scoped-to-diff`). One `winget list` costs ~3.7 s where a
    // per-package probe costs ~1.3 s, so break-even is around three: below it the
    // listing is a LOSS, and the fast path exists to be fast.
    let bulk = if to_probe.len() >= BULK_PRESENCE_WORTH_IT {
        std::sync::Arc::new(
            tokio::task::spawn_blocking(move || crate::detect::fetch_bulk_presence(os))
                .await
                .unwrap_or(None),
        )
    } else {
        std::sync::Arc::new(None)
    };
    // ⭐ NO count threshold for this one, unlike the bulk listing right above: without it every
    // extension row in the diff reads unknown, which is precisely the row Apply is about to act
    // on. But it is taken only when such a row IS in the diff — only `vscode-extension` rows read
    // it, and it is not free: one `code --version` plus one file read, ~0.2 s on a Mac but 0.74 s
    // measured on Windows (2026-10-08), paid on every Apply that touches no extension at all.
    let needs_vscode = to_probe
        .iter()
        .any(|&i| steps[i].route.as_deref() == Some("vscode-extension"));
    let vs = std::sync::Arc::new(if needs_vscode {
        let home = crate::detect::user_home();
        tokio::task::spawn_blocking(move || crate::vscode::snapshot(os, &home))
            .await
            .ok()
    } else {
        None
    });
    for (nth, &i) in to_probe.iter().enumerate() {
        // Narrate BEFORE probing: the veil says which package is being checked while
        // it is being checked (`nth of total`) — the same honesty the splash got, now
        // on the wait that outlived it. Sent first, because a probe can take seconds
        // and a name announced after the fact describes the past.
        let _ = socket
            .send(Message::Text(
                json!({
                    "type": "rescan-progress", "i": i, "name": steps[i].name,
                    "nth": nth + 1, "total": to_probe.len()
                })
                .to_string(),
            ))
            .await;
        let step_owned = steps[i].clone();
        let bulk_c = std::sync::Arc::clone(&bulk);
        let vs_c = std::sync::Arc::clone(&vs);
        let p = tokio::task::spawn_blocking(move || {
            crate::detect::detect_present_detailed_with_vscode(
                &step_owned,
                os,
                bulk_c.as_ref().as_ref(),
                vs_c.as_ref().as_ref(),
            )
        })
        .await
        .unwrap_or_default();
        remember_presence(state, i, p.clone()).await;
        probed.insert(i, p);
    }
    println!(
        "[apply] re-scan TOTAL {:?} for {}/{} packages (scoped to the diff)",
        started.elapsed(),
        to_probe.len(),
        steps.len()
    );
    // Un-probed rows keep their last OBSERVED presence — never an invented `false`.
    //
    // Note the invariant that makes this safe rather than merely better: every fact a
    // DECISION rests on is fresh. `requires_reason` runs only over the probed rows, and
    // it reads the requirements of those rows — which rescan_scope pulled in, so they
    // are probed too. `topo_sort` only draws edges between packages IN the plan, and the
    // plan is a subset of the diff. The remembered values fill the vector so the indices
    // line up; they are not what the decisions rest on.
    //
    // ⚠️ That claim held only because seeds_for_rescan covers every fact `action_for`
    // reads — and it once did NOT: a PINNED row reads the installed VERSION, which
    // presence cannot report, and was excluded when its presence matched the desire. So
    // an upgrade/downgrade against the pin was decided on a remembered version. Fixed by
    // the pin clause in seeds_for_rescan. If a future fact joins MachineFacts, it must
    // join the seed predicate too, or this invariant quietly becomes false again.
    let presences = merge_presences(steps.len(), &probed, &state.last_seen.lock().await);

    // Future state per package: present now OR wanted-on, never if wanted-off.
    let mut nodes: Vec<DepNode> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| DepNode {
            name: s.name.clone(),
            requires: s.requires.clone(),
            will_be_present: !want_off.contains(&i)
                && (presences[i].present == Some(true) || want_on.contains(&i)),
            // The two facts `will_be_present` folds away, kept apart because
            // `requires_blocked` needs them apart. `present_now` is the observation with
            // no desire mixed in; `reachable` is "Talos would act on this row at all",
            // which is `uninstall.is_some()` — the very bit the front sends as
            // `canUninstall`, so both halves of the rule read the same predicate.
            present_now: presences[i].present == Some(true),
            reachable: s.uninstall.is_some(),
        })
        .collect();
    let idx = make_index(&nodes);

    // Repaint the pills BEFORE acting (repaint-at-apply) — for the RE-PROBED rows only.
    // The others were not looked at, so there is nothing new to say about them; the row
    // the front already shows is the last thing we actually observed. (`state` is
    // per-index and idempotent — the front applies whichever it receives, in any number.)
    for &i in &to_probe {
        let p = &presences[i];
        let reason = requires_reason(&nodes[i], &nodes, &idx).or_else(|| p.reason.clone());
        let _ = socket
            .send(Message::Text(
                json!({
                    "type": "state", "i": i, "present": p.present, "reason": reason,
                    "version": p.version.clone().unwrap_or_default(), "external": p.external,
                    "probe": probe_json(&p.diag)
                })
                .to_string(),
            ))
            .await;
        if p.present == Some(true) {
            let od = outdated_for(steps[i].system_id.as_deref(), &scan);
            emit_outdated_if(socket, i, od).await;
        }
    }

    // Action per package: desire (on→present, off→absent, neither→auto=None).
    let mut visual_plan: Vec<(usize, Action)> = Vec::new();
    // What brew offered belongs to this Apply: an answer left by a stopped one is not reused.
    state.brew_pins.lock().await.clear();
    let mut held_absent: Vec<usize> = Vec::new();
    let sel = read_selection(&state.data_dir);
    for (i, step) in steps.iter().enumerate() {
        // Out of scope → never an action. TWO sources, deliberately, because they
        // know different halves of the rule:
        //   · the front's `unmanaged` list carries the DERIVED half (`external`,
        //     `!canUninstall`) — live facts the server does not hold;
        //   · the persisted overrides carry the user's EXPLICIT half, and reading
        //     them here gives the batch path the same teeth `retry_attempt` has.
        // Without the second, a client that simply omitted the list would still get
        // its uninstall: the batch path would trust the wire where the row path does
        // not. Same predicate, same disk, one asymmetry closed.
        //   · and the REQUIREMENTS, which only the server can answer: it holds the whole
        //     graph and it has just re-probed it, so it does not depend on the wire being
        //     honest here at all. This is the half that was computed and never enforced —
        //     `requires_reason` fed the row's TEXT (below, at the repaint) and nothing
        //     else, so a Microsoft redistributable on a Mac kept an install button and
        //     counted against its bundle.
        if out_of_scope.contains(&i)
            || scope_refuses(&sel, &step.name)
            || requires_blocked(&nodes[i], &nodes, &idx).is_some()
        {
            continue;
        }
        let desired = if want_on.contains(&i) {
            Some(Desired::Present)
        } else if want_off.contains(&i) {
            Some(Desired::Absent)
        } else {
            None // auto → never touched
        };
        let Some(desired) = desired else { continue };
        let od = outdated_for(step.system_id.as_deref(), &scan);
        let facts = MachineFacts {
            present: presences[i].present == Some(true),
            outdated: is_outdated_now(od),
            can_uninstall: step.uninstall.is_some(),
            pin: step.pin.as_deref(),
            installed_version: presences[i].version.as_deref().unwrap_or(""),
            available: od.map(|o| o.available.as_str()).unwrap_or(""),
            can_self_update: is_self_updating(step) && step.upgrade.is_some(),
            update_each_apply: step.update_each_apply,
            updated_this_session: updated_this_session(state, step),
        };
        match batch_action(action_for(desired, &facts), step.downgrade.is_some()) {
            Ok(Some(a)) => {
                // A change that already ran toward this target in this session and moved
                // nothing is not run again: the row says why, and a new target lifts the hold.
                if matches!(a, Action::Upgrade | Action::Downgrade) {
                    let announced = od.map(|o| o.available.as_str());
                    let installed = presences[i].version.as_deref();
                    if let Some(u) = unmoved_in_view(state, step, a, installed, announced).await {
                        println!("[apply] {} held: {}", step.name, u.reason);
                        emit_unmoved(socket, i, &u).await;
                        continue;
                    }
                }
                // A brew pin is checked against what brew offers NOW: held when brew cannot
                // serve it, rebuilt by the step when it is brew's current version.
                let verdict = ask_brew(a, step, os).await;
                if let Err(why) = brew_plan(a, step, verdict.as_ref()) {
                    println!("[apply] {} held: {why}", step.name);
                    send_held(socket, i, &why).await;
                    if !nodes[i].present_now {
                        held_absent.push(i);
                    }
                    continue;
                }
                state
                    .brew_pins
                    .lock()
                    .await
                    .insert(step.id.clone(), verdict);
                visual_plan.push((i, a));
            }
            Ok(None) => {}
            // The front already shows this row's reason (it reads `canDowngrade`), so the
            // trace here is the log line — the row is not acted on, and it says why.
            Err(why) => println!("[apply] {} held: {why}", step.name),
        }
    }
    for i in drop_unannounced(&mut visual_plan, announced.as_ref()) {
        state.brew_pins.lock().await.remove(&steps[i].id);
        println!("[apply] {} held: {UNANNOUNCED_REMOVAL}", steps[i].name);
        send_unannounced(socket, i).await;
    }
    for (i, why) in drop_requiring_held(&mut visual_plan, &mut nodes, &idx, &held_absent) {
        state.brew_pins.lock().await.remove(&steps[i].id);
        println!("[apply] {} held: {why}", steps[i].name);
        send_held(socket, i, &why).await;
    }
    // Order by dependencies (required before dependents; visual order = tie-break).
    let plan = topo_sort(&visual_plan, &nodes, &idx);
    if plan.is_empty() {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "done", "nothing": true }).to_string(),
            ))
            .await;
        return;
    }
    // Announce the WHOLE plan in execution order → the front enters focus-mode and
    // REMOVES the "Plotting the gallop…" veil (shortcut #1 fixed).
    let plan_json: Vec<_> = plan
        .iter()
        .map(|(i, a)| json!({ "i": i, "action": a.as_str() }))
        .collect();
    let _ = socket
        .send(Message::Text(
            json!({ "type": "apply-plan", "plan": plan_json }).to_string(),
        ))
        .await;

    // Each dependent is re-checked at most ONCE per Apply, whatever number of its requirements
    // change: half of the bound (`dependents_of` being direct is the other half).
    let mut rechecked: HashSet<usize> = HashSet::new();
    'plan: for (pos, (i, action)) in plan.iter().enumerate() {
        // is_cask from the scan we already have → do_step re-issues the forced
        // --cask --force command for cask upgrades (single resolution point).
        // A `self` row's upgrade is the tool's own command, never brew's: no cask upgrade, so
        // neither the forced command nor the App Management gate applies to it.
        let is_cask = !is_self_updating(&steps[*i])
            && outdated_for(steps[*i].system_id.as_deref(), &scan)
                .map(|o| o.is_cask)
                .unwrap_or(false);
        if cask_upgrade_blocked(*action, is_cask, state.appmgmt) {
            let _ = socket
                .send(Message::Text(
                    json!({ "type": "needs-appmgmt", "i": *i }).to_string(),
                ))
                .await;
            continue;
        }
        let announced =
            outdated_for(steps[*i].system_id.as_deref(), &scan).map(|o| o.available.clone());
        let aim = Aim {
            before: Some(&presences[*i]),
            announced: announced.as_deref(),
        };
        let (outcome, stop_all) =
            step_with_forbidden_pause(socket, state, *i, *action, &steps[*i], is_cask, aim).await;
        if stop_all {
            break; // "stop everything" → abandon the rest of the plan
        }

        // ⭐ THE DEPENDENTS PASS. The re-scan above left dependents out — "untouched, presence
        // unchanged" — and that is false for one family at least: upgrading Nushell kills the
        // plugins registered against the old one (a deleted Cellar, a binary built for the
        // previous minor) without touching them. Measured 2026-10-18: Nushell 0.113 → 0.116
        // took TWO Applies, the first upgrading nu while its plugins, probed before it, read
        // healthy. So once a step has CHANGED its package, its direct dependents are probed
        // again, and the ones the user wants present and that now read absent are installed
        // here, in this Apply, right after the requirement they depend on.
        //
        // ⚠️ Triggered by a change OBSERVED (`step_changed_package`), not by a step having run:
        // a step that changed nothing — winget's "already current" — re-checks nothing.
        //
        // Consent: only rows wanted PRESENT (on), which were already present and chosen; this
        // repairs what the user chose, it adds nothing. Auto and off rows are re-probed and
        // repainted (detect, don't remember) and never acted on. Every gate the plan passed —
        // scope, requirements — is passed again.
        if !step_changed_package(Some(&presences[*i]), outcome.after.as_ref()) {
            continue;
        }
        let still_planned: HashSet<usize> = plan[pos + 1..].iter().map(|(j, _)| *j).collect();
        for d in dependents_to_recheck(&requires, &steps[*i].name, &still_planned, &mut rechecked) {
            let step_c = steps[d].clone();
            let p = tokio::task::spawn_blocking(move || detect_present_detailed(&step_c, os))
                .await
                .unwrap_or_default();
            remember_presence(state, d, p.clone()).await;
            let _ = socket
                .send(Message::Text(
                    json!({
                        "type": "state", "i": d, "present": p.present, "reason": p.reason,
                        "version": p.version.clone().unwrap_or_default(), "external": p.external,
                        "probe": probe_json(&p.diag)
                    })
                    .to_string(),
                ))
                .await;
            let repair = want_on.contains(&d)
                && !out_of_scope.contains(&d)
                && !scope_refuses(&sel, &steps[d].name)
                && requires_blocked(&nodes[d], &nodes, &idx).is_none()
                && p.present == Some(false);
            if !repair {
                continue;
            }
            println!(
                "[apply] {} changed; {} no longer reads present → install again",
                steps[*i].name, steps[d].name
            );
            // The row joins the plan on screen (focus mode hides what is not in it), and its
            // terminal says WHY before the install streams — a gesture leaves a trace.
            let _ = socket
                .send(Message::Text(
                    json!({ "type": "apply-plan-add", "i": d, "action": "install" }).to_string(),
                ))
                .await;
            // Queued, not sent: the install's running status resets the row's terminal, so a
            // line sent now would be wiped as it was written. do_step writes it after.
            let why = format!(
                "[repair] {} changed in this Apply ({} → {}); this package no longer reads \
                 present, so it is installed again.",
                steps[*i].name,
                shown_version(&presences[*i]),
                shown_version(outcome.after.as_ref().unwrap_or(&presences[*i])),
            );
            if steps[d].install.is_some() {
                queue_note(state, d, why).await;
            }
            // A repair does not re-check ITS dependents: one extra pass, never a cascade.
            let (_, stop_all) = step_with_forbidden_pause(
                socket,
                state,
                d,
                Action::Install,
                &steps[d],
                false,
                Aim::default(),
            )
            .await;
            if stop_all {
                break 'plan;
            }
        }
    }
    // ONE write per Apply, here at the end, for the packages it touched. Reached on every
    // way out of the loop above — a normal finish, a "stop everything", and a run whose
    // rows were all cancelled: those `break`s leave the loop, not this function.
    //
    // The `done/nothing` early return above DOES skip it, and so do all four of
    // retry_attempt's guards: unknown index, out of scope, an unknown action verb (reachable
    // — `retry-step` forwards an arbitrary wire string), and needs-appmgmt. Every one of
    // them returns BEFORE any do_step, so there is nothing accumulated to lose — but that
    // is WHY they are safe, not an accident to rely on: a future early exit placed AFTER a
    // do_step would drop that step's facts silently.
    flush_behaviour(state).await;
    state.brew_pins.lock().await.clear(); // rows the Apply did not reach leave no answer behind
    clear_sudo_pw(state).await; // end of Apply: the cached password is cleared (model C)
    let _ = socket
        .send(Message::Text(json!({ "type": "done" }).to_string()))
        .await;
}

/// What ONE pty run produced. NAMED rather than a tuple because three of its five fields
/// are `bool`: as a `(i32, bool, Option<String>, bool, bool)` — which this was — swapping
/// `cancelled` and `saw_window` at the return or at either destructure compiled silently,
/// mislabelled the row as cancelled, and recorded an elevation from a user's Stop. No test
/// can catch that (both callers destructure positionally and `run_in_pty` needs a live
/// socket), so the type is what has to.
struct PtyRun {
    /// The child's exit code, or -1 if it could not be reaped.
    code: i32,
    /// `forbidden::is403` matched the step's output. RAW — not "the step
    /// failed with a 403"; see the observation site in `do_step` for why that distinction
    /// is kept.
    forbidden: bool,
    /// The first URL in the output, extracted only when `forbidden` — what the modal
    /// offers to open.
    url: Option<String>,
    /// The USER killed this step. Distinct from a plain non-zero exit: the caller must be
    /// able to tell "I stopped this" from "this broke".
    cancelled: bool,
    /// The Windows watcher saw a foreign window during this step — our only elevation
    /// signal, imprecise by nature (see behaviour.rs). Always false on macOS/Linux.
    saw_window: bool,
    /// Everything the step printed, as the 403 and sudo scans accumulated it — MOVED out, not
    /// copied. The journal makes its tail from it (`step_tail`), and only for a failure.
    output: String,
    /// What Talos typed into the step (a sudo password), so the journal can write it `***`:
    /// a prompt that echoes would otherwise hand it back as output.
    typed: Vec<String>,
}

/// Streams the command into a pty,
/// scans the 403 as it streams, ticks the watcher (Windows). The pty runs in a
/// blocking thread; mpsc channel → async. The verdict (done/step/overlay) is left to
/// the caller (do_step), like the TS.
async fn run_in_pty(socket: &mut WebSocket, state: &AppState, i: u32, cmdline: &str) -> PtyRun {
    use base64::{engine::general_purpose::STANDARD, Engine};

    // pty_shell is THE single source of the shell wrapping: Windows → powershell + PATH
    // refresh + exit $LASTEXITCODE + `&&`→ PS 5.1 guard translation; POSIX → user shell
    // in interactive+login (sees ~/.local/bin). DO NOT duplicate here: the hardcoded version
    // that lived here short-circuited pty_shell → the `&&` of a `claude plugin marketplace
    // add … && install …` reached PS 5.1 as-is ("token && is not a valid statement
    // separator"). A single path, tested (platform::tests).
    let (program, args): (String, Vec<String>) = {
        let probe = crate::platform::pty_shell(crate::platform::current_os(), cmdline);
        (probe.cmd, probe.args)
    };

    // Watcher (Windows only): tick 1200ms → looks for an installer window
    // surfaced behind the panel, dedup by title. Root = our own pid.
    #[cfg(target_os = "windows")]
    let (watch_stop, mut watch_rx) = {
        let (wtx, wrx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            let root = std::process::id();
            let mut last_title: Option<String> = None;
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                let scan = crate::watch::parse_scan(&crate::watch::powershell_spawner(root));
                if scan.found {
                    let title = scan.title.clone().unwrap_or_default();
                    if Some(&title) != last_title.as_ref() {
                        last_title = Some(title.clone());
                        let _ = wtx.send(serde_json::json!({
                            "type": "wait-window", "i": 0, "title": title, "pushed": scan.pushed
                        }));
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1200));
            }
        });
        (stop, wrx)
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (code_tx, code_rx) = tokio::sync::oneshot::channel::<i32>();
    // INPUT channel to the pty (std::sync::mpsc: the pty thread drains it
    // blocking). This is where the sudo password is injected.
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    // The killer for THIS step's child, shared with the select! loop below. A plain
    // Mutex (not tokio's): the lock is held for the duration of a `kill()` syscall and
    // never across an await, so an async mutex would buy nothing.
    let killer: std::sync::Arc<
        std::sync::Mutex<Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(None));
    let killer_slot = killer.clone();
    std::thread::spawn(move || {
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let code = crate::pty::run(
            &program,
            &arg_refs,
            Some(in_rx),
            None, // a row's terminal is a fixed-width display box — keep 100x24
            &[],  // steps inherit the engine env unchanged
            |k| {
                if let Ok(mut slot) = killer_slot.lock() {
                    *slot = Some(k);
                }
            },
            |bytes| {
                let _ = tx.send(bytes.to_vec());
            },
        )
        .unwrap_or(-1);
        let _ = code_tx.send(code);
    });

    // Sudo prompt detection as it streams (the 403 is read once, at the end). The
    // "Password:" prompt is NOT followed by a newline (sudo writes it raw), so we test the
    // END of the current buffer. Once answered, we don't re-ask for this step.
    let mut buf = String::new();
    // Where the output after the last answer starts, and what was answered (see `PtyRun`).
    let mut answered_at = 0usize;
    let mut typed: Vec<String> = Vec::new();
    // The bytes of a character a read cut in two, waiting for the rest (`decode_chunk`).
    let mut carry: Vec<u8> = Vec::new();
    // Set when the USER killed this step. Distinct from `forbidden` and from a plain
    // non-zero exit: the caller must be able to tell "I stopped this" from "this broke".
    let mut cancelled = false;
    // Did the Windows watcher see a foreign window during this step? That is our only
    // elevation signal, and it is imprecise by nature (see behaviour.rs). Always false
    // on macOS/Linux, where no equivalent observation exists yet — hence the cfg'd
    // allow: nothing assigns it on those targets.
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut saw_window = false;
    // Gates the socket arm of the select below. A dropped socket yields None forever, so
    // polling it after that is a hot loop; this turns the branch off once and for all.
    let mut socket_open = true;
    // How many times we have answered a password prompt in THIS step. sudo grants
    // three attempts, so answering only once was a hang waiting to happen: a wrong
    // password (very often a STALE CACHED one — model C reuses it across the whole
    // Apply) gets "Sorry, try again." and a second "Password:", which the old
    // answer-once guard refused to serve. The pty then sat forever on a prompt
    // nobody would ever type into, with no way to cancel the row.
    let mut sudo_tries = 0u8;
    const SUDO_MAX_TRIES: u8 = 3; // sudo's own budget — past it, sudo gives up by itself
                                  // Where the last answer came from, so a refusal can invalidate the right thing.
    let mut last_from_cache = false;
    loop {
        // TWO sources, because a Stop must be readable WHILE output flows. The sudo
        // prompt and the 403 pause also read this socket mid-step, but they BLOCK on it;
        // a cancel cannot block, or the row's terminal would freeze while we waited for
        // a message that may never come.
        let chunk = tokio::select! {
            // Biased: drain output first, so a chatty command cannot starve the cancel
            // check AND a cancel cannot swallow bytes already queued. Deterministic
            // beats fair here — the pending-output path is the common one.
            biased;
            maybe = rx.recv() => match maybe {
                Some(c) => c,
                None => break, // pty closed → step over
            },
            msg = socket.recv(), if socket_open => {
                // ⚠️ A CLOSED socket returns None IMMEDIATELY and forever. Without this
                // arm the `continue` below would spin a hot loop — burning a core until
                // the pty happened to close, which for a long install is minutes. So we
                // stop polling the socket once it is gone: `socket_open = false` removes
                // this branch from the select and the loop goes back to draining output
                // only, which is exactly right — nobody is left to send a cancel, but the
                // step must still finish and be reaped.
                if msg.is_none() {
                    socket_open = false;
                    continue;
                }
                // Only a cancel aimed at THIS step acts. Anything else is ignored here
                // and NOT consumed elsewhere — sudo-pw and forbidden-* are read by their
                // own blocking loops, which run at moments this select is not active.
                if let Some(Ok(Message::Text(txt))) = msg {
                    if cancel_targets(&txt, i) {
                        if let Ok(mut slot) = killer.lock() {
                            if let Some(k) = slot.as_mut() {
                                let _ = k.kill();
                            }
                        }
                        cancelled = true;
                        // No break: the kill makes the child exit, the master is dropped,
                        // the reader hits EOF and `rx.recv()` returns None on its own. We
                        // leave through the SAME door as a normal finish, so the exit code
                        // and the Windows watcher teardown below still happen.
                        println!("[cancel] killed step {i} at the user's request");
                    }
                }
                continue;
            }
        };
        // Always accumulate (the sudo prompt may be split across several chunks,
        // or "Password:" arrive in a separate piece → testing the chunk alone misses it).
        buf.push_str(&decode_chunk(&mut carry, &chunk));
        let out = json!({ "type": "out", "i": i, "data": STANDARD.encode(&chunk) });
        if socket.send(Message::Text(out.to_string())).await.is_err() {
            return PtyRun {
                code: -1,
                forbidden: crate::forbidden::is403(&buf),
                url: None,
                cancelled,
                saw_window,
                output: buf,
                typed,
            };
        }
        // The last line of what followed the last answer, lowercased ONCE per chunk.
        let last = last_line(prompt_window(&buf, answered_at));
        // A REFUSAL invalidates whatever we just sent. If it came from the cache, drop
        // it: the whole Apply would otherwise keep replaying the same wrong secret,
        // every sudo row hanging in turn. Clearing makes the next prompt ask the user.
        if last.ends_with("try again.") && last_from_cache {
            clear_sudo_pw(state).await;
            last_from_cache = false;
        }
        // sudo writes "Password:" (or "Password for X:") WITHOUT a final newline, so we
        // test the ACCUMULATED BUFFER (the prompt may arrive in its own chunk). We answer
        // EACH prompt up to sudo's own budget — a re-prompt means the last answer was
        // refused, and leaving it unanswered is a permanent hang, not a safeguard.
        // Only what came AFTER the last answer is read (`prompt_window`), so the SAME prompt
        // is not re-detected on the next chunk — its tail would still end with ':' and we
        // would answer a question already answered, burning the attempt budget on nothing.
        // The buffer itself is kept whole: `is403`, `extract_url` and the journal's tail read it.
        if sudo_tries < SUDO_MAX_TRIES && prompt_armed(&last) {
            if let Some((pw, from_cache)) = obtain_sudo_pw(socket, state, i).await {
                let mut line = pw.clone().into_bytes();
                line.push(b'\n');
                let _ = in_tx.send(line);
                sudo_tries += 1;
                last_from_cache = from_cache;
                answered_at = buf.len();
                typed.push(pw);
            }
        }
        // Relay the watcher signals (non-blocking) as they stream. A relayed message is
        // also what `saw_window` records: the two must not drift, so the flag is set
        // HERE rather than in the watcher thread.
        #[cfg(target_os = "windows")]
        while let Ok(wmsg) = watch_rx.try_recv() {
            saw_window = true;
            let _ = socket.send(Message::Text(wmsg.to_string())).await;
        }
    }

    // The pty is closed: whatever `decode_chunk` still held can never be completed.
    buf.push_str(&String::from_utf8_lossy(&carry));
    // pty finished: stop the watcher + wait-clear (like watcher.stop()).
    #[cfg(target_os = "windows")]
    {
        watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        while let Ok(wmsg) = watch_rx.try_recv() {
            saw_window = true;
            let _ = socket.send(Message::Text(wmsg.to_string())).await;
        }
        let _ = socket
            .send(Message::Text(
                json!({ "type": "wait-clear", "i": i }).to_string(),
            ))
            .await;
    }

    // The 403 rule, read ONCE over the whole output: nothing reads `forbidden` before the step
    // ends, and one pass cannot disagree with itself at a chunk boundary.
    let forbidden = crate::forbidden::is403(&buf);
    let code = code_rx.await.unwrap_or(-1);
    let url = if forbidden {
        crate::forbidden::extract_url(&buf)
    } else {
        None
    };
    PtyRun {
        code,
        forbidden,
        url,
        cancelled,
        saw_window,
        output: buf,
        typed,
    }
}

/// The journal's tail of a step's output, this user written out of it (`~`, `<user>`) and
/// what Talos typed into the step written `***`. `user` is the journal's own user name
/// (`ConsentStore::user`), so the tail masks the name the shared file is named after.
fn step_tail(buf: &str, typed: &[String], user: &str) -> String {
    let typed: Vec<&str> = typed.iter().map(String::as_str).collect();
    // Who the user is. On Windows the PROFILE folder before `HOME`: a Git-Bash or corporate
    // `HOME` (`/c/Users/al`, `H:\`) is not the spelling tools print. And the account name as
    // well, which may differ from the folder's (`al.CORP`).
    let home = std::env::var("USERPROFILE")
        .ok()
        .filter(|h| cfg!(windows) && !h.is_empty())
        .unwrap_or_else(|| crate::detect::user_home().to_string_lossy().into_owned());
    crate::tail::readable_tail(buf, &home, user, &typed)
}

/// The end of the output a prompt is looked for in: what followed the last answer, and at
/// most its last `PROMPT_WINDOW` bytes. Reading the whole buffer on every chunk made a long
/// step quadratic, and let a `password` printed anywhere earlier arm the detection for any
/// later line ending in `:`. The window is generous on purpose: a prompt that names a long
/// path, wrapped in ConPTY's escapes, must still fit — a prompt cut out of it is a step that
/// hangs on a question nobody answers.
fn prompt_window(buf: &str, answered_at: usize) -> &str {
    const PROMPT_WINDOW: usize = 4096;
    let mut start = answered_at
        .max(buf.len().saturating_sub(PROMPT_WINDOW))
        .min(buf.len());
    while !buf.is_char_boundary(start) {
        start += 1;
    }
    &buf[start..]
}

/// Is the window's LAST line (`last_line`) a password prompt? It ends in `:` and names a
/// password. Only
/// that line: a `password` printed on an earlier one ("Checking password policy...") must
/// not arm a later `Select option:` — answering it would type the sudo password into a
/// stdin that echoes it.
fn prompt_armed(last_line: &str) -> bool {
    last_line.ends_with(':') && last_line.contains("password")
}

/// The window's last line, trimmed and lowercased — what `prompt_armed` and the refusal
/// check read. Only that line is copied, not the window.
fn last_line(window: &str) -> String {
    window
        .trim_end()
        .rsplit(['\n', '\r'])
        .next()
        .unwrap_or("")
        .to_lowercase()
}

/// A pty read as text, without splitting a character: a read can end inside a multi-byte
/// character, and decoding each read on its own turned both halves into U+FFFD — a
/// password with an `ä` echoed across two reads then no longer matched its mask. The
/// incomplete end waits in `carry` for the next read; a byte that is invalid whatever
/// follows is replaced, as before.
fn decode_chunk(carry: &mut Vec<u8>, chunk: &[u8]) -> String {
    carry.extend_from_slice(chunk);
    // Walk back over at most three continuation bytes to the last lead byte: if the
    // character it starts is longer than what arrived, it waits — whatever came before.
    let mut start = carry.len();
    for back in 1..=carry.len().min(4) {
        let b = carry[carry.len() - back];
        if b & 0b1100_0000 != 0b1000_0000 {
            let need = match b {
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF7 => 4,
                _ => 1,
            };
            if need > back {
                start = carry.len() - back;
            }
            break;
        }
    }
    let rest = carry.split_off(start);
    let text = String::from_utf8_lossy(carry).into_owned();
    *carry = rest;
    text
}

/// Obtains the sudo password — MODEL C. If the RAM cache already holds it (entered
/// earlier in this Apply), we reuse it without re-asking. Otherwise we ask the
/// front (message `sudo-prompt`), wait for its response (`sudo-pw`), cache it.
/// The password NEVER touches disk/log/journal. Cleared by clear_sudo_pw
/// at the end of the Apply. None if the front cancels (closes the modal → `sudo-cancel`).
/// Returns `(password, came_from_cache)`. The caller needs the provenance: when sudo
/// answers "Sorry, try again.", a CACHED password must be dropped before re-asking,
/// or we would hand sudo the same wrong secret until it gives up — and the row would
/// look stuck for reasons the user cannot see.
async fn obtain_sudo_pw(
    socket: &mut WebSocket,
    state: &AppState,
    i: u32,
) -> Option<(String, bool)> {
    // 1) cache?
    {
        let guard = state.sudo_pw.lock().await;
        if let Some(pw) = guard.as_ref() {
            return Some((pw.clone(), true));
        }
    }
    // 2) ask the front (masked field).
    let _ = socket
        .send(Message::Text(
            json!({ "type": "sudo-prompt", "i": i }).to_string(),
        ))
        .await;
    // 3) wait for the response on the SAME socket (run_in_pty has exclusive use of it here).
    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(txt) = msg else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
        match parsed.get("type").and_then(|t| t.as_str()) {
            Some("sudo-pw") => {
                let pw = parsed
                    .get("pw")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                *state.sudo_pw.lock().await = Some(pw.clone()); // RAM cache for the duration of the Apply
                return Some((pw, false)); // freshly typed, not from the cache
            }
            Some("sudo-cancel") => return None,
            _ => {} // ignore any other message while waiting for the password
        }
    }
    None
}

/// Clears the cached password (end of Apply / close). RAM reset to None.
async fn clear_sudo_pw(state: &AppState) {
    *state.sudo_pw.lock().await = None;
}

/// What the user chose when a 403 held the Apply. THREE outcomes, because three
/// things can honestly be wanted — and the UI now offers exactly three controls.
/// (It used to offer two words for one message: "Retry" sent `forbidden-continue`,
/// so it retried nothing, and `forbidden-stop` was unreachable dead code.)
#[derive(Debug, Clone, Copy, PartialEq)]
enum ForbiddenChoice {
    /// Re-run the SAME action on the SAME row, then carry on with the rest.
    Retry,
    /// Skip this row (it keeps its `forbidden` mark) and run the rest of the plan.
    Continue,
    /// Abandon everything still to do.
    Stop,
}

/// How many times ONE row may RUN inside a single Apply — so this many minus one
/// is the number of retries offered (3 runs = the first attempt + 2 retries). Named
/// for runs rather than retries because `attempt` is incremented BEFORE do_step, and
/// the earlier "attempts"/"retries" wording described the same constant two
/// different ways — the handoff said "retries are capped at 3", which was one too
/// many.
///
/// Each retry is user-initiated so it cannot spin on its own, but an unbounded loop
/// lets a user hang the Apply forever against a firewall that will not budge. Past
/// the cap the pause offers only Continue/Stop.
const MAX_FORBIDDEN_RUNS: u32 = 3;

/// The wire word → the decision. Anything else is NOT an answer: the pause keeps
/// holding (notably `open-forbidden`, which we serve while we hold).
fn forbidden_choice(kind: &str) -> Option<ForbiddenChoice> {
    match kind {
        "forbidden-retry" => Some(ForbiddenChoice::Retry),
        "forbidden-continue" => Some(ForbiddenChoice::Continue),
        "forbidden-stop" => Some(ForbiddenChoice::Stop),
        _ => None,
    }
}

/// Is Retry still on the table? `attempt` counts runs already made — 1 means the
/// first run just failed, so a retry would be run 2.
fn retry_offered(attempt: u32) -> bool {
    attempt < MAX_FORBIDDEN_RUNS
}

/// Defence in depth: a stale front could send `forbidden-retry` past the cap. Do
/// not re-run the step then — move on rather than risk hanging the Apply.
fn honour_choice(choice: ForbiddenChoice, attempt: u32) -> ForbiddenChoice {
    if choice == ForbiddenChoice::Retry && !retry_offered(attempt) {
        return ForbiddenChoice::Continue;
    }
    choice
}

/// A step was blocked by the firewall (403). PAUSE the Apply and wait for the user
/// to decide — they go unblock the page, then tell us what to do. Same shape as
/// obtain_sudo_pw: send a prompt, block on THIS socket until the answer arrives.
///
/// `attempt` = how many times this row has already run (1 after the first failure).
/// It rides the pause message so the front can SAY which attempt this is — a second
/// identical banner with no count reads as "nothing happened" — and so it can drop
/// the Retry control once the cap is reached.
///
/// A closed socket (window shut mid-pause) reads as `Stop` — nothing should run on
/// with nobody watching.
async fn await_forbidden_decision(
    socket: &mut WebSocket,
    i: usize,
    attempt: u32,
) -> ForbiddenChoice {
    let _ = socket
        .send(Message::Text(
            json!({
                "type": "forbidden-pause", "i": i,
                "attempt": attempt, "canRetry": retry_offered(attempt)
            })
            .to_string(),
        ))
        .await;
    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(txt) = msg else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
        let kind = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");
        // The user dealt with it — retry it, skip it, or abandon the run. Their call.
        if let Some(choice) = forbidden_choice(kind) {
            return honour_choice(choice, attempt);
        }
        // "Open blocked page" MUST keep working while we hold: it is the very
        // gesture the pause exists for. Served here too, because the main
        // handler loop isn't reading the socket while we own it.
        if kind == "open-forbidden" {
            if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                if let Err(e) = crate::platform::open_url(url) {
                    println!("[forbidden] failed to open {url}: {e}");
                } else {
                    println!("[forbidden] opening browser: {url}");
                }
            }
        }
        // Anything else is ignored while paused — nothing advances.
    }
    ForbiddenChoice::Stop
}

// Benign exit codes: a non-zero exit that is a success anyway, for THIS action.
// - winget "no applicable upgrade" (-1978335189): already current, whatever asked.
// - winget "no package found" (-1978335212): gone is what an uninstall wants. For an
//   install or an upgrade it is the failure itself (MEASURED: a wrong id journalled ok).
fn benign_code(action: crate::decision::Action, code: i32) -> bool {
    match code {
        -1978335189 => true,
        -1978335212 => action == crate::decision::Action::Uninstall,
        _ => false,
    }
}

/// One step of an Apply, with the 403 pause around it. Returns the LAST attempt's outcome
/// and whether the user chose to stop everything.
///
/// A 403 is REPAIRABLE, not a plain failure: the user opens the blocked page and unblocks
/// it. So we do NOT run on — carrying on would (a) make them read a page while installs
/// scroll past behind, and (b) burn every FOLLOWING package that goes through the same
/// blocked source, when one unblock up front would have saved them all. Instead: the action
/// that was running FINISHES (never kill a pty mid-flight — a half-installed package is the
/// worst case; the exit code is the truth), and THEN we wait for the user.
///
/// Resuming is gated on a DECISION, not on success: they may fail to unblock. Either way
/// it's their call — nothing restarts until they say so. And Retry means RETRY: the same
/// action runs again HERE, in place, before the plan moves on. (It used to mean "continue",
/// which left the user to hunt the row down afterwards — the word on the button lied.)
async fn step_with_forbidden_pause(
    socket: &mut WebSocket,
    state: &AppState,
    i: usize,
    action: crate::decision::Action,
    step: &crate::bundles::Step,
    is_cask: bool,
    aim: Aim<'_>,
) -> (StepOutcome, bool) {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let outcome = do_step(socket, state, i, action, step, is_cask, aim).await;
        if !outcome.blocked {
            return (outcome, false);
        }
        match await_forbidden_decision(socket, i, attempt).await {
            ForbiddenChoice::Retry => continue, // same row, same action, again
            ForbiddenChoice::Continue => return (outcome, false),
            ForbiddenChoice::Stop => return (outcome, true),
        }
    }
}

/// Did a step CHANGE the package it acted on? Presence or version, compared between what the
/// Apply decided from and what `do_step` re-probed after. `None` after = the step failed or
/// did nothing observable, so nothing changed that we know of.
///
/// ⭐ The trigger of the dependents pass, chosen over "a step ran": winget answering "already
/// current" exits benign and changes nothing, and must not cost a probe per dependent.
fn step_changed_package(
    before: Option<&crate::detect::Presence>,
    after: Option<&crate::detect::Presence>,
) -> bool {
    let Some(after) = after else { return false };
    let Some(before) = before else { return true };
    let v = |p: &crate::detect::Presence| p.version.as_deref().unwrap_or("").trim().to_string();
    before.present != after.present || v(before) != v(after)
}

/// Which dependents of the package called `name` the Apply re-probes after that package
/// changed: the direct ones, minus those still ahead in the plan (they will run anyway, after
/// their requirement — topo_sort put them there), minus those already re-checked in this
/// Apply. `rechecked` is updated: each dependent is re-checked at most once per Apply.
fn dependents_to_recheck(
    requires: &[Vec<String>],
    name: &str,
    still_planned: &std::collections::HashSet<usize>,
    rechecked: &mut std::collections::HashSet<usize>,
) -> Vec<usize> {
    crate::deps::dependents_of(requires, name)
        .into_iter()
        .filter(|d| !still_planned.contains(d) && rechecked.insert(*d))
        .collect()
}

/// A version for a sentence: the probed one, or `?` when none was read.
fn shown_version(p: &crate::detect::Presence) -> String {
    match p.version.as_deref().map(str::trim) {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => "?".into(),
    }
}

/// What a version change was run to reach, when it can be named: an exact pin owns it,
/// otherwise the version the manager announced.
fn step_target(pin: Option<&str>, announced: Option<&str>) -> Option<String> {
    use crate::bundles::{classify_pin, PinKind};
    if let Some(PinKind::Exact(p)) = classify_pin(pin) {
        return Some(p);
    }
    announced
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
}

/// Why a version change that EXITED OK did not do what it was run for, or None.
///
/// ⭐ An exit code says the command is content, not that the machine changed. MEASURED twice:
/// winget's VS Code 1.142.0 manifest installs 1.141.2 and exits 0 (the integrator, Windows,
/// 2026-10-22), and `winget install --version 0.113.1 --force --uninstall-previous` printed
/// "Successfully installed" with `nu` still 0.116.1 (Windows VM, 2026-10-20). So the version
/// read AFTER the step is the verdict: toward a pin, it must be the pin (at the pin's own
/// precision); otherwise it must have moved, or be at or past what was announced. A version
/// that cannot be read, before or after, gives no verdict — never a guess.
///
/// ⚠️ "At or past the announcement" is not a courtesy. A tool that updates itself (Copilot
/// CLI, VS Code) is often already current when the manager's stale receipt still lists it as
/// behind: the upgrade runs, exits 0 or winget's benign "no applicable upgrade", and the
/// version does not move because it was already there. Before this, that row ended red.
fn unmoved_reason(
    action: crate::decision::Action,
    pin: Option<&str>,
    before: Option<&crate::detect::Presence>,
    after: &crate::detect::Presence,
    target: Option<&str>,
    by: &str,
) -> Option<String> {
    use crate::bundles::{classify_pin, PinKind};
    use crate::decision::{bare_version, compare_versions, Action};
    if !matches!(action, Action::Upgrade | Action::Downgrade) || after.present != Some(true) {
        return None;
    }
    let read = |v: Option<&str>| bare_version(v.unwrap_or("")).to_string();
    let known = |v: &str| !v.is_empty() && v != "?";
    let now = read(after.version.as_deref());
    if !known(&now) {
        return None;
    }
    // Toward a pin, what RUNS is the verdict: the manager's word is exactly what lied in the
    // measured downgrade.
    if let Some(PinKind::Exact(p)) = classify_pin(pin) {
        return (crate::decision::compare_to_pin(&now, &p) != 0)
            .then(|| format!("{by} reported success, but {now} is installed, not {p}"));
    }
    // Otherwise the manager's own copy counts too: `detect:` may read another one that comes
    // first on PATH (a mise `node` ahead of winget's), and the manager changed ITS copy.
    let managed_now = read(after.managed.as_deref());
    let target = target.map(bare_version).filter(|t| !t.is_empty());
    if let Some(t) = target {
        if [&now, &managed_now]
            .into_iter()
            .any(|v| known(v) && compare_versions(v, t) >= 0)
        {
            return None;
        }
    }
    let before = before?;
    let was = read(before.version.as_deref());
    let managed_was = read(before.managed.as_deref());
    // Numerically: the listing before and the probe after may spell one version two ways.
    let managed_moved = known(&managed_was)
        && known(&managed_now)
        && compare_versions(&managed_was, &managed_now) != 0;
    if !known(&was) || was != now || managed_moved {
        return None;
    }
    Some(match target {
        Some(t) => format!("{by} reported success, but {now} is still installed, not {t}"),
        None => format!("{by} reported success, but {now} is still installed"),
    })
}

/// A version change that exited ok and moved nothing, as the session remembers it.
#[derive(Debug, Clone, Default)]
struct Unmoved {
    /// The version read after the change — the one it left in place.
    found: String,
    /// What it ran toward (`step_target`), or empty when nothing was named.
    target: String,
    /// The row's sentence (`unmoved_reason`).
    reason: String,
    /// The command the row runs for this change (`change_command`). Another command — the
    /// author moved to `version: self` and its `update:` — is another attempt, never held.
    cmd: String,
    /// Not a failure: the tool is PAST what its manager offers, and forcing the manager would
    /// take it back (`ahead_of_manager`). Held all the same, so the Apply stops asking.
    ahead: bool,
}

/// winget's "Unable to remove Portable package as it has been modified; to override this check
/// use --force" (MEASURED, winget v1.29, Windows VM, 2026-10-25).
const PORTABLE_MODIFIED: i32 = -1978335145;

/// Did winget refuse a manager upgrade because the PORTABLE package's files changed since it
/// installed them — a tool that updated itself in winget's own folder? Not for a `self` row:
/// its upgrade is the tool's command, never winget's.
fn portable_refused(
    action: crate::decision::Action,
    route: Option<&str>,
    self_updating: bool,
    code: i32,
) -> bool {
    action == crate::decision::Action::Upgrade
        && route == Some("winget")
        && !self_updating
        && code == PORTABLE_MODIFIED
}

/// What to do about that refusal, from the version the tool reports and the version winget
/// announced.
#[derive(Debug, PartialEq, Eq)]
enum PortableChoice {
    /// At or below the announcement: `--force` takes it there and realigns winget's record.
    Force,
    /// Past it: forcing would take the tool BACK. Left as it is.
    Ahead,
    /// No version read, or none announced: no evidence either way, the refusal stands.
    Neither,
}

///
/// ⚠️ Only a reading of the BINARY may force (`from_binary`). Without one, the version read is
/// the manager's own record — behind the announcement by definition — and would always say
/// "force", over a tool that may well be ahead of it.
fn portable_choice(read: &str, target: Option<&str>, from_binary: bool) -> PortableChoice {
    use crate::decision::{bare_version, compare_versions};
    let read = bare_version(read);
    match target.map(bare_version).filter(|t| !t.is_empty()) {
        Some(t) if from_binary && !read.is_empty() && read != "?" => {
            if compare_versions(read, t) > 0 {
                PortableChoice::Ahead
            } else {
                PortableChoice::Force
            }
        }
        _ => PortableChoice::Neither,
    }
}

/// A plain `winget upgrade`, one command: the only kind `--force` may be appended to. An
/// author's compound `upgrade:` would take the flag on its last part.
fn forceable(cmd: &str) -> bool {
    cmd.trim_start().starts_with("winget upgrade ") && !cmd.contains([';', '&', '|'])
}

/// Did a version change find the tool already at or past its target and leave everything as it
/// was — what runs, and the manager's own record? Then running it again changes nothing either:
/// a record the manager will not realign (an installer of another technology, winget's "no
/// applicable upgrade"), or the manager's copy behind one that comes first on PATH. A success,
/// held like a tool ahead of its manager, so the Apply stops knocking.
fn settled_without_moving(
    before: Option<&crate::detect::Presence>,
    after: &crate::detect::Presence,
    target: Option<&str>,
) -> bool {
    use crate::decision::{bare_version, compare_versions};
    let read = |v: Option<&str>| bare_version(v.unwrap_or("")).to_string();
    let known = |v: &str| !v.is_empty() && v != "?";
    let (Some(before), Some(t)) = (before, target.map(bare_version).filter(|t| !t.is_empty()))
    else {
        return false;
    };
    let (was, now) = (
        read(before.version.as_deref()),
        read(after.version.as_deref()),
    );
    let (m_was, m_now) = (
        read(before.managed.as_deref()),
        read(after.managed.as_deref()),
    );
    let reached = [&now, &m_now]
        .into_iter()
        .any(|v| known(v) && compare_versions(v, t) >= 0);
    let moved = compare_versions(&was, &now) != 0
        || (known(&m_was) && known(&m_now) && compare_versions(&m_was, &m_now) != 0);
    known(&was) && known(&now) && reached && !moved
}

/// The hold of a tool past what its manager offers: remembered like a change that moved
/// nothing (`held_unmoved`), but a success — the Apply stops knocking on the manager until it
/// announces something newer, or the tool moves.
fn ahead_of_manager(found: &str, target: &str, cmd: &str) -> Unmoved {
    Unmoved {
        found: found.to_string(),
        target: target.to_string(),
        reason: if crate::decision::compare_versions(found, target) > 0 {
            format!(
                "{found} is past {target}, the newest its manager offers; the manager will not \
                 replace a package the tool updated itself, and forcing it would go back"
            )
        } else {
            format!(
                "{found} is already {target}, and the manager's record did not move: running \
                 its upgrade again would change nothing"
            )
        },
        cmd: cmd.to_string(),
        ahead: true,
    }
}

/// The commands a row changes its version with, as the catalogue gave them: what tells one
/// attempt from another for a hold (`held_unmoved`). Both of them, whatever the action: the scan
/// looks a hold up without knowing which one recorded it, and a downgrade hold keyed on its own
/// command alone was never found again. The forms rebuilt at run time (a forced cask, a winget
/// scope, a brew pin served as plain brew) follow from these and from the machine, so they
/// need no key of their own.
fn change_command(step: &crate::bundles::Step) -> String {
    format!(
        "{}\n{}",
        step.upgrade.as_deref().unwrap_or(""),
        step.downgrade.as_deref().unwrap_or("")
    )
}

/// Has this package's change already been run, in this session, by this same command, from the
/// version installed now toward this same target, and left the version where it was? Then the
/// Apply holds it. The machine moving (by hand, or by the tool itself), a new target or another
/// command lifts the hold: what was remembered no longer describes what would run.
fn held_unmoved(
    memory: &std::collections::HashMap<String, Unmoved>,
    id: &str,
    installed: &str,
    target: &str,
    cmd: &str,
) -> bool {
    use crate::decision::bare_version;
    let installed = bare_version(installed);
    !installed.is_empty()
        && memory.get(id).is_some_and(|u| {
            bare_version(&u.found) == installed
                && bare_version(&u.target) == bare_version(target)
                && u.cmd == cmd
        })
}

/// What a step was run from and toward, for the verdict on the version it leaves
/// (`unmoved_reason`). Default = no verdict: a retry or a repair install.
#[derive(Default, Clone, Copy)]
struct Aim<'a> {
    /// The presence the Apply decided from.
    before: Option<&'a crate::detect::Presence>,
    /// The version the manager announced for this package, if any.
    announced: Option<&'a str>,
}

/// The outcome of one step. `ok` = it succeeded (kept for callers that will want it;
/// the front already learns it from the `step` event). `blocked` = it failed BECAUSE
/// the corporate firewall answered 403 — the distinction the Apply loop needs: a plain
/// failure moves on, a 403 is REPAIRABLE by the user, so the loop pauses and waits.
#[derive(Default, Clone)]
struct StepOutcome {
    #[allow(dead_code)]
    ok: bool,
    blocked: bool,
    /// The presence re-probed right after a SUCCESSFUL step; `None` when it failed, was
    /// cancelled or had no command. What the Apply compares with the presence it decided from,
    /// to learn whether the step changed the package (`step_changed_package`).
    after: Option<crate::detect::Presence>,
}

/// Runs ONE step (install/upgrade/uninstall): streams the command, reads the exit
/// code, emits `step` (running → ok/absent/fail/forbidden) and the 403 verdict.
async fn do_step(
    socket: &mut WebSocket,
    state: &AppState,
    i: usize,
    action: crate::decision::Action,
    step: &crate::bundles::Step,
    is_cask: bool,
    aim: Aim<'_>,
) -> StepOutcome {
    use crate::decision::Action;
    // Timed from the top so the measurement covers everything the step really does:
    // the pty run, the prompt waits inside it, AND the post-action
    // `detect_present_detailed` re-probe below — that probe is slow enough that the
    // startup scan times it per package. (The outdated scan is NOT in this window; both
    // callers scan before calling and pass `is_cask` in.) A duration that stopped at the
    // pty would flatter exactly the slow packages the number exists to expose.
    let started = std::time::Instant::now();
    let os = state.os;
    // Whatever this step does — install, upgrade, uninstall, even a failure half-way — the
    // outdated map read before it may no longer be the machine's. Forgotten HERE, the one
    // place both callers act through, so the next Apply scans it again.
    *state.outdated_seen.lock().await = None;
    // A cask Upgrade needs the forced --cask --force command (brew's receipt drift
    // → anti-clobber exit 1 otherwise). Single resolution point for BOTH callers
    // (batch Apply + retry-step); formulae, non-brew and pinned casks keep step.upgrade.
    let forced = if action == Action::Upgrade {
        forced_cask_upgrade_cmd(step, os, is_cask)
    } else {
        None
    };
    // A winget version change is rebuilt HERE, for where the package must live — a fact of
    // the machine, read just before acting, that the command stored at load cannot hold.
    let rebuilt = match winget_in_place(state, action, step, os).await {
        Some(r) => Some(r),
        // A brew pin, the same way: plain brew when the pin is brew's current version, and
        // nothing at all when brew cannot serve it. The plan's answer is taken when there is
        // one; a retry or a repair asks brew itself.
        None => {
            let planned = state.brew_pins.lock().await.remove(&step.id);
            let verdict = match planned {
                Some(answer) => answer,
                None => ask_brew(action, step, os).await,
            };
            match brew_plan(action, step, verdict.as_ref()) {
                Ok(r) => r,
                Err(why) => {
                    // Reached from a retry or a repair only: the row's terminal says why, and
                    // the notes queued for a run that will not happen go with it.
                    println!("[{}] {} held: {why}", action.as_str(), step.name);
                    state.notes.lock().await.remove(&i);
                    say(socket, i, &format!("[{}] held: {why}", action.as_str())).await;
                    send_held(socket, i, &why).await;
                    return StepOutcome::default();
                }
            }
        }
    };
    if let Some((_, note)) = &rebuilt {
        queue_note(state, i, format!("[{}] {note}", action.as_str())).await;
    }
    let cmd = match &rebuilt {
        Some((c, _)) => Some(c.as_str()),
        None => match action {
            Action::Install => step.install.as_deref(),
            Action::Uninstall => step.uninstall.as_deref(),
            Action::Upgrade => forced.as_deref().or(step.upgrade.as_deref()),
            Action::Downgrade => step.downgrade.as_deref(),
        },
    };
    let Some(cmd) = cmd else {
        return StepOutcome::default(); // no command for this route → skip
    };
    let running = match action {
        Action::Uninstall => "uninstalling",
        Action::Upgrade => "upgrading",
        Action::Downgrade => "reinstalling",
        Action::Install => "installing",
    };
    let _ = socket
        .send(Message::Text(
            json!({ "type": "step", "i": i, "status": running }).to_string(),
        ))
        .await;

    // The bare command is passed to run_in_pty, which wraps it in the native shell
    // (POSIX: user shell in -ilc via pty_shell; Windows: powershell + PATH refresh).
    let mut run = run_in_pty(socket, state, i as u32, cmd).await;
    // What a version change was run toward: an exact pin owns it, otherwise the announcement.
    let target = step_target(step.pin.as_deref(), aim.announced);
    // ⭐ winget refuses to upgrade a PORTABLE package whose files changed since it installed
    // them (`PORTABLE_MODIFIED`) — what a tool that updates itself in winget's folder leaves
    // behind. The version it reports decides: at or below the announcement, `--force` takes it
    // there and realigns winget's record (MEASURED); past it, forcing would go back, so it is
    // left as it is — a success, held (`ahead_of_manager`). A rebuilt command (a winget scope)
    // is not a plain upgrade to add a flag to: its refusal stands.
    let mut ahead: Option<String> = None;
    if !run.cancelled
        && rebuilt.is_none()
        && portable_refused(
            action,
            step.route.as_deref(),
            is_self_updating(step),
            run.code,
        )
    {
        let step_c = step.clone();
        let p = tokio::task::spawn_blocking(move || detect_present_detailed(&step_c, os))
            .await
            .ok()
            .unwrap_or_default();
        // `managed` is set exactly when the version shown came from the binary.
        let read = p.version.clone().unwrap_or_default();
        match portable_choice(&read, target.as_deref(), p.managed.is_some()) {
            PortableChoice::Force if forceable(cmd) => {
                say(
                    socket,
                    i,
                    &format!(
                        "[upgrade] winget will not replace a portable package that changed since \
                         it installed it; the tool reads {read}, not past {}, so the upgrade runs \
                         again with --force.",
                        target.as_deref().unwrap_or("")
                    ),
                )
                .await;
                run = run_in_pty(socket, state, i as u32, &format!("{cmd} --force")).await;
            }
            PortableChoice::Ahead => ahead = Some(read),
            PortableChoice::Force | PortableChoice::Neither => {}
        }
    }
    let PtyRun {
        code,
        forbidden,
        url,
        cancelled,
        saw_window,
        output,
        typed,
    } = run;
    let ok = code == 0 || benign_code(action, code) || ahead.is_some();

    // The queued notes, NOW: before the command they were wiped twice over — by the front's
    // reset on the running status, and by ConPTY clearing the screen as the command starts.
    // Written here they read as the step's summary, above its exit line.
    for note in take_notes(state, i).await {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let line = format!("\r\n\x1b[2m{note}\x1b[0m");
        let _ = socket
            .send(Message::Text(
                json!({ "type": "out", "i": i, "data": STANDARD.encode(line.as_bytes()) })
                    .to_string(),
            ))
            .await;
    }
    // Diagnostic line in the row's terminal.
    let line = format!(
        "\r\n\x1b[2m[{}] exit {code} → {}\x1b[0m\r\n",
        action.as_str(),
        if cancelled {
            "cancelled"
        } else if ok {
            "ok"
        } else {
            "failed"
        }
    );
    {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let _ = socket
            .send(Message::Text(
                json!({ "type": "out", "i": i, "data": STANDARD.encode(line.as_bytes()) })
                    .to_string(),
            ))
            .await;
    }

    // The tool's own `update:` reaches the network on its own terms, behind a corporate
    // firewall as anywhere: its failure is a line on the row, never an Apply paused on a 403.
    let own_update = action == Action::Upgrade && is_self_updating(step);
    if own_update && !ok && !cancelled {
        say(
            socket,
            i,
            "[update] the tool's own update failed; Talos changed nothing else, and the next \
             Apply tries again. It needs the network — behind a corporate firewall, the usual \
             cause.",
        )
        .await;
    }
    let blocked = !ok && forbidden && !own_update;
    let status = if cancelled {
        // The user's own decision outranks every other reading of this exit code. A
        // killed process exits non-zero, which would otherwise print `fail` and send
        // them hunting a cause that does not exist.
        "cancelled"
    } else if ok {
        if action == Action::Uninstall {
            "absent"
        } else {
            "ok"
        }
    } else if blocked {
        "forbidden"
    } else {
        "fail"
    };
    let _ = socket
        .send(Message::Text(
            json!({ "type": "step", "i": i, "status": status }).to_string(),
        ))
        .await;
    if blocked {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "forbidden", "i": i, "url": url }).to_string(),
            ))
            .await;
    }

    // ⭐ This site keeps probing PER PACKAGE, and not only because one package does
    // not pay for a whole-machine listing: a listing fetched before the action would
    // be STALE for the very row that just changed, which is the one fact this probe
    // exists to establish. Detect, don't remember — and don't consult a table older
    // than the install.
    //
    // ⚠️ Kept ABOVE the anchor comment below on purpose: the guard
    // `every_presence_observation_is_recorded` reads a 1600-char window starting at
    // that anchor, and this block placed inside it pushed `remember_presence` out by
    // 8 characters. The guard was right; the comment moved.
    //
    // Re-detect presence AFTER any successful action — and RE-EMIT a `state` to the
    // front, so the fresh version shows RIGHT AWAY (before, it was only computed for
    // the journal → the front kept "absent/no version" until a manual refresh). The
    // front's `state` handler does setInstalledVersion + paintVersion.
    //
    // ⚠️ An uninstall is included, and `remember_presence` is NOT optional — both
    // because the Apply re-scan is now SCOPED against `last_seen` (seeds_for_rescan).
    // A row we acted on but never re-recorded keeps the presence the CONNECT scan saw,
    // and the next Apply then reads desire == observation, skips the probe, and decides
    // "nothing to do" against a state that no longer exists: uncheck + Apply (removes
    // it), re-check + Apply → the install silently never happens. Every site that
    // OBSERVES a presence must write it. There are four: scan_and_emit, the apply
    // re-scan, here, and the Apply's dependents pass.
    let (version, after) = if ok {
        let step_c = step.clone();
        let p = tokio::task::spawn_blocking(move || detect_present_detailed(&step_c, os))
            .await
            .ok()
            .unwrap_or_default();
        let v = p.version.clone().unwrap_or_default();
        remember_presence(state, i, p.clone()).await;
        let _ = socket
            .send(Message::Text(
                json!({
                    "type": "state", "i": i, "present": p.present,
                    "version": v, "external": p.external,
                    "probe": probe_json(&p.diag)
                })
                .to_string(),
            ))
            .await;
        // The journal records what an INSTALL/UPGRADE landed; a removal has no version.
        let v = if action == Action::Uninstall {
            String::new()
        } else {
            v
        };
        (v, Some(p))
    } else {
        (String::new(), None)
    };
    // ⭐ The verdict on the version, read just above: an exit 0 that moved nothing is not a
    // success (`unmoved_reason`). Said on the row, journalled as a failure, and remembered so
    // the Apply does not run it again toward the same target.
    let by = if own_update {
        format!("`{cmd}`")
    } else {
        step.route.clone().unwrap_or_else(|| "the command".into())
    };
    let unmoved = after
        .as_ref()
        .filter(|_| judges_version(step, action))
        .and_then(|p| {
            unmoved_reason(
                action,
                step.pin.as_deref(),
                aim.before,
                p,
                target.as_deref(),
                &by,
            )
        });
    if let Some(why) = &unmoved {
        let t = target.clone().unwrap_or_default();
        let found = after
            .as_ref()
            .and_then(|p| p.version.as_deref())
            .unwrap_or("")
            .trim()
            .to_string();
        let until = if matches!(
            crate::bundles::classify_pin(step.pin.as_deref()),
            Some(crate::bundles::PinKind::Exact(_))
        ) {
            format!("Until {found} or the pin changes, the Apply leaves this row")
        } else if t.is_empty() {
            format!("Until {found} changes, the Apply leaves this row")
        } else {
            format!(
                "Until {found} changes or a version other than {t} is announced, the Apply \
                 leaves this row"
            )
        };
        say(
            socket,
            i,
            &format!(
                "[{}] {why} — not a success. {until} (a restart of Talos tries once more).",
                action.as_str()
            ),
        )
        .await;
        let u = Unmoved {
            found,
            target: t,
            reason: why.clone(),
            cmd: change_command(step),
            ahead: false,
        };
        // The `ok` sent above was the exit code's; the verdict outranks it, as in the journal.
        let _ = socket
            .send(Message::Text(
                json!({ "type": "step", "i": i, "status": "fail" }).to_string(),
            ))
            .await;
        emit_unmoved(socket, i, &u).await;
        state.unmoved.lock().await.insert(step.id.clone(), u);
    } else if let Some((found, t)) = ahead.clone().zip(target.clone()).or_else(|| {
        // Nothing moved, at or past the target: the same rerun would change nothing either.
        let exact = matches!(
            crate::bundles::classify_pin(step.pin.as_deref()),
            Some(crate::bundles::PinKind::Exact(_))
        );
        let judged = ok && action == Action::Upgrade && !exact && judges_version(step, action);
        let p = after.as_ref().filter(|_| judged)?;
        if !settled_without_moving(aim.before, p, target.as_deref()) {
            return None;
        }
        let found = crate::decision::bare_version(p.version.as_deref()?).to_string();
        Some((found, target.clone()?))
    }) {
        let (found, t) = (&found, &t);
        let u = ahead_of_manager(found, t, &change_command(step));
        say(
            socket,
            i,
            &format!(
                "[upgrade] {} — left as it is. Until {found} changes or a version other than {t} \
                 is announced, the Apply leaves this row.",
                u.reason
            ),
        )
        .await;
        emit_unmoved(socket, i, &u).await;
        state.unmoved.lock().await.insert(step.id.clone(), u);
    } else if ok {
        state.unmoved.lock().await.remove(&step.id);
    }
    // A tool's own update run at each Apply, done: the row leaves the plan until a Refresh, and
    // says so — in its terminal and on the row (`self-updated`), so it does not look untouched.
    // A removal forgets it: a later reinstall is a new copy, and arms the row again.
    // The front is told both ways (`done`): a removal that exits ok can leave another copy
    // present, and a front still holding "done" would hide the update the next Apply runs.
    if ok && !cancelled && action == Action::Uninstall {
        let forgot = state
            .updated_this_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&step.id);
        if forgot {
            let _ = socket
                .send(Message::Text(
                    json!({ "type": "self-updated", "i": i, "done": false }).to_string(),
                ))
                .await;
        }
    }
    if arms_session_done(step, action, ok && !cancelled) {
        state
            .updated_this_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(step.id.clone());
        say(
            socket,
            i,
            &format!(
                "[{}] done for this session — the Apply leaves this row until Refresh re-arms it.",
                action.as_str()
            ),
        )
        .await;
        let _ = socket
            .send(Message::Text(
                json!({ "type": "self-updated", "i": i, "done": true }).to_string(),
            ))
            .await;
    }
    // What this step OBSERVED, for the shared behaviour file. Accumulated in memory
    // only; the write happens once, when the Apply ends.
    //
    // `uac` is Windows-only and comes from the watcher (a foreign window appeared during
    // the step) — a signal that cannot distinguish an elevation prompt from an
    // installer's own window, which the monotone design tolerates: a false positive
    // costs one label of caution, never a wrong promise.
    //
    // `forbidden` is `run_in_pty`'s raw signal: `forbidden::is403` matched
    // the step's output. It is deliberately NOT the `blocked` computed above — that one is
    // `!ok && forbidden`, the UI verdict, which is right for a modal ("offer Retry") and
    // wrong for a fact ("this package's source is blocked here"). A tool that prints a 403
    // for one mirror and then succeeds from another still met the firewall, and the row label
    // wants to know. The cost is the 403 rule's own imprecision — a `\b403\b` near a
    // download-failure phrase — which the same monotone tolerance covers.
    //
    // Merged unconditionally, including on a failure or a cancel: a step that hit a 403
    // FAILED, and that is exactly the observation worth keeping. `slow_secs` from a
    // cancelled step under-states the real duration, which the ratchet absorbs — a max
    // never goes down, so a short cancelled run cannot lower anything.
    //
    // A 403 that the user Retries runs do_step AGAIN, so this site is reached once per
    // ATTEMPT. Nothing double-counts (there is no counter, only `∨`) and nothing is lost
    // (a successful retry's `forbidden: false` cannot clear the first attempt's `true`) —
    // both directly because the accumulator ratchets.
    remember_behaviour(
        state,
        &step.id,
        step.route.as_deref().unwrap_or(""),
        crate::behaviour::Facts {
            uac: saw_window,
            forbidden,
            slow_secs: started.elapsed().as_secs(),
        },
    )
    .await;
    // The same elapsed seconds, into the LOCAL cache — for the minutes shown to THIS user.
    //
    // ⚠️ The number is the same; what happens to it is the opposite. Above it is merged with
    // a `max` into a fleet-wide classification; here it is ASSIGNED, so a warm run LOWERS the
    // estimate. Reached once per ATTEMPT, exactly like the call above, and that is why the
    // two accumulators are separate: after a 403 + Retry the share must keep the WORST of the
    // two attempts and this file must keep the LAST.
    //
    // ⚠️ AND A CANCELLED OR FAILED STEP IS RECORDED TOO, which cuts differently here than
    // above. The ratchet absorbs a short cancelled run (a max never goes down); assignment
    // does not — a step killed after 2s writes 2s and under-states the next estimate. Kept
    // anyway: a filter would need to know what "properly finished" means for every route,
    // and the cost of the under-statement is one optimistic estimate that the next full run
    // corrects.
    //
    // ⚠️ THIS CALL'S ARGUMENTS ARE UNTESTED, exactly like `remember_behaviour`'s above, and
    // for the same reason: `do_step` needs a live WebSocket and a pty, so no unit test
    // reaches this line. MEASURED, not assumed — a mutant that passed `&step.name` instead of
    // `&step.id` and a hardcoded `0` instead of the elapsed seconds left all 208 tests green
    // and clippy silent. What IS pinned is everything downstream (`remember_duration`, the
    // flush, the wire lookup); the four values handed over here are verified by reading, and
    // by the real-click check Task 8 owes. Deleting the call outright is caught, but only by
    // clippy's dead-code error, not by a test.
    remember_duration(
        state,
        &step.id,
        step.route.as_deref().unwrap_or(""),
        started.elapsed().as_secs(),
    )
    .await;
    // ⭐ A failure says WHY on the share, not only THAT: the exit code and the last lines the
    // command printed. Every `ok: false` carries them — a plain failure, a Stop, and the exit 0
    // that moved nothing (`unmoved`), where the tool's own words are the best clue there is.
    let failed = !(ok && unmoved.is_none());
    append_history(
        &state.consent,
        &HistEntry {
            at: chrono::Utc::now().to_rfc3339(),
            package: step.name.clone(),
            version,
            action: action.as_str().to_string(),
            ok: !failed,
            exit: failed.then_some(code),
            tail: if failed {
                step_tail(&output, &typed, &state.consent.user)
            } else {
                String::new()
            },
            secs: started.elapsed().as_secs(),
            // What it was found at and run toward — for a version change only, so a reader of
            // the journal can tell "failed" from "succeeded without changing anything".
            found: match action {
                Action::Upgrade | Action::Downgrade => aim
                    .before
                    .and_then(|p| p.version.clone())
                    .unwrap_or_default(),
                _ => String::new(),
            },
            target: match action {
                Action::Upgrade | Action::Downgrade => target.unwrap_or_default(),
                _ => String::new(),
            },
        },
    );
    StepOutcome {
        ok: ok && unmoved.is_none(),
        blocked,
        after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_cut_by_a_read_is_decoded_whole() {
        let bytes = "pässwörd".as_bytes();
        let mut carry = Vec::new();
        let mut text = String::new();
        for chunk in bytes.chunks(1) {
            text.push_str(&decode_chunk(&mut carry, chunk));
        }
        assert_eq!(text, "pässwörd");
        assert!(carry.is_empty());
        // A byte that can never start a character is still replaced, not held forever.
        assert_eq!(decode_chunk(&mut carry, b"a\xffb"), "a\u{FFFD}b");
        // An invalid byte EARLIER in the read does not stop the cut character being held.
        assert_eq!(decode_chunk(&mut carry, b"\xff p\xc3"), "\u{FFFD} p");
        assert_eq!(decode_chunk(&mut carry, b"\xa4ss"), "\u{e4}ss");
    }

    /// The sudo prompt is looked for in what followed the last answer, and in the end only.
    #[test]
    fn a_prompt_is_read_after_the_last_answer_and_at_the_end_only() {
        let answered = "Password for al:";
        let buf = format!("{answered}\nSorry, try again.\nPassword for al:");
        assert_eq!(
            prompt_window(&buf, answered.len()),
            "\nSorry, try again.\nPassword for al:"
        );
        // Just answered: nothing after it, so the same prompt is not answered twice.
        assert_eq!(prompt_window(answered, answered.len()), "");
        // A `password` on an earlier line does not arm a later `:`.
        let near = "Checking password policy...\nSelect option:";
        assert!(!prompt_armed(&last_line(prompt_window(near, 0))));
        assert!(prompt_armed(&last_line(prompt_window(
            "Password for al:",
            0
        ))));
        assert!(prompt_armed(&last_line(prompt_window(
            "ok\n[sudo] password for al: ",
            0
        ))));
        // A `password` far back no longer arms a later `:`.
        let long = format!("password reset\n{}\nContinue:", "x".repeat(5000));
        assert!(!prompt_window(&long, 0).contains("password"));
        // On a char boundary, whatever the cut.
        let wide = "é".repeat(3000);
        assert!(prompt_window(&wide, 0).chars().all(|c| c == 'é'));
        // A long prompt, escapes and all, is still read whole.
        let prompt = format!(
            "\x1b[33mEnter password for C:{}\\store.jks:\x1b[0m",
            "\\some-long-directory".repeat(40)
        );
        assert!(prompt_window(&prompt, 0).contains("password"));
    }
    use crate::bundles::{Overrides, Posture, Step};
    use crate::managers::Outdated;
    use crate::platform::Os;

    /// A minimal Step for the forced-cask tests: only `system_id`/`pin`/`upgrade`
    /// matter here, the rest are inert defaults.
    /// A candidate needs BOTH a `doctor:` block and a `detect:` to take a binary from;
    /// one without the other is not launchable and is left out, in catalogue order.
    fn observed(present: Option<bool>, version: &str) -> crate::detect::Presence {
        crate::detect::Presence {
            present,
            version: Some(version.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_change_that_exits_ok_and_moves_nothing_is_not_a_success() {
        use crate::decision::Action;
        let before = observed(Some(true), "1.141.2");
        // MEASURED by the integrator (Windows, 2026-10-22): winget-pkgs' VS Code 1.142.0
        // manifest downloads the 1.141.2 installer; winget exits 0, three Applies in a row.
        let why = unmoved_reason(
            Action::Upgrade,
            None,
            Some(&before),
            &observed(Some(true), "1.141.2"),
            Some("1.142.0"),
            "winget",
        )
        .expect("an upgrade that left the version where it was");
        assert!(why.contains("winget"), "{why}");
        assert!(why.contains("1.141.2") && why.contains("1.142.0"), "{why}");
        // It moved: a success, even short of what was announced (a newer manifest may land).
        assert_eq!(
            unmoved_reason(
                Action::Upgrade,
                None,
                Some(&before),
                &observed(Some(true), "1.141.9"),
                Some("1.142.0"),
                "winget",
            ),
            None
        );
        // Nothing to compare: no verdict, never a guess.
        for (b, a) in [("", "1.0"), ("1.0", ""), ("?", "")] {
            assert_eq!(
                unmoved_reason(
                    Action::Upgrade,
                    None,
                    Some(&observed(Some(true), b)),
                    &observed(Some(true), a),
                    None,
                    "winget",
                ),
                None,
                "{b} → {a}"
            );
        }
        assert_eq!(
            unmoved_reason(
                Action::Upgrade,
                None,
                None,
                &observed(Some(true), "1.0"),
                None,
                "winget"
            ),
            None,
            "no before (a retry) → no verdict"
        );
        // An install or an uninstall is judged by presence, elsewhere.
        for act in [Action::Install, Action::Uninstall] {
            assert_eq!(
                unmoved_reason(act, None, Some(&before), &before, None, "winget"),
                None
            );
        }
    }

    #[test]
    fn an_upgrade_that_finds_the_tool_already_at_the_announced_version_is_a_success() {
        use crate::decision::Action;
        // The review of `self-updating`, finding 1, on SHIPPED content: Copilot CLI (winget,
        // `latest`) updated itself to 1.0.102 while winget's receipt still says v1.0.96. The
        // scan lists it outdated, winget's upgrade exits 0 — or with its benign "no applicable
        // upgrade" — and `detect:` reads 1.0.102 before and after. It is where it was run to
        // be: no verdict, whatever the spelling of the announcement.
        let current = observed(Some(true), "1.0.102");
        for target in ["v1.0.102", "1.0.102", "1.0.101"] {
            assert_eq!(
                unmoved_reason(
                    Action::Upgrade,
                    None,
                    Some(&current),
                    &current,
                    Some(target),
                    "winget",
                ),
                None,
                "{target}"
            );
        }
        assert!(benign_code(Action::Upgrade, -1978335189));
        // `latest` is the same rule as silence.
        assert_eq!(
            unmoved_reason(
                Action::Upgrade,
                Some("latest"),
                Some(&current),
                &current,
                Some("v1.0.102"),
                "winget",
            ),
            None
        );
        // Still behind the announcement and unmoved: the failure it always was.
        let behind = observed(Some(true), "1.0.96");
        assert!(unmoved_reason(
            Action::Upgrade,
            None,
            Some(&behind),
            &behind,
            Some("v1.0.102"),
            "winget",
        )
        .is_some());
        // A spelling is not a move: `v1.0.96` then `1.0.96` moved nothing.
        assert!(unmoved_reason(
            Action::Upgrade,
            None,
            Some(&observed(Some(true), "v1.0.96")),
            &behind,
            Some("1.0.102"),
            "winget",
        )
        .is_some());
    }

    #[test]
    fn a_portable_package_the_tool_rewrote_is_forced_only_forward() {
        use crate::decision::Action;
        // MEASURED (Windows VM, winget v1.29, 2026-10-25): jq installed portable at 1.7.1, its
        // exe replaced by 1.8.2. `winget upgrade` → "Unable to remove Portable package as it has
        // been modified; to override this check use --force", exit -1978335145. With `--force`:
        // exit 0, the record says 1.8.2, `jq` still answers. A tool that updates itself in
        // winget's folder (Copilot CLI) meets this on every manager upgrade.
        assert!(portable_refused(
            Action::Upgrade,
            Some("winget"),
            false,
            -1978335145
        ));
        assert!(
            !portable_refused(Action::Upgrade, Some("winget"), true, -1978335145),
            "self"
        );
        assert!(!portable_refused(
            Action::Upgrade,
            Some("brew"),
            false,
            -1978335145
        ));
        assert!(!portable_refused(
            Action::Install,
            Some("winget"),
            false,
            -1978335145
        ));
        assert!(!portable_refused(Action::Upgrade, Some("winget"), false, 1));
        // Forced only where it takes the tool forward or to the same version: never backwards.
        use PortableChoice::*;
        assert_eq!(portable_choice("1.8.2", Some("1.8.2"), true), Force);
        assert_eq!(portable_choice("1.8.0", Some("v1.8.2"), true), Force);
        assert_eq!(portable_choice("1.0.103", Some("v1.0.102"), true), Ahead);
        // No evidence, no force: the refusal stands as the failure it is.
        assert_eq!(portable_choice("", Some("1.8.2"), true), Neither);
        assert_eq!(portable_choice("?", Some("1.8.2"), true), Neither);
        assert_eq!(portable_choice("1.8.2", None, true), Neither);
        // The pre-merge review: a version that is the manager's OWN record says nothing of the
        // tool — the record lags by definition, so it always read "behind" and forced 1.0.102
        // over a tool at 1.0.103. Only a reading of the binary may force.
        assert_eq!(portable_choice("v1.0.96", Some("v1.0.102"), false), Neither);
        // And only a plain `winget upgrade`: a flag appended to an author's compound command
        // would land on its last part.
        assert!(forceable(
            "winget upgrade --id jqlang.jq -e --source winget --accept-source-agreements"
        ));
        assert!(!forceable("winget upgrade --id a.b -e; do-something"));
        assert!(!forceable("winget upgrade --id a.b -e && b"));
        assert!(!forceable("custom-updater.exe"));
    }

    #[test]
    fn a_change_that_finds_the_tool_current_and_moves_nothing_is_held_as_a_success() {
        // The pre-merge review, finding 3: at or past the announcement is a success — but when
        // nothing moved, not even the manager's record, the next Apply would run the same
        // upgrade again, and again. Held like a tool ahead of its manager, never a failure.
        let p = |run: &str, managed: Option<&str>| crate::detect::Presence {
            managed: managed.map(str::to_string),
            ..observed(Some(true), run)
        };
        let t = Some("1.8.2");
        assert!(settled_without_moving(
            Some(&p("1.8.2", Some("1.7.1"))),
            &p("1.8.2", Some("1.7.1")),
            t
        ));
        assert!(settled_without_moving(
            Some(&p("1.8.2", None)),
            &p("v1.8.2", None),
            t
        ));
        // The record moved: realigned, nothing to hold.
        assert!(!settled_without_moving(
            Some(&p("1.8.2", Some("1.7.1"))),
            &p("1.8.2", Some("1.8.2")),
            t
        ));
        // What runs moved: a change, nothing to hold.
        assert!(!settled_without_moving(
            Some(&p("1.7.1", None)),
            &p("1.8.2", None),
            t
        ));
        // Below the target: the verdict's business, not this.
        assert!(!settled_without_moving(
            Some(&p("1.6", Some("1.7.1"))),
            &p("1.6", Some("1.7.1")),
            t
        ));
        // Finding 5: a mise `node` 22.1 ahead on PATH, winget's 20.11 that did not move — held,
        // so the Apply stops knocking; never a rerun on every Apply.
        assert!(settled_without_moving(
            Some(&p("22.1.0", Some("20.11.0"))),
            &p("22.1.0", Some("20.11.0")),
            Some("21.0.0")
        ));
        // No before, no target: no evidence.
        assert!(!settled_without_moving(None, &p("1.8.2", None), t));
        assert!(!settled_without_moving(
            Some(&p("1.8.2", None)),
            &p("1.8.2", None),
            None
        ));
    }

    #[test]
    fn the_managers_copy_moving_short_of_the_target_is_still_a_move() {
        use crate::decision::Action;
        let p = |run: &str, managed: &str| crate::detect::Presence {
            managed: Some(managed.into()),
            ..observed(Some(true), run)
        };
        // winget took its node from 20.11 to 21.0 toward 22.1; the mise node on PATH stays.
        assert_eq!(
            unmoved_reason(
                Action::Upgrade,
                None,
                Some(&p("20.11.0", "20.11.0")),
                &p("20.11.0", "21.0.0"),
                Some("22.1.0"),
                "winget",
            ),
            None
        );
        // Two spellings of one version (the listing before, the probe after) are no move.
        assert!(unmoved_reason(
            Action::Upgrade,
            None,
            Some(&p("20.11.0", "20.11.0")),
            &p("20.11.0", "20.11.0.0"),
            Some("22.1.0"),
            "winget",
        )
        .is_some());
    }

    #[test]
    fn a_held_downgrade_is_still_seen_by_the_scan() {
        use crate::decision::Action;
        // The pre-merge review, finding 2: the hold was keyed on the ACTION's command, recorded
        // with `downgrade` and looked up by the scan with `upgrade` — the measured nu case came
        // back as pending work after a Refresh. The key is the row's commands, whatever the action.
        let mut step = brew_step("nushell", Some("0.113.1"));
        step.downgrade = Some("brew uninstall nushell && brew install nushell@0.113.1".into());
        let mut memory = std::collections::HashMap::new();
        memory.insert(
            "nushell".to_string(),
            Unmoved {
                found: "0.116.1".into(),
                target: "0.113.1".into(),
                reason: "brew reported success, but 0.116.1 is installed, not 0.113.1".into(),
                cmd: change_command(&step),
                ahead: false,
            },
        );
        for act in [Action::Upgrade, Action::Downgrade] {
            assert!(
                hold_for(&memory, &step, act, Some("0.116.1"), None).is_some(),
                "{act:?}"
            );
        }
        // Another command — the author moved the row to its own update — lifts it.
        step.upgrade = Some("nu-update".into());
        assert!(hold_for(&memory, &step, Action::Downgrade, Some("0.116.1"), None).is_none());
    }

    #[test]
    fn a_tool_ahead_of_its_manager_is_held_but_never_called_a_failure() {
        // Copilot CLI rewrote itself to 1.0.103 while winget-pkgs offers 1.0.102: forcing would
        // take it BACK. It is left as it is, and remembered, so the next Apply does not knock on
        // winget again for nothing — a hold that is a success.
        let u = ahead_of_manager("1.0.103", "1.0.102", "winget upgrade --id GitHub.Copilot");
        assert!(u.ahead);
        assert!(
            u.reason.contains("1.0.103") && u.reason.contains("1.0.102"),
            "{}",
            u.reason
        );
        let mut memory = std::collections::HashMap::new();
        memory.insert("copilot-cli".to_string(), u);
        let cmd = "winget upgrade --id GitHub.Copilot";
        assert!(held_unmoved(
            &memory,
            "copilot-cli",
            "1.0.103",
            "1.0.102",
            cmd
        ));
        // winget-pkgs catches up: a new target, one more try (it will be forced forward then).
        assert!(!held_unmoved(
            &memory,
            "copilot-cli",
            "1.0.103",
            "1.0.104",
            cmd
        ));
    }

    #[test]
    fn the_copy_the_manager_upgraded_counts_even_when_another_one_runs() {
        use crate::decision::Action;
        // The review's finding 2: winget upgrades Node.js, but a mise `node` comes first on PATH,
        // so `node --version` does not move. The manager's own reading did: its copy is the one
        // it was asked to upgrade, and it did. A success — never "still installed".
        let with = |run: &str, managed: &str| crate::detect::Presence {
            managed: Some(managed.into()),
            ..observed(Some(true), run)
        };
        assert_eq!(
            unmoved_reason(
                Action::Upgrade,
                None,
                Some(&with("20.11.0", "20.11.0")),
                &with("20.11.0", "22.1.0"),
                Some("22.1.0"),
                "winget",
            ),
            None
        );
        // Neither copy moved: the failure stands.
        assert!(unmoved_reason(
            Action::Upgrade,
            None,
            Some(&with("20.11.0", "20.11.0")),
            &with("20.11.0", "20.11.0"),
            Some("22.1.0"),
            "winget",
        )
        .is_some());
        // Toward an exact pin, what RUNS is the verdict: winget's word was exactly what lied
        // in the measured downgrade.
        assert!(unmoved_reason(
            Action::Downgrade,
            Some("0.113.1"),
            Some(&with("0.116.1", "0.116.1")),
            &with("0.116.1", "0.113.1"),
            Some("0.113.1"),
            "winget",
        )
        .is_some());
    }

    #[test]
    fn a_version_change_toward_a_pin_succeeds_only_at_the_pin() {
        use crate::decision::Action;
        // The 2026-10-20 request, measured in the VM: `winget install --version 0.113.1
        // --force --uninstall-previous` printed "Successfully installed", exit 0, and `nu`
        // still answered 0.116.1. A step toward a pin is a success when the pin is READ.
        let above = observed(Some(true), "0.116.1");
        for act in [Action::Downgrade, Action::Upgrade] {
            let why = unmoved_reason(
                act,
                Some("0.113.1"),
                Some(&above),
                &observed(Some(true), "0.116.1"),
                Some("0.113.1"),
                "winget",
            )
            .expect("off the pin after the step");
            assert!(why.contains("0.116.1") && why.contains("0.113.1"), "{why}");
            assert_eq!(
                unmoved_reason(
                    act,
                    Some("0.113.1"),
                    Some(&above),
                    &observed(Some(true), "0.113.1"),
                    Some("0.113.1"),
                    "winget",
                ),
                None
            );
        }
        // At the pin's own precision, as everywhere else.
        assert_eq!(
            unmoved_reason(
                Action::Upgrade,
                Some("2.50.1"),
                Some(&observed(Some(true), "2.49.0")),
                &observed(Some(true), "2.50.1.windows.1"),
                Some("2.50.1"),
                "winget",
            ),
            None
        );
    }

    #[test]
    fn a_change_that_moved_nothing_is_not_run_again_for_the_same_target() {
        let mut memory = std::collections::HashMap::new();
        memory.insert(
            "visual-studio-code".to_string(),
            Unmoved {
                found: "1.141.2".into(),
                target: "1.142.0".into(),
                reason: "winget reported success…".into(),
                cmd: "winget upgrade code".into(),
                ahead: false,
            },
        );
        let held = |id: &str, found: &str, target: &str| {
            held_unmoved(&memory, id, found, target, "winget upgrade code")
        };
        assert!(held("visual-studio-code", "1.141.2", "1.142.0"));
        // A new announcement — the manifest changed — is worth one more try.
        assert!(!held("visual-studio-code", "1.141.2", "1.142.1"));
        assert!(!held("visual-studio-code", "1.141.2", ""));
        // The machine moved since — by hand, or by the tool itself: what was remembered no
        // longer describes it. Detect, don't remember.
        assert!(!held("visual-studio-code", "1.141.5", "1.142.0"));
        assert!(!held("visual-studio-code", "", "1.142.0"));
        assert!(!held("git", "1.141.2", "1.142.0"));
        // A change toward no named version is held too, while the version stays.
        memory.insert(
            "git".to_string(),
            Unmoved {
                found: "2.50.1".into(),
                target: String::new(),
                reason: "brew reported success, but 2.50.1 is still installed".into(),
                cmd: "brew upgrade git".into(),
                ahead: false,
            },
        );
        assert!(held_unmoved(
            &memory,
            "git",
            "2.50.1",
            "",
            "brew upgrade git"
        ));
        assert!(!held_unmoved(
            &memory,
            "git",
            "2.50.2",
            "",
            "brew upgrade git"
        ));
        // The review's finding 3: held after the manager's upgrade, the author switches to
        // `version: self` + `update:`. Another command is another attempt — never held by
        // what the first one did.
        assert!(!held_unmoved(
            &memory,
            "git",
            "2.50.1",
            "",
            "git self-update"
        ));
        // A spelling is not a move: winget's listing writes `v`, the tool does not.
        assert!(held_unmoved(
            &memory,
            "git",
            "v2.50.1",
            "",
            "brew upgrade git"
        ));
        // The target: an exact pin owns it; otherwise what the manager announced.
        assert_eq!(
            step_target(Some("0.113.1"), Some("0.116.1")).as_deref(),
            Some("0.113.1")
        );
        assert_eq!(
            step_target(Some("latest"), Some("1.1")).as_deref(),
            Some("1.1")
        );
        assert_eq!(
            step_target(Some("self"), Some("1.1")).as_deref(),
            Some("1.1")
        );
        assert_eq!(step_target(None, None), None);
        assert_eq!(step_target(None, Some(" ")), None);
    }

    #[test]
    fn a_step_changed_its_package_when_the_version_or_the_presence_moved() {
        let before = observed(Some(true), "0.113.1");
        // The Nushell case: upgraded in place, present before and after.
        assert!(step_changed_package(
            Some(&before),
            Some(&observed(Some(true), "0.116.1"))
        ));
        // An install: absent before.
        assert!(step_changed_package(
            Some(&observed(Some(false), "")),
            Some(&observed(Some(true), "0.116.1"))
        ));
        // ⭐ winget's "already current": the step ran, exited benign, and changed NOTHING —
        // it must not cost a probe per dependent. Whitespace is not a version change either.
        assert!(!step_changed_package(
            Some(&before),
            Some(&observed(Some(true), " 0.113.1\n"))
        ));
        // A failed or skipped step re-probed nothing: no claim of change.
        assert!(!step_changed_package(Some(&before), None));
        // Nothing observed before, something after: not knowably unchanged.
        assert!(step_changed_package(None, Some(&before)));
    }

    #[test]
    fn dependents_are_rechecked_once_per_apply_and_never_ahead_of_the_plan() {
        // 0 Nushell; 1 Polars, 2 Excel, 3 Formats require it; 4 Git does not.
        let reqs: Vec<Vec<String>> = vec![
            vec![],
            vec!["Nushell".into()],
            vec!["Nushell".into()],
            vec!["Nushell".into()],
            vec![],
        ];
        let mut done = std::collections::HashSet::new();
        // Excel is still ahead in the plan: it runs anyway, after Nushell.
        let ahead = std::collections::HashSet::from([2]);
        assert_eq!(
            dependents_to_recheck(&reqs, "Nushell", &ahead, &mut done),
            vec![1, 3]
        );
        // A second change of the same requirement (a retried step) re-checks nothing again.
        assert!(dependents_to_recheck(&reqs, "Nushell", &ahead, &mut done).is_empty());
        assert!(dependents_to_recheck(&reqs, "Git", &Default::default(), &mut done).is_empty());
    }

    #[test]
    fn a_version_in_a_sentence_is_never_blank() {
        assert_eq!(shown_version(&observed(Some(true), " 0.116.1 ")), "0.116.1");
        assert_eq!(shown_version(&observed(Some(false), "")), "?");
        assert_eq!(shown_version(&Default::default()), "?");
    }

    #[test]
    fn doctor_candidates_need_a_doctor_block_and_a_detect() {
        let mut declared = brew_step("a", None);
        declared.name = "Agent".into();
        declared.detect = Some("agent --version".into());
        declared.doctor = Some(Default::default());
        let mut blind = brew_step("b", None);
        blind.doctor = Some(Default::default());
        blind.detect = None;
        let mut plain = brew_step("c", None);
        plain.detect = Some("cc".into());
        let out = doctor_candidates(&[blind, declared, plain]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Agent");
        assert_eq!(
            out[0].word, "agent",
            "the first word of detect is the binary"
        );
        assert!(out[0].clean.is_none());
    }

    /// The Doctor tab exists only for a catalogue that declares a candidate — declared,
    /// not installed. The floor alone (always there) does not earn the tab.
    #[test]
    fn the_doctor_tab_needs_a_declared_candidate() {
        assert!(
            !doctor_declared(&[brew_step("c", None)]),
            "no doctor: block, no tab"
        );
        let mut blind = brew_step("b", None);
        blind.doctor = Some(Default::default());
        blind.detect = None;
        assert!(
            !doctor_declared(&[blind]),
            "a doctor: without detect: is not a candidate, so still no tab"
        );
        let mut declared = brew_step("a", None);
        declared.detect = Some("agent --version".into());
        declared.doctor = Some(Default::default());
        assert!(
            doctor_declared(&[brew_step("c", None), declared]),
            "one declared candidate is enough"
        );
    }

    fn brew_step(system_id: &str, pin: Option<&str>) -> Step {
        Step {
            doctor: None,
            id: system_id.into(),
            bundle: String::new(),
            name: system_id.into(),
            description: String::new(),
            install: None,
            uninstall: None,
            upgrade: Some(format!("brew upgrade --yes {system_id}")),
            downgrade: None,
            route: Some("brew".into()),
            system_id: Some(system_id.into()),
            detect: None,
            check: None,
            is_config: false,
            is_extension: false,
            version_regex: None,
            pin: pin.map(|p| p.into()),
            scope: None,
            requires: Vec::new(),
            posture: Posture::OptIn,
            categories: Vec::new(),
            overrides: Overrides::default(),
            update_each_apply: false,
        }
    }

    #[test]
    fn a_brew_pin_at_brews_current_version_runs_plain_brew() {
        use crate::decision::Action::*;
        use crate::managers::BrewPin;
        let s = brew_step("nushell", Some("0.116.1"));
        let cur = Some(&BrewPin::Current);
        let cmd = |a| super::brew_plan(a, &s, cur).unwrap().map(|(c, _)| c);
        assert_eq!(cmd(Install).as_deref(), Some("brew install --yes nushell"));
        // `install`, as the stored `id@ver` form was: `brew upgrade` exits 1 when the plain
        // formula is not brew's own (a row `detect:` finds through `node@24`, MEASURED "not
        // installed"), where `brew install` upgrades an outdated one.
        assert_eq!(cmd(Upgrade).as_deref(), Some("brew install --yes nushell"));
        assert_eq!(
            cmd(Downgrade).as_deref(),
            Some("brew uninstall nushell && brew install --yes nushell")
        );
        let (_, note) = super::brew_plan(Upgrade, &s, cur).unwrap().unwrap();
        assert!(note.contains("0.116.1"), "the row says why: {note}");
        assert_eq!(super::brew_plan(Uninstall, &s, cur), Ok(None));
    }

    #[test]
    fn a_held_absent_row_takes_the_rows_that_require_it_out_of_the_plan() {
        // Found at the review: nushell absent and held (brew cannot serve its pin) still read
        // "will be present", so its plugin ran in the same Apply and landed orphaned.
        use crate::decision::Action::*;
        use crate::deps::{make_index, DepNode};
        let node = |name: &str, requires: &[&str], present: bool| DepNode {
            name: name.into(),
            requires: requires.iter().map(|r| r.to_string()).collect(),
            will_be_present: true,
            present_now: present,
            reachable: true,
        };
        let mut nodes = vec![
            node("plugin", &["nushell"], false),
            node("nushell", &[], false),
            node("config", &["plugin"], false),
            node("old-tool", &["nushell"], true),
            node("jq", &[], false),
        ];
        let idx = make_index(&nodes);
        let mut plan = vec![(0, Install), (2, Install), (3, Uninstall), (4, Install)];
        let dropped = super::drop_requiring_held(&mut plan, &mut nodes, &idx, &[1]);
        assert_eq!(
            dropped,
            vec![
                (0, "requires nushell".to_string()),
                (2, "requires plugin".to_string())
            ],
            "a chain goes with it, each row saying what it waits for"
        );
        // A removal never needs its requirement; an unrelated row stays.
        assert_eq!(plan, vec![(3, Uninstall), (4, Install)]);
    }

    #[test]
    fn a_removal_the_button_did_not_announce_leaves_the_plan() {
        // Measured 2026-10-27: after a reload with a bundle off, the button said "2 installs"
        // and the plan uninstalled two more rows. The button's count is what was consented to.
        use crate::decision::Action::*;
        use std::collections::HashSet;
        let full = vec![
            (0, Install),
            (1, Uninstall),
            (2, Downgrade),
            (3, Upgrade),
            (4, Uninstall),
        ];
        let mut plan = full.clone();
        let announced: HashSet<usize> = [4].into();
        assert_eq!(
            super::drop_unannounced(&mut plan, Some(&announced)),
            vec![1, 2]
        );
        assert_eq!(
            plan,
            vec![(0, Install), (3, Upgrade), (4, Uninstall)],
            "installs and updates stay"
        );
        // A front that sends no list keeps the plan whole.
        let mut plan = full.clone();
        assert!(super::drop_unannounced(&mut plan, None).is_empty());
        assert_eq!(plan, full);
    }

    #[test]
    fn the_apply_cuts_unannounced_removals_and_says_why_before_ordering() {
        // The pure test above stays green if the wiring is removed.
        let plan = fn_body("apply_diff");
        let at = |b: &str, s: &str| b.find(s).unwrap_or_else(|| panic!("no `{s}`"));
        let cut = at(
            &plan,
            "drop_unannounced(&mut visual_plan, announced.as_ref())",
        );
        assert!(at(&plan, "visual_plan.push((i, a))") < cut);
        assert!(cut < at(&plan, "topo_sort("));
        // Not `held`: the front keeps a hold until a Refresh, and this one must not outlive the
        // click it answers, or the next Apply could never announce the removal (review, 2026-10-27).
        assert!(plan.contains("send_unannounced(socket, i)"));
        assert!(fn_body("send_unannounced").contains("\"type\": \"unannounced\""));
        let wire = fn_body("handle_socket");
        assert!(
            wire.contains("json_indices_opt(&parsed, \"removals\")"),
            "the front's list must reach the Apply"
        );
    }

    #[test]
    fn a_brew_pin_brew_cannot_serve_is_held_with_what_brew_offers() {
        use crate::decision::Action::*;
        use crate::managers::BrewPin;
        let s = brew_step("node", Some("26.8.1"));
        let v = BrewPin::Unreachable {
            offered: "26.12.1".into(),
        };
        for a in [Install, Upgrade, Downgrade] {
            assert_eq!(
                super::brew_plan(a, &s, Some(&v)),
                Err("brew offers 26.12.1, not 26.8.1".to_string()),
                "{a:?}"
            );
        }
        // Holding is about not acting: removing the package stays possible.
        assert_eq!(super::brew_plan(Uninstall, &s, Some(&v)), Ok(None));
    }

    #[test]
    fn a_brew_pin_keeps_its_stored_command_when_a_versioned_formula_exists_or_brew_is_unread() {
        use crate::decision::Action::*;
        use crate::managers::BrewPin;
        let s = brew_step("node", Some("24"));
        assert_eq!(
            super::brew_plan(Install, &s, Some(&BrewPin::Versioned)),
            Ok(None)
        );
        assert_eq!(super::brew_plan(Install, &s, None), Ok(None));
        // Not an exact pin, or not brew: nothing to decide.
        let latest = brew_step("node", Some("latest"));
        assert_eq!(
            super::brew_plan(Upgrade, &latest, Some(&BrewPin::Current)),
            Ok(None)
        );
        // `version: self`: the tool updates itself, and brew must never replace its `update:`.
        let own = brew_step("claude-code", Some("self"));
        let away = BrewPin::Unreachable {
            offered: "2.0".into(),
        };
        for a in [Install, Upgrade] {
            assert_eq!(super::brew_plan(a, &own, Some(&BrewPin::Current)), Ok(None));
            assert_eq!(
                super::brew_plan(a, &own, Some(&away)),
                Ok(None),
                "a self row is never held by brew's offer"
            );
        }
        let mut winget = brew_step("OpenJS.NodeJS", Some("26.8.1"));
        winget.route = Some("winget".into());
        assert_eq!(
            super::brew_plan(Install, &winget, Some(&BrewPin::Current)),
            Ok(None)
        );
    }

    fn seen(present: bool, version: &str) -> crate::detect::Presence {
        crate::detect::Presence {
            present: Some(present),
            version: Some(version.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_pin_is_honoured_both_ways_and_a_missing_downgrade_says_so() {
        use crate::decision::Action::*;
        // The per-row button was the only way down to a pin; it left on 2026-10-10, so the
        // batch takes the downgrade — or SAYS why it cannot, never a silent skip.
        let cases = [
            (Some(Install), true, Ok(Some(Install))),
            (Some(Uninstall), false, Ok(Some(Uninstall))),
            (Some(Upgrade), false, Ok(Some(Upgrade))),
            (Some(Downgrade), true, Ok(Some(Downgrade))),
            (Some(Downgrade), false, Err(NO_DOWNGRADE_ROUTE)),
            (None, true, Ok(None)),
        ];
        for (a, can, want) in cases {
            assert_eq!(batch_action(a, can), want, "{a:?} can_downgrade={can}");
        }
    }

    #[test]
    fn a_row_says_on_the_wire_whether_its_route_can_go_down_to_its_pin() {
        // The front previews what the Apply will do, so it must know BEFORE the click that a
        // pinned row above its pin cannot be taken down — and show it, not light a downgrade.
        let no_timings = crate::timings::Timings::new();
        let mut s = brew_step("nushell", Some("0.113.1"));
        let row = super::step_json(&s, 0, &[], &no_timings, Os::Darwin);
        assert_eq!(row["canDowngrade"], false, "no downgrade command → false");
        s.downgrade = Some("brew uninstall nushell && brew install nushell@0.113.1".into());
        let row = super::step_json(&s, 0, &[], &no_timings, Os::Darwin);
        assert_eq!(row["canDowngrade"], true);
    }

    #[test]
    fn a_row_says_on_the_wire_whether_it_runs_its_own_update() {
        // The front's twin of `action_for` reads `canSelfUpdate` from here.
        let no_timings = crate::timings::Timings::new();
        let mut s = brew_step("copilot-cli", Some("self"));
        s.upgrade = None;
        let row = super::step_json(&s, 0, &[], &no_timings, Os::Darwin);
        assert_eq!(row["selfUpdate"], false, "`self` without `update:`");
        s.upgrade = Some("copilot update".into());
        let row = super::step_json(&s, 0, &[], &no_timings, Os::Darwin);
        assert_eq!(row["selfUpdate"], true);
        assert_eq!(row["updateEachApply"], false, "run only when behind");
        s.update_each_apply = true;
        let row = super::step_json(&s, 0, &[], &no_timings, Os::Darwin);
        assert_eq!(row["updateEachApply"], true);
        let plain = brew_step("git", None);
        let row = super::step_json(&plain, 0, &[], &no_timings, Os::Darwin);
        assert_eq!(
            row["selfUpdate"], false,
            "a manager upgrade is not the tool's own"
        );
    }

    #[test]
    fn a_rows_behaviour_crosses_the_wire_under_the_names_the_front_reads() {
        // The three behaviour facts, keyed and mapped. Written because a mutant that
        // swapped `uac` for `forbidden` and hardcoded `slow: false` passed the whole suite:
        // `steps_json` lived inside `handle_socket`, which needs a live WebSocket, so the
        // mapping was unreachable from any test. The facts differ from each other on purpose
        // — a fixture with two equal booleans cannot catch a swap.
        let facts = vec![
            crate::behaviour::Facts {
                uac: true,
                forbidden: false,
                slow_secs: 0,
            },
            crate::behaviour::Facts {
                uac: false,
                forbidden: true,
                slow_secs: crate::plan_facts::SLOW_SECS,
            },
        ];
        let s = brew_step("nushell", None);
        let no_timings = crate::timings::Timings::new();

        let row = super::step_json(&s, 0, &facts, &no_timings, Os::Darwin);
        assert_eq!(row["uac"], true, "index 0 elevates");
        assert_eq!(row["forbidden"], false);
        assert_eq!(row["slow"], false, "0 seconds is unknown, not slow");

        let row = super::step_json(&s, 1, &facts, &no_timings, Os::Darwin);
        assert_eq!(row["uac"], false);
        assert_eq!(row["forbidden"], true, "index 1 is the blocked one");
        assert_eq!(
            row["slow"], true,
            "the fleet's WORST seconds stay server-side; the front is told the CLASSIFICATION"
        );

        // A row past the end reads all-false rather than panicking on a socket. `facts` is
        // built from `plan.steps` so this cannot happen today — which is exactly why nothing
        // else would notice if the lookup became `[i]`.
        let row = super::step_json(&s, 99, &facts, &no_timings, Os::Darwin);
        assert_eq!(row["uac"], false);
        assert_eq!(row["forbidden"], false);
        assert_eq!(row["slow"], false);
        assert_eq!(
            row["i"], 99,
            "and the index it was asked about is what it reports"
        );
    }

    #[test]
    fn the_local_last_seen_duration_crosses_the_wire_keyed_by_route_and_os() {
        // The ESTIMATION half. Two things a mutant could get wrong silently, so both are
        // pinned: the (id, route/os) key, and that an unmeasured row sends 0 rather than
        // borrowing a sibling route's number.
        let s = brew_step("nushell", None); // id "nushell", route "brew"
        let mut t = crate::timings::Timings::new();
        crate::timings::note_duration(&mut t, "nushell", "brew/darwin", 38);
        crate::timings::note_duration(&mut t, "nushell", "winget/windows", 242);
        let facts = vec![crate::behaviour::Facts::default()];

        let row = super::step_json(&s, 0, &facts, &t, Os::Darwin);
        assert_eq!(row["secs"], 38, "this machine's os picks brew/darwin");
        // Same package, same route, other os → the other entry. Not 38, or the os half of
        // the key is being ignored.
        let mut win = brew_step("nushell", None);
        win.route = Some("winget".into());
        let row = super::step_json(&win, 0, &facts, &t, Os::Windows);
        assert_eq!(row["secs"], 242);
        // Measured on brew, asked about winget on THIS os → unknown, not the brew number.
        let row = super::step_json(&win, 0, &facts, &t, Os::Darwin);
        assert_eq!(
            row["secs"], 0,
            "a route this machine never measured is unknown, not a sibling's duration"
        );
        // A package with no entry at all → 0, which the widget shows as unknown.
        let row = super::step_json(&brew_step("jq", None), 0, &facts, &t, Os::Darwin);
        assert_eq!(row["secs"], 0);
    }

    #[test]
    fn a_slow_classification_and_a_fast_local_estimate_travel_together() {
        // The consequence somebody will file as a bug, pinned so the two halves cannot be
        // "fixed" into agreement: 7-Zip is CLASSIFIED slow for the whole fleet (worst-seen
        // over a minute) while THIS Mac measured 1 second, its brew bottle already cached.
        // Both fields are right; they answer different questions.
        let s = brew_step("7-zip", None);
        let facts = vec![crate::behaviour::Facts {
            slow_secs: crate::plan_facts::SLOW_SECS + 182,
            ..Default::default()
        }];
        let mut t = crate::timings::Timings::new();
        crate::timings::note_duration(&mut t, "7-zip", "brew/darwin", 1);

        let row = super::step_json(&s, 0, &facts, &t, Os::Darwin);
        assert_eq!(row["slow"], true, "the FLEET's classification");
        assert_eq!(row["secs"], 1, "and this MACHINE's estimate, unreconciled");
    }

    #[test]
    fn merge_prefers_the_fresh_probe() {
        let remembered = vec![Some(seen(false, "old")), None];
        let probed = std::collections::HashMap::from([(0usize, seen(true, "new"))]);
        let out = merge_presences(2, &probed, &remembered);
        assert_eq!(out[0].present, Some(true));
        assert_eq!(out[0].version.as_deref(), Some("new"));
    }

    #[test]
    fn merge_reuses_the_connect_scan_for_unprobed_rows() {
        // THE point of the scoped re-scan: a row we did not re-probe must keep the
        // verdict the connect scan gave it — inventing `false` would make every
        // `requires` reason lie ("requires jj" on a machine that has jj).
        let remembered = vec![Some(seen(true, "1.2.3"))];
        let probed = std::collections::HashMap::new();
        let out = merge_presences(1, &probed, &remembered);
        assert_eq!(out[0].present, Some(true));
        assert_eq!(out[0].version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn merge_leaves_never_seen_rows_unknown() {
        // Never probed, never remembered → present stays None (unknown), NOT Some(false):
        // "we don't know" and "it's absent" are different claims.
        let out = merge_presences(1, &std::collections::HashMap::new(), &[]);
        assert_eq!(out[0].present, None);
    }

    #[test]
    fn appmgmt_wire_strings() {
        use crate::platform::AppMgmtStatus::*;
        assert_eq!(super::appmgmt_wire(Granted), "granted");
        assert_eq!(super::appmgmt_wire(Missing), "missing");
        assert_eq!(super::appmgmt_wire(NotApplicable), "na");
    }

    #[test]
    fn only_missing_cask_upgrade_is_blocked() {
        use crate::decision::Action::*;
        use crate::platform::AppMgmtStatus::*;
        assert!(super::cask_upgrade_blocked(Upgrade, true, Missing));
        assert!(!super::cask_upgrade_blocked(Upgrade, true, Granted));
        assert!(!super::cask_upgrade_blocked(Upgrade, true, NotApplicable));
        assert!(!super::cask_upgrade_blocked(Upgrade, false, Missing));
        assert!(!super::cask_upgrade_blocked(Install, true, Missing));
    }

    #[test]
    fn forced_cask_upgrade_only_for_unpinned_casks() {
        let s = brew_step("visual-studio-code", None);
        // cask, no pin → forced --cask --force command
        assert_eq!(
            forced_cask_upgrade_cmd(&s, Os::Darwin, true),
            Some("brew upgrade --cask --force --yes visual-studio-code".to_string())
        );
        // formula → None (trusts the precomputed step.upgrade)
        assert_eq!(forced_cask_upgrade_cmd(&s, Os::Darwin, false), None);
        // EXACT pin → None (an exact pin owns the direction, keep install-pinned cmd)
        let exact_pinned = brew_step("visual-studio-code", Some("1.130.0"));
        assert_eq!(
            forced_cask_upgrade_cmd(&exact_pinned, Os::Darwin, true),
            None
        );
        // KEYWORD pins (pending, latest) → forced command (they don't own a direction)
        let pending = brew_step("visual-studio-code", Some("pending"));
        assert_eq!(
            forced_cask_upgrade_cmd(&pending, Os::Darwin, true),
            Some("brew upgrade --cask --force --yes visual-studio-code".to_string()),
            "pending is a pin but does not own a direction, so the cask needs the forced cmd"
        );
        let latest = brew_step("visual-studio-code", Some("latest"));
        assert_eq!(
            forced_cask_upgrade_cmd(&latest, Os::Darwin, true),
            Some("brew upgrade --cask --force --yes visual-studio-code".to_string()),
            "latest is a pin but does not own a direction, so the cask needs the forced cmd"
        );
    }

    /// The shape the UI ACTUALLY sends: every index carries a desired state, so
    /// `on ∪ off` is the whole catalogue on every Apply. Building the seeds from
    /// that directly is why "scoped to the diff" scoped nothing in production.
    fn ui_shaped_apply(desired: &[bool]) -> (Vec<usize>, Vec<usize>) {
        let mut on = Vec::new();
        let mut off = Vec::new();
        for (i, &want) in desired.iter().enumerate() {
            if want {
                on.push(i)
            } else {
                off.push(i)
            }
        }
        (on, off)
    }

    /// Reads the `match kind` arms of `handle_socket` out of this very source file.
    /// Text-level on purpose: `handle_socket` needs a live socket, so the dispatch table
    /// cannot be reached from a unit test — but the arms are a literal list, and a
    /// missing one is exactly the defect. Same precedent as test/veil.test.mjs.
    fn handled_client_msgs() -> String {
        let src = include_str!("server.rs");
        let start = src
            .find("match kind {")
            .expect("handle_socket dispatches on `kind`");
        let end = src[start..]
            .find("\n            _ => {}")
            .expect("the dispatch match ends with a catch-all");
        src[start..start + end].to_string()
    }

    #[test]
    fn every_ui_locking_message_has_a_handler() {
        // THE guard for a whole class of user-facing freezes. The front locks its UI
        // before sending these and only a server message unlocks it, so an unhandled
        // verb kills the panel until the window is closed. `diff` was that bug, live,
        // with the shipped catalogue (config-atom rows in bundles/terminal.yaml).
        let arms = handled_client_msgs();
        for kind in LOCKING_CLIENT_MSGS {
            assert!(
                arms.contains(&format!("\"{kind}\"")),
                "`{kind}` locks the front's UI but handle_socket has no arm for it → \
                 the panel freezes on it with no error and no recovery"
            );
        }
    }

    #[test]
    fn a_downgrade_retried_on_a_package_already_gone_reinstalls_at_the_pin() {
        use crate::decision::Action::*;
        // A downgrade is `uninstall && install@pin`. When the reinstall half fails (a 403 on a
        // corporate network), the package is already removed — and re-running the same chain
        // fails at once on the uninstall, so the download is never attempted again. The retry
        // asks the machine first: gone means install at the pin.
        assert_eq!(retry_action(Downgrade, Some(false)), Install);
        assert_eq!(retry_action(Downgrade, Some(true)), Downgrade);
        assert_eq!(
            retry_action(Downgrade, None),
            Downgrade,
            "unknown is not absent"
        );
        assert_eq!(
            retry_action(Upgrade, Some(false)),
            Upgrade,
            "only a downgrade is re-derived"
        );
    }

    #[test]
    fn a_row_verb_is_no_longer_an_action_but_never_freezes_an_old_front() {
        // The per-row install/uninstall/upgrade/downgrade left on 2026-10-10. A stale
        // WebView may still send one: it must not ACT, and it must not FREEZE — the front
        // locked its UI before sending, and only `done` releases it.
        //
        // ⚠️ `handle_socket`'s own body, not `handled_client_msgs`: that one runs from
        // `match kind {` to the next `_ => {}`, which lies past `handle_socket` (its
        // catch-all is `other =>`) and reaches `retry_step`'s `"install" => Action::Install`.
        let arms = fn_body("handle_socket");
        for verb in ["install", "uninstall", "upgrade", "downgrade"] {
            assert!(
                !LOCKING_CLIENT_MSGS.contains(&verb),
                "`{verb}` is still listed as a locking client message"
            );
            assert!(
                !arms.contains(&format!("\"{verb}\" =>"))
                    && !arms.contains(&format!("\"{verb}\" |")),
                "`{verb}` still has an action arm"
            );
            assert!(
                RETIRED_ROW_VERBS.contains(&verb),
                "`{verb}` is not answered as retired → an old front freezes on it"
            );
        }
        assert!(
            fn_body("handle_socket").contains("RETIRED_ROW_VERBS"),
            "a retired verb is not answered with `done` → an old front freezes"
        );
    }

    #[test]
    fn an_exact_pinned_row_is_always_a_seed() {
        // An EXACT pin replaces "latest" as the reference, so `action_for` ignores the
        // machine-wide `outdated` flag when an exact pin is set. Consequence: a row
        // with an exact pin that is present and wanted present looks converged to a
        // presence-only test and was never re-probed — while the thing that actually
        // decides is its installed VERSION, read from `last_seen`.
        //
        // The exact pin itself cannot go stale (it is a literal in the YAML). The
        // remembered version can. Upgrade Nushell by hand between the connect scan and
        // an Apply and Talos answers "satisfied" against the old number. The reverse is
        // worse: a remembered version ABOVE the pin yields Downgrade — the only
        // destructive path, uninstall+reinstall — decided on data nobody re-checked.
        //
        // ⚠️ Keywords (pending, latest) do NOT make version the deciding fact, so they
        // must NOT seed — seeding them would re-introduce the serial scan cost the
        // batching work removed.
        let last_seen = vec![Some(seen(true, "0.113.0"))];
        // Simulate an exact pin: predicate returns true for index 0
        let is_exact_pinned = |i: usize| i == 0;
        assert_eq!(
            seeds_for_rescan(&[0], &[], &last_seen, &|_| false, &is_exact_pinned),
            vec![0],
            "an EXACT-pinned row must be re-probed even when presence matches and not outdated"
        );
        // Unpinned, present, wanted, not outdated → excluded. The exact-pin clause
        // must not widen the scoping back into probing everything.
        assert!(seeds_for_rescan(&[0], &[], &last_seen, &|_| false, &|_| false).is_empty());
        // Keyword pins (pending, latest) → excluded. They do not make version the
        // deciding fact, so they must not seed.
        let not_pinned = |_: usize| false; // simulates pending or latest (not exact)
        assert!(
            seeds_for_rescan(&[0], &[], &last_seen, &|_| false, &not_pinned).is_empty(),
            "a keyword pin (pending/latest) must NOT seed — it does not own a version"
        );
        // An exact-pinned row wanted ABSENT is governed by presence, not by the pin: it
        // is already a seed via the desire mismatch, and the pin is irrelevant to an
        // uninstall. No special case needed — assert it stays out of double-counting.
        let present = vec![Some(seen(true, "0.113.0"))];
        assert_eq!(
            seeds_for_rescan(&[], &[0], &present, &|_| false, &is_exact_pinned),
            vec![0]
        );
    }

    #[test]
    fn a_stale_last_seen_makes_the_next_apply_skip_a_real_action() {
        // THE invariant that makes the scoping safe over TIME, not just once.
        //
        // Scenario: the user unchecks a package and Applies. It uninstalls fine. Then
        // they re-check it and Apply again. If `last_seen` still says "present" from
        // the connect scan, this row reads as desire==observation → NOT a seed → never
        // probed → `merge_presences` feeds the stale `present: true` to action_for,
        // which answers "nothing to do". The install silently never happens.
        //
        // ⚠️ SCOPE OF THIS TEST, stated plainly: it DOCUMENTS why the invariant
        // matters, it does not enforce it. `seeds_for_rescan` was never the faulty
        // part — the missing `remember_presence` call in `do_step` was. Deleting that
        // call leaves this test green (checked, by deleting it). The enforcement lives
        // in `every_presence_observation_is_recorded` below, which reads the call
        // sites; keep the two together.
        let after_uninstall = seen(false, "");
        let stale = seen(true, "1.0");
        // Fresh observation recorded → wanting it present again IS a seed.
        assert_eq!(
            seeds_for_rescan(&[0], &[], &[Some(after_uninstall)], &|_| false, &|_| false),
            vec![0],
            "a row observed absent must be re-probed when wanted present"
        );
        // Stale observation → the action is invisible. This is the failure mode.
        assert!(
            seeds_for_rescan(&[0], &[], &[Some(stale)], &|_| false, &|_| false).is_empty(),
            "a stale `present` really does hide the install — hence the rule above"
        );
    }

    #[test]
    fn every_presence_observation_is_recorded() {
        // THE enforcing guard. Three sites call `detect_present_detailed`; every one
        // of them MUST hand the verdict to `remember_presence`, because the Apply
        // re-scan now narrows against `last_seen` and a site that observes without
        // recording makes a real action invisible (uncheck+Apply removes it,
        // re-check+Apply answers "nothing to do" against a state that is gone).
        //
        // Text-level, deliberately: `do_step` and `scan_and_emit` need a live socket
        // and a live pty, so no unit test can reach them — but the defect is the
        // ABSENCE OF A CALL, which the source shows exactly. Same technique as
        // test/veil.test.mjs. Verified to bite: deleting `do_step`'s call fails this.
        let src = include_str!("server.rs");
        // Ignore this test module, or its own mentions would satisfy the assertions.
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];

        // Assert PER SITE rather than by counting: a raw count is satisfied by any
        // four mentions anywhere, which is how a guard ends up not guarding. Each
        // probe site is named by the comment that introduces it, and the recording
        // call must appear in the block that follows.
        for (site, anchor) in [
            ("the connect / Refresh scan", "// Remember the verdict:"),
            ("the Apply re-scan", "// Narrate BEFORE probing:"),
            (
                "do_step's post-action re-probe",
                "// Re-detect presence AFTER any successful action",
            ),
        ] {
            let at = code
                .find(anchor)
                .unwrap_or_else(|| panic!("{site}: anchor comment gone — re-point this test"));
            assert!(
                code[at..(at + 1600).min(code.len())].contains("remember_presence(state"),
                "{site} observes a presence but does not record it → the next Apply \
                 narrows against a stale `last_seen` and a real action becomes invisible"
            );
        }
    }

    #[test]
    fn the_vscode_snapshot_is_taken_once_per_scan_not_once_per_row() {
        // Text-level, same technique and the same reason as the guard above: both scans live in
        // async WebSocket handlers that need a live socket and a live pty, so no unit test can
        // reach them — but the defect here is a call in the WRONG PLACE, which the source shows
        // exactly. And the wrong place is cheap to reach: `snapshot` takes `os` and a home, both
        // of which are in scope inside the loops too, so moving the call one block down compiles,
        // passes every other test, and costs one `code --version` per package on a scan that is
        // serial on purpose (~0.2 s × 33 rows) while risking microsoft/vscode#302026 — a GUI
        // window — once per row.
        let src = include_str!("server.rs");
        // Ignore this test module, or its own mentions would satisfy the assertions.
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];

        // Two scans OBSERVE presence with the snapshot; the third site (do_step's post-action
        // re-probe) deliberately does not — it goes through `detect_present_detailed`, which
        // passes None, and the row is re-probed by `seeds_for_rescan` on the next Apply. So the
        // expected count is 2, not 3: a 3 here means someone threaded it into the per-row probe.
        assert_eq!(
            code.matches("crate::vscode::snapshot(").count(),
            2,
            "the snapshot must be built exactly twice — once per scan. A third build means it \
             moved into a per-row path; two builds in one scan means the other scan lost its own."
        );
        // The log line is Task 7's evidence at a real click: it must print once per scan.
        assert_eq!(
            code.matches("[scan] vscode host=").count(),
            1,
            "the once-per-scan log line is how the property is verified at a real click"
        );

        // ⚠️ Anchored by a UNIQUE marker first, then by the loop header, because
        // `for (i, step) in steps.iter().enumerate()` occurs TWICE in this file (the scan and
        // the visual plan). A bare `find` on it happens to hit the scan today only because the
        // scan comes first — an ordering, not a fact, and the guard would then be checking a
        // loop that probes nothing.
        for (site, unique_marker, loop_header) in [
            (
                "the connect / Refresh scan",
                "[scan] vscode host=",
                "for (i, step) in steps.iter().enumerate() {",
            ),
            (
                "the Apply re-scan",
                "// SERIAL live re-scan (presence).",
                "for (nth, &i) in to_probe.iter().enumerate() {",
            ),
        ] {
            let from = code
                .find(unique_marker)
                .unwrap_or_else(|| panic!("{site}: anchor `{unique_marker}` gone — re-point this"));
            let at = from
                + code[from..].find(loop_header).unwrap_or_else(|| {
                    panic!("{site}: the loop header moved — re-point this test")
                });
            assert!(
                code[..at].contains("crate::vscode::snapshot("),
                "{site}: the snapshot must be built BEFORE the per-package loop it feeds"
            );
            // From the loop header to the probe call: the whole prologue of one iteration. A
            // per-row snapshot would have to be built in there.
            let probe = code[at..]
                .find("detect_present_detailed_with_vscode(")
                .unwrap_or_else(|| {
                    panic!("{site}: does not pass the snapshot to the probe at all")
                });
            let prologue = &code[at..at + probe];
            assert!(
                !prologue.contains("crate::vscode::snapshot("),
                "{site} builds the snapshot INSIDE its loop → one `code --version` per row on a \
                 serial scan, which is exactly what taking it once was for"
            );
            assert!(
                prologue.contains("std::sync::Arc::clone(&vs)"),
                "{site} must share the one snapshot by Arc rather than rebuild or clone it"
            );
        }
    }

    #[test]
    fn a_fresh_outdated_map_is_reused_a_stale_or_missing_one_is_not() {
        use std::time::{Duration, Instant};
        let at = Instant::now();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "git.git".to_string(),
            crate::managers::Outdated {
                current: "2.55.0.3".into(),
                available: "2.55.0.5".into(),
                is_cask: false,
            },
        );
        let seen = OutdatedSeen { at, map };

        // Within the window: reused, and it is the very map the scan read.
        let reused = reusable_outdated(Some(&seen), at + Duration::from_secs(60));
        assert_eq!(reused.map(|s| s.map.len()), Some(1));
        // At the edge and beyond: stale, the Apply scans again.
        assert!(reusable_outdated(Some(&seen), at + OUTDATED_FRESH_FOR).is_none());
        assert!(reusable_outdated(Some(&seen), at + Duration::from_secs(3600)).is_none());
        // Nothing recorded (the scan failed, or a step acted since): scan again.
        assert!(reusable_outdated(None, at).is_none());
    }

    #[test]
    fn a_fully_converged_plan_needs_no_probes_at_all() {
        // THE regression guard. 30 packages, every one already in its desired state,
        // fed in the shape the UI really sends (all 30 split across on/off). The
        // scope must COLLAPSE. Before this fix it was all 30 — the "scoped re-scan"
        // was a no-op in production, and my 5/30 measurement came from a hand-driven
        // WS probe with a single index, a message shape the UI never sends.
        let n = 30;
        let desired: Vec<bool> = (0..n).map(|i| i % 2 == 0).collect();
        let (on, off) = ui_shaped_apply(&desired);
        assert_eq!(on.len() + off.len(), n, "the UI sends every index");
        let last_seen: Vec<Option<crate::detect::Presence>> = desired
            .iter()
            .map(|&want| Some(seen(want, "1.0"))) // machine already matches
            .collect();
        let seeds = seeds_for_rescan(&on, &off, &last_seen, &|_| false, &|_| false);
        assert!(
            seeds.is_empty(),
            "nothing to do → nothing to probe, got {seeds:?}"
        );
    }

    #[test]
    fn a_row_whose_desire_differs_from_what_we_saw_is_a_seed() {
        // want present + seen absent → install pending; want absent + seen present
        // → uninstall pending. Both must be re-probed before acting.
        let last_seen = vec![Some(seen(false, "")), Some(seen(true, "1.0"))];
        assert_eq!(
            seeds_for_rescan(&[0], &[1], &last_seen, &|_| false, &|_| false),
            vec![0, 1]
        );
    }

    #[test]
    fn an_outdated_row_is_a_seed_even_though_presence_matches() {
        // desired present + seen present is NOT "nothing to do" when the row is
        // behind: an upgrade IS an action. The batched outdated scan runs first, so
        // this is known before the seeds are chosen.
        let last_seen = vec![Some(seen(true, "1.0"))];
        assert!(seeds_for_rescan(&[0], &[], &last_seen, &|_| false, &|_| false).is_empty());
        assert_eq!(
            seeds_for_rescan(&[0], &[], &last_seen, &|i| i == 0, &|_| false),
            vec![0]
        );
    }

    #[test]
    fn should_seed_outdated_row_rejects_pending_pins() {
        // Direct test of the production rule: a `pending` pin makes an upgrade unreachable,
        // so an outdated row with `pending` must NOT seed.
        assert!(!should_seed_outdated_row(Some("pending"), true));
        assert!(!should_seed_outdated_row(Some("Pending"), true)); // case-insensitive
        assert!(!should_seed_outdated_row(Some("PENDING"), true));
    }

    #[test]
    fn should_seed_outdated_row_accepts_latest_and_no_pin() {
        // `latest` and no pin both mean "chase the newest", so outdated DOES mean upgrade.
        assert!(should_seed_outdated_row(Some("latest"), true));
        assert!(should_seed_outdated_row(Some("Latest"), true)); // case-insensitive
        assert!(should_seed_outdated_row(None, true)); // no pin declared
    }

    #[test]
    fn should_seed_outdated_row_accepts_exact_pins() {
        // An exact pin makes version the deciding fact. `action_for` ignores `outdated`
        // entirely, and the row seeds via `is_pinned_row` instead, so this path is
        // irrelevant. Returning `true` here is harmless.
        assert!(should_seed_outdated_row(Some("2.50.1"), true));
        assert!(should_seed_outdated_row(Some("1.0-beta"), true));
    }

    #[test]
    fn should_seed_outdated_row_false_when_not_outdated() {
        // If the row is NOT outdated, the pin is irrelevant — never seed.
        assert!(!should_seed_outdated_row(Some("pending"), false));
        assert!(!should_seed_outdated_row(Some("latest"), false));
        assert!(!should_seed_outdated_row(Some("2.50.1"), false));
        assert!(!should_seed_outdated_row(None, false));
    }

    #[test]
    fn a_pending_outdated_row_does_not_seed() {
        // A row marked `pending` has no reachable upgrade (action_for returns None),
        // so being outdated does NOT make it worth re-probing. This prevents spending
        // serial processes on rows that will do nothing. Measured: 4 rows here (AWS CLI,
        // Obsidian, uv, VS Code), up to 24 as the estate drifts.
        //
        // ⭐ The seventh of a family: every guard written when "a pin is a number" or
        // "outdated ⇒ upgrade" was always true has silently inherited the keyword pins.
        let last_seen = vec![Some(seen(true, "1.0"))];
        // With `latest` (or no pin), outdated DOES seed.
        assert_eq!(
            seeds_for_rescan(&[0], &[], &last_seen, &|i| i == 0, &|_| false),
            vec![0]
        );
        // But the same row, when the pin is `pending`, does NOT seed despite being outdated.
        // The test closure here models the production `is_outdated_row` that now checks the pin.
        let is_outdated_with_pending = |i: usize| {
            if i == 0 {
                should_seed_outdated_row(Some("pending"), true)
            } else {
                false
            }
        };
        assert!(
            seeds_for_rescan(&[0], &[], &last_seen, &is_outdated_with_pending, &|_| false)
                .is_empty()
        );
    }

    #[test]
    fn a_never_observed_row_is_a_seed_unknown_is_not_satisfied() {
        // `last_seen == None` means nobody looked. "We don't know" and "nothing to
        // do" are different claims — probe it.
        let last_seen = vec![None, None];
        assert_eq!(
            seeds_for_rescan(&[0], &[1], &last_seen, &|_| false, &|_| false),
            vec![0, 1]
        );
        // Also when the vector is simply SHORTER than the plan (a row past its end
        // has never been observed either).
        assert_eq!(
            seeds_for_rescan(&[5], &[], &[], &|_| false, &|_| false),
            vec![5]
        );
    }

    #[test]
    fn an_unknown_presence_is_a_seed_too() {
        // Probed once but indeterminate (`present: None` — e.g. no practicable route
        // on this OS). Not a match with any desire, so it stays in.
        let last_seen = vec![Some(crate::detect::Presence::default())];
        assert_eq!(
            seeds_for_rescan(&[0], &[], &last_seen, &|_| false, &|_| false),
            vec![0]
        );
    }

    #[test]
    fn requirements_of_a_surviving_seed_come_along() {
        // Integration with rescan_scope: only row 1 has an action, but it requires
        // row 0, whose presence feeds will_be_present / topo_sort. Row 2 is settled
        // and unrelated → stays out.
        let requires = vec![vec![], vec!["dep".to_string()], vec![]];
        let name_idx = crate::deps::index_of_names(["dep", "app", "other"].into_iter());
        let last_seen = vec![
            Some(seen(true, "1.0")),
            Some(seen(false, "")),
            Some(seen(true, "1.0")),
        ];
        let seeds = seeds_for_rescan(&[0, 1, 2], &[], &last_seen, &|_| false, &|_| false);
        assert_eq!(seeds, vec![1], "only the pending install seeds");
        let mut scope: Vec<usize> = crate::deps::rescan_scope(&requires, &name_idx, &seeds)
            .into_iter()
            .collect();
        scope.sort_unstable();
        assert_eq!(
            scope,
            vec![0, 1],
            "the requirement is pulled in, `other` is not"
        );
    }

    #[test]
    fn forbidden_choice_parses_the_three_wire_words() {
        use ForbiddenChoice::*;
        assert_eq!(forbidden_choice("forbidden-retry"), Some(Retry));
        assert_eq!(forbidden_choice("forbidden-continue"), Some(Continue));
        assert_eq!(forbidden_choice("forbidden-stop"), Some(Stop));
        // Anything else is NOT a decision — the pause keeps holding (`open-forbidden`
        // is served while we hold, and must not be read as an answer).
        assert_eq!(forbidden_choice("open-forbidden"), None);
        assert_eq!(forbidden_choice("apply"), None);
    }

    #[test]
    fn retry_is_offered_until_the_attempt_cap_then_only_continue_or_stop() {
        // Each retry is user-initiated so it cannot spin on its own, but an
        // unbounded loop lets a user hang the Apply forever against a firewall that
        // will not budge. The cap counts RUNS, so 3 means first attempt + 2 retries.
        assert!(retry_offered(1));
        assert!(retry_offered(MAX_FORBIDDEN_RUNS - 1));
        assert!(!retry_offered(MAX_FORBIDDEN_RUNS));
        assert!(!retry_offered(MAX_FORBIDDEN_RUNS + 1));
    }

    #[test]
    fn a_retry_past_the_cap_degrades_to_continue() {
        // Defence in depth: even if a stale front sends `forbidden-retry` after the
        // cap, the loop must not re-run the step — it moves on instead of hanging.
        use ForbiddenChoice::*;
        assert_eq!(honour_choice(Retry, 1), Retry);
        assert_eq!(honour_choice(Retry, MAX_FORBIDDEN_RUNS), Continue);
        assert_eq!(honour_choice(Stop, MAX_FORBIDDEN_RUNS), Stop);
    }

    #[test]
    fn outdated_trusts_the_greedy_scan() {
        // Approach B: presence in the scan IS the signal, for casks and
        // formulae alike (the scan uses --greedy-auto-updates, so a lagging
        // self-updating cask is listed and gets forced). None → not outdated.
        let cask = Outdated {
            current: "1.127.0".into(),
            available: "1.130.0".into(),
            is_cask: true,
        };
        let formula = Outdated {
            current: "1.7".into(),
            available: "1.8".into(),
            is_cask: false,
        };
        assert!(is_outdated_now(Some(&cask))); // cask listed by greedy → outdated
        assert!(is_outdated_now(Some(&formula))); // formula listed → outdated
        assert!(!is_outdated_now(None)); // absent from scan → not outdated
    }

    #[test]
    fn scope_refuses_only_rows_the_user_put_out_of_scope() {
        let mut sel = Selection::default();
        sel.scope.insert("Git".into(), "out".into());
        sel.scope.insert("Nushell".into(), "in".into());
        // Out of scope by the user's own hand → refused.
        assert!(super::scope_refuses(&sel, "Git"));
        // Explicitly pulled IN → allowed, even though it is external. The user
        // was warned at the click; the server does not second-guess them.
        assert!(!super::scope_refuses(&sel, "Nushell"));
        // Absent from the map → the front's derivation governs, and the front
        // already refused to offer the action. The server allows it: refusing
        // here would break every ordinary row.
        assert!(!super::scope_refuses(&sel, "Obsidian"));
    }

    /// The BATCH path must refuse on the same evidence as the row path, from disk —
    /// not only on the list the client chose to send. This asserts the predicate over
    /// a round-tripped file, which is what `apply_diff` actually reads: a client that
    /// omits `unmanaged` entirely still cannot get an action on an explicit `out`.
    #[test]
    fn scope_refuses_from_disk_so_omitting_the_wire_list_is_not_a_bypass() {
        let dir = std::env::temp_dir().join("talos-test-scope-batch-teeth");
        let _ = std::fs::remove_dir_all(&dir);
        let mut written = Selection::default();
        written.scope.insert("Git".into(), "out".into());
        write_selection(&dir, &written);
        // Exactly what apply_diff does — read the persisted overrides, then ask.
        let sel = read_selection(&dir);
        assert!(
            super::scope_refuses(&sel, "Git"),
            "explicit out survives the file"
        );
        assert!(
            !super::scope_refuses(&sel, "Nushell"),
            "an untouched row still acts"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_apply_takes_no_rung_and_filters_nothing_after_action_for() {
        // The ladder left on 2026-10-10: applying PART of a desire built states the engine
        // never reasons about (a fresh config on a stale app), and nobody ever moved the
        // slider. An old front still sends `rung`; it must be ignored, never parsed.
        let src = include_str!("server.rs").replace('\r', "");
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        assert!(
            !code.contains("rung_allows("),
            "a rung filter is back in the batch"
        );
        assert!(
            !code.contains("rung_from_wire("),
            "the apply arm parses a rung again"
        );
        assert!(
            !code.contains("crate::ladder"),
            "a caller still names the old module"
        );
        let body = fn_body("apply_diff");
        assert!(
            body.contains("action_for(desired, &facts)"),
            "apply_diff no longer asks the shared rule"
        );
    }

    #[test]
    fn scope_refuses_survives_a_garbage_value() {
        let mut sel = Selection::default();
        sel.scope.insert("Git".into(), "sideways".into());
        // parse_selection would have dropped this, but a direct construction
        // must not become a silent refusal either: only "out" refuses.
        assert!(!super::scope_refuses(&sel, "Git"));
    }

    #[test]
    fn cancel_targets_only_the_running_step() {
        // The cheap mistake with the expensive consequence: the user clicks Stop as
        // step 3 is already finishing, the message lands while step 4 runs, and we
        // kill the WRONG row. The index must match.
        assert!(super::cancel_targets(r#"{"type":"cancel-step","i":3}"#, 3));
        assert!(!super::cancel_targets(r#"{"type":"cancel-step","i":5}"#, 3));
    }

    #[test]
    fn cancel_targets_ignores_everything_else() {
        // Other verbs stream past this check constantly (sudo-pw, forbidden-*): none
        // of them may be read as a cancel.
        assert!(!super::cancel_targets(r#"{"type":"sudo-pw","pw":"x"}"#, 3));
        assert!(!super::cancel_targets(r#"{"type":"cancel-step"}"#, 3)); // no index
        assert!(!super::cancel_targets("not json at all", 3));
        assert!(!super::cancel_targets(
            r#"{"type":"cancel-step","i":"3"}"#,
            3
        )); // string, not number
    }

    // ---- the behaviour accumulator -------------------------------------------------
    //
    // `do_step` and `run_in_pty` need a live WebSocket, so the OBSERVATION SITE inside
    // do_step is not reachable from a unit test — what it passes (`saw_window`,
    // `forbidden`, `started.elapsed()`) is verified by reading, and Task 6 verifies it at
    // a real click. `remember_behaviour` and `flush_behaviour` take only `&AppState`
    // though, so the accumulator itself IS testable, and it holds the two properties this
    // task's design rests on: the in-Apply ratchet, and the clear that keeps one Apply's
    // facts out of the next one's write.

    /// An AppState with nothing in it but a share directory. Every field is plain data —
    /// no socket, no pty — so the accumulator can be exercised for real.
    fn state_with_share(share: &std::path::Path) -> super::AppState {
        super::AppState {
            plan: tokio::sync::RwLock::new(crate::bundles::Plan { steps: Vec::new() }),
            // Pointed at the share's own folder: nothing here reloads, and a bogus path
            // would only surface if something did.
            content: std::sync::Mutex::new(crate::content::Content {
                catalog: share.join("catalog"),
                bundles: share.join("bundles"),
                kit: share.to_path_buf(),
                source: crate::content::Source::Loose,
            }),
            profiles: crate::profiles::Profiles::default(),
            os: Os::Darwin,
            appmgmt: crate::platform::AppMgmtStatus::NotApplicable,
            disk_root: None,
            data_dir: share.join("local"),
            consent: crate::consent::ConsentStore {
                local_dir: share.join("local"),
                exe_dir: share.to_path_buf(),
                host: "test-host".into(),
                user: "test-user".into(),
            },
            sudo_pw: tokio::sync::Mutex::new(None),
            last_seen: tokio::sync::Mutex::new(Vec::new()),
            outdated_seen: tokio::sync::Mutex::new(None),
            observed: tokio::sync::Mutex::new(std::collections::BTreeMap::new()),
            // Empty, matching the empty plan above: these tests exercise the WRITE side,
            // and the resolved beliefs are exactly what must never travel that way.
            facts: Vec::new(),
            // Empty for the same reason: `timings` is the READ snapshot, and the write side
            // must never consult it — a flush that merged the boot snapshot back in would
            // re-write, on every Apply, durations this Apply never measured.
            timings: crate::timings::Timings::new(),
            timed: tokio::sync::Mutex::new(crate::timings::Timings::new()),
            notes: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            found_scope: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            machine: std::sync::Mutex::new(None),
            machine_started: std::sync::atomic::AtomicBool::new(false),
            machine_running: std::sync::atomic::AtomicBool::new(false),
            machine_shared: std::sync::atomic::AtomicBool::new(false),
            last_scan: std::sync::Mutex::new(None),
            unmoved: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            brew_pins: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            updated_this_session: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    fn share(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A tool whose own update runs at each Apply is done for the session after an `ok` of that
    /// update or of its install, and only then: a failure, a removal, or a row run only when
    /// behind leaves it armed.
    #[test]
    fn only_a_successful_update_or_install_makes_its_row_done_for_the_session() {
        let mut s = brew_step("claude-code", Some("self"));
        s.upgrade = Some("claude update".into());
        s.update_each_apply = true;
        assert!(super::arms_session_done(&s, Action::Upgrade, true));
        assert!(
            !super::arms_session_done(&s, Action::Upgrade, false),
            "failed: armed"
        );
        assert!(
            super::arms_session_done(&s, Action::Install, true),
            "installed: the manager put the current build there"
        );
        assert!(
            !super::arms_session_done(&s, Action::Uninstall, true),
            "a removal"
        );
        let mut behind_only = s.clone();
        behind_only.update_each_apply = false;
        assert!(!super::arms_session_done(
            &behind_only,
            Action::Upgrade,
            true
        ));
        let mut managed = s.clone();
        managed.pin = None;
        assert!(
            !super::arms_session_done(&managed, Action::Upgrade, true),
            "a manager upgrade"
        );
    }

    /// The plan tells the front which rows are done for the session, so the front's twin of
    /// `action_for` reads the fact the Apply reads; emptying the set (the Refresh) re-arms them.
    #[test]
    fn the_plan_says_which_rows_already_updated_in_this_session() {
        let dir = share("talos-test-plan-updated-this-session");
        std::fs::create_dir_all(dir.join("catalog")).unwrap();
        let state = state_with_share(&dir);
        let mut s = brew_step("claude-code", Some("self"));
        s.upgrade = Some("claude update".into());
        s.update_each_apply = true;
        let steps = vec![s.clone()];
        let m = super::plan_message(&state, &steps);
        assert_eq!(m["steps"][0]["updatedThisSession"], false, "{m}");
        state
            .updated_this_session
            .lock()
            .unwrap()
            .insert(s.id.clone());
        let m = super::plan_message(&state, &steps);
        assert_eq!(m["steps"][0]["updatedThisSession"], true, "{m}");
        assert!(super::updated_this_session(&state, &s));
        state.updated_this_session.lock().unwrap().clear();
        let m = super::plan_message(&state, &steps);
        assert_eq!(m["steps"][0]["updatedThisSession"], false, "re-armed: {m}");
    }

    /// The first message names the distribution found beside the catalog, re-read on
    /// every build of the message like the catalogue itself — and `null`, not absent and
    /// not `""`, when the content has no such file.
    #[test]
    fn the_plan_message_names_the_distribution_beside_the_catalog() {
        let dir = share("talos-test-plan-distribution");
        std::fs::create_dir_all(dir.join("catalog")).unwrap();
        let state = state_with_share(&dir);
        let m = super::plan_message(&state, &[]);
        assert!(m["distribution"].is_null(), "{m}");
        std::fs::write(dir.join("distribution.yaml"), "name: Ops\nversion: 3\n").unwrap();
        let m = super::plan_message(&state, &[]);
        assert_eq!(m["distribution"]["label"], "Ops 3", "{m}");
        assert_eq!(m["distribution"]["name"], "Ops", "{m}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Machine tab is told what the journal received about this machine, verbatim, and
    /// where a consented copy goes — `null` while the profile is still being collected.
    #[test]
    fn the_log_message_carries_the_machine_profile_and_the_last_scan() {
        let dir = share("talos-test-log-machine");
        let state = state_with_share(&dir);
        let m: serde_json::Value = serde_json::from_str(&super::log_msg(&state)).unwrap();
        assert!(m["machine"].is_null(), "{m}");
        assert!(m["scan"].is_null(), "{m}");
        let to = m["sharedTo"].as_str().unwrap();
        assert!(to.ends_with("test-user.jsonl"), "{to}");
        *state.machine.lock().unwrap() = Some(crate::machine::Profile {
            kind: "machine".into(),
            cores: 8,
            ..Default::default()
        });
        *state.last_scan.lock().unwrap() = Some(crate::machine::ScanRecord {
            kind: "scan".into(),
            total_ms: 4200,
            ..Default::default()
        });
        let m: serde_json::Value = serde_json::from_str(&super::log_msg(&state)).unwrap();
        assert_eq!(m["machine"]["cores"], 8, "{m}");
        assert_eq!(m["scan"]["total_ms"], 4200, "{m}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Turning sharing on mid-session sends this launch's profile to the share once; turning
    /// it on again, or off, sends nothing more.
    #[test]
    fn turning_sharing_on_copies_this_launchs_profile_to_the_share_once() {
        let dir = share("talos-test-consent-profile");
        let state = state_with_share(&dir);
        *state.machine.lock().unwrap() = Some(crate::machine::Profile {
            kind: "machine".into(),
            ..Default::default()
        });
        let shared = crate::consent::shared_log_path(
            &state.consent.exe_dir,
            &state.consent.host,
            &state.consent.user,
        );
        super::set_consent(&state, false);
        assert!(!shared.exists());
        super::set_consent(&state, true);
        super::set_consent(&state, true);
        super::set_consent(&state, false);
        super::set_consent(&state, true);
        let text = std::fs::read_to_string(&shared).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("\"kind\":\"machine\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A first launch (no `selection.json` yet) hands the front the author's default-on
    /// cards, needs included; the first saved selection replaces them, for good.
    #[test]
    fn the_plan_message_seeds_the_default_cards_on_a_first_launch_only() {
        let dir = share("talos-test-plan-default-cards");
        let mut state = state_with_share(&dir);
        state.profiles = crate::profiles::Profiles {
            columns: 2,
            items: vec![
                crate::profiles::parse_one_bundle("bundle: Base\n").unwrap(),
                crate::profiles::parse_one_bundle("bundle: Data\ndefault: on\nneeds: [Base]\n")
                    .unwrap(),
                crate::profiles::parse_one_bundle("bundle: Legacy\n").unwrap(),
            ],
        };
        let m = super::plan_message(&state, &[]);
        assert_eq!(
            m["selection"]["activeBundles"],
            json!(["Base", "Data"]),
            "{m}"
        );
        // Each card says whether it is a default one — what the front's Reset returns to.
        let on: Vec<bool> = (0..3)
            .map(|i| m["profiles"][i]["defaultOn"] == true)
            .collect();
        assert_eq!(on, vec![true, true, false], "{m}");
        let saved = crate::selection::Selection {
            active_bundles: vec!["Legacy".into()],
            ..Default::default()
        };
        crate::selection::write_selection(&state.data_dir, &saved);
        let m = super::plan_message(&state, &[]);
        assert_eq!(m["selection"]["activeBundles"], json!(["Legacy"]), "{m}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn two_observations_of_one_package_ratchet_within_the_same_apply() {
        // A row can run twice in ONE Apply: a 403 with Retry re-runs the same action in
        // place. The second run must not REPLACE the first's facts — a retry that
        // succeeds quickly would otherwise erase the 403 that made the user unblock the
        // firewall, which is the single most useful fact of the run.
        let dir = share("talos-test-observed-ratchet");
        let state = state_with_share(&dir);
        super::remember_behaviour(
            &state,
            "aws-cli",
            "brew",
            crate::behaviour::Facts {
                uac: false,
                forbidden: true,
                slow_secs: 120,
            },
        )
        .await;
        super::remember_behaviour(
            &state,
            "aws-cli",
            "brew",
            crate::behaviour::Facts {
                uac: true,
                forbidden: false,
                slow_secs: 7,
            },
        )
        .await;
        let obs = state.observed.lock().await;
        let f = obs["aws-cli"]["brew/darwin"];
        assert!(f.forbidden, "the first run's 403 survived the quick retry");
        assert!(f.uac, "and the second run's elevation was added");
        assert_eq!(f.slow_secs, 120, "the WORST duration is what is kept");
    }

    #[tokio::test]
    async fn the_key_is_the_route_and_os_of_the_running_machine() {
        // A behaviour belongs to the (route, os) couple, not the package. And a package
        // with no route on this platform must still land somewhere stable rather than
        // under an empty key.
        let dir = share("talos-test-observed-key");
        let state = state_with_share(&dir);
        // Real catalogue ids, since that is what the file is named after.
        super::remember_behaviour(
            &state,
            "visual-studio-code",
            "brew",
            crate::behaviour::Facts::default(),
        )
        .await;
        super::remember_behaviour(
            &state,
            "starship-config",
            "",
            crate::behaviour::Facts::default(),
        )
        .await;
        let obs = state.observed.lock().await;
        assert!(
            obs["visual-studio-code"].contains_key("brew/darwin"),
            "{obs:?}"
        );
        assert!(
            obs["starship-config"].contains_key("none/darwin"),
            "{obs:?}"
        );
    }

    #[tokio::test]
    async fn flush_writes_every_observed_package_then_empties_the_accumulator() {
        // The whole shape of this task in one test: accumulate during the Apply, write
        // ONCE at the end, and leave nothing behind. If the clear were forgotten, a
        // second Apply in the same session would re-write the first Apply's packages —
        // harmless to the values (the ratchet is idempotent) but a write to files it
        // never touched, which is exactly the collision window the design narrows.
        let dir = share("talos-test-observed-flush");
        let state = state_with_share(&dir);
        super::remember_behaviour(
            &state,
            "nushell",
            "brew",
            crate::behaviour::Facts {
                uac: false,
                forbidden: false,
                slow_secs: 41,
            },
        )
        .await;
        super::remember_behaviour(
            &state,
            "rclone",
            "brew",
            crate::behaviour::Facts {
                uac: false,
                forbidden: true,
                slow_secs: 3,
            },
        )
        .await;
        super::flush_behaviour(&state).await;

        assert_eq!(
            crate::behaviour_io::read_one(&dir, "nushell")["brew/darwin"].slow_secs,
            41
        );
        assert!(crate::behaviour_io::read_one(&dir, "rclone")["brew/darwin"].forbidden);
        assert!(
            state.observed.lock().await.is_empty(),
            "the accumulator must be empty after the flush, or the next Apply rewrites this one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn flushing_nothing_creates_no_folder_at_all() {
        // An Apply that ran no step (or a `done/nothing` one) reaches the flush too. It
        // must not conjure a `behaviour/` folder on the share — an empty directory
        // appearing next to the exe is a thing someone has to explain.
        //
        // ⚠️ This does NOT pin flush's `is_empty()` early return: measured by removing it,
        // the whole suite stays green, because the loop over an empty map writes nothing
        // regardless. What it pins is the OUTCOME — that `write_one`'s create_dir_all is
        // never reached on an empty flush, which would change the day someone made the
        // folder eagerly. The guard itself only silences a "0 package(s)" log line.
        let dir = share("talos-test-observed-empty");
        std::fs::create_dir_all(&dir).unwrap();
        let state = state_with_share(&dir);
        super::flush_behaviour(&state).await;
        assert!(
            !dir.join("behaviour").exists(),
            "an empty flush wrote something"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_second_apply_ratchets_onto_what_the_first_one_wrote() {
        // Across two Applies the file is the only memory, so the merge has to happen
        // against the DISK. Asserted end to end through flush, because the in-memory
        // ratchet passing proves nothing about the read-modify-write behind it.
        let dir = share("talos-test-observed-two-applies");
        let state = state_with_share(&dir);
        super::remember_behaviour(
            &state,
            "uv",
            "brew",
            crate::behaviour::Facts {
                uac: false,
                forbidden: true,
                slow_secs: 300,
            },
        )
        .await;
        super::flush_behaviour(&state).await;
        // Second Apply, same machine: a quick clean run. Nothing may be un-learned.
        super::remember_behaviour(
            &state,
            "uv",
            "brew",
            crate::behaviour::Facts {
                uac: false,
                forbidden: false,
                slow_secs: 9,
            },
        )
        .await;
        super::flush_behaviour(&state).await;
        let f = crate::behaviour_io::read_one(&dir, "uv")["brew/darwin"];
        assert!(f.forbidden, "the first Apply's 403 is still on the share");
        assert_eq!(f.slow_secs, 300, "and so is the worst duration");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_written_file_names_no_host_no_user_no_time() {
        // ⭐ The subject-less promise, asserted on the BYTES. `state_with_share` puts a
        // recognisable host and user in the consent store — the same struct the write
        // path is handed — so if either ever leaked into this file, this fails.
        //
        // The date check looks for the ISO SEPARATORS an rfc3339 stamp cannot avoid
        // (`append_history` writes `2026-08-01T21:17:32...`) rather than for a year: a bare
        // `202` would false-fail the day a legitimate `slow_secs: 202` or `2024` was
        // measured, which would make this test a trap rather than a guard.
        let dir = share("talos-test-observed-subjectless");
        let state = state_with_share(&dir);
        super::remember_behaviour(
            &state,
            "git",
            "brew",
            crate::behaviour::Facts {
                uac: true,
                forbidden: true,
                slow_secs: 12,
            },
        )
        .await;
        super::flush_behaviour(&state).await;
        let raw =
            std::fs::read_to_string(crate::behaviour_io::behaviour_path(&dir, "git")).unwrap();
        assert!(!raw.contains("test-host"), "a host leaked: {raw}");
        assert!(!raw.contains("test-user"), "a user leaked: {raw}");
        let date = regex::Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
        assert!(!date.is_match(&raw), "a date leaked: {raw}");
        assert!(
            raw.contains("403"),
            "and the facts themselves are there: {raw}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the LOCAL duration accumulator -------------------------------------------------
    //
    // Same shape as the four above, one file over: `remember_duration` + the second half of
    // `flush_behaviour`. Pinned separately because the SEMANTICS are inverted — assignment,
    // not a ratchet — and because they write to a different folder (`data_dir`, local) than
    // everything above (`consent.exe_dir`, shared). `state_with_share` puts the local dir
    // INSIDE the share dir, so one `remove_dir_all` still cleans up.

    #[tokio::test]
    async fn a_second_run_in_the_same_apply_replaces_the_duration_it_does_not_ratchet() {
        // ⭐ THE property, at the accumulator. A 403 + Retry runs the same step twice in ONE
        // Apply: the share must keep the WORST of the two (asserted above), and this file
        // must keep the LAST. If someone reached for `merge_into` here by reflex, both would
        // keep 240 and one unlucky cold run would be this machine's estimate forever.
        let dir = share("talos-test-timed-assign");
        let state = state_with_share(&dir);
        super::remember_duration(&state, "aws-cli", "brew", 240).await;
        super::remember_duration(&state, "aws-cli", "brew", 12).await;
        assert_eq!(
            state.timed.lock().await["aws-cli"]["brew/darwin"],
            12,
            "the LAST attempt's duration, not the worst"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn flush_writes_the_local_durations_then_empties_the_accumulator() {
        // The local half of the one flush: into `data_dir`, NOT the share. Asserted on the
        // path as well as the value, because writing this to the shared folder would publish
        // one machine's warm cache as if it were the fleet's classification.
        let dir = share("talos-test-timed-flush");
        let state = state_with_share(&dir);
        super::remember_duration(&state, "nushell", "brew", 41).await;
        super::remember_duration(&state, "rclone", "brew", 3).await;
        super::flush_behaviour(&state).await;

        let back = crate::timings::read_timings(&state.data_dir);
        assert_eq!(back["nushell"]["brew/darwin"], 41);
        assert_eq!(back["rclone"]["brew/darwin"], 3);
        assert!(
            !crate::timings::timings_path(&dir).exists(),
            "timings.yaml must be LOCAL; it landed in the shared folder"
        );
        assert!(
            state.timed.lock().await.is_empty(),
            "the accumulator must be empty after the flush, like the shared one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_second_apply_lowers_a_duration_and_keeps_the_packages_it_did_not_touch() {
        // ⭐ THE property, end to end through the DISK — which is where it actually matters,
        // and where the in-memory test above proves nothing: `merge_and_write_timings` does a
        // read-modify-write, so a `max` could hide there and stay green above.
        let dir = share("talos-test-timed-two-applies");
        let state = state_with_share(&dir);
        super::remember_duration(&state, "uv", "brew", 300).await;
        super::remember_duration(&state, "jq", "brew", 3).await;
        super::flush_behaviour(&state).await;
        // Second Apply: uv again, warm this time. jq untouched.
        super::remember_duration(&state, "uv", "brew", 9).await;
        super::flush_behaviour(&state).await;

        let back = crate::timings::read_timings(&state.data_dir);
        assert_eq!(
            back["uv"]["brew/darwin"], 9,
            "the faster run LOWERED the estimate — the opposite of the share's ratchet"
        );
        assert_eq!(
            back["jq"]["brew/darwin"], 3,
            "and a package this Apply never touched survived the overwrite"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_local_flush_does_not_depend_on_the_shared_one_having_anything() {
        // The two halves of `flush_behaviour` are sequential, not nested. Written because the
        // obvious wiring — appending the local flush inside the shared one's body — puts it
        // behind `observed.is_empty()`'s early return, and every measurement would vanish
        // for any caller that recorded a duration without recording a fact. They happen to
        // be filled together in `do_step` today; nothing enforces that.
        let dir = share("talos-test-timed-independent");
        let state = state_with_share(&dir);
        super::remember_duration(&state, "jq", "brew", 7).await;
        super::flush_behaviour(&state).await;
        assert_eq!(
            crate::timings::read_timings(&state.data_dir)["jq"]["brew/darwin"],
            7,
            "a duration with no accompanying fact still reached the disk"
        );
        assert!(
            !dir.join("behaviour").exists(),
            "and nothing was written to the share"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- requirements as TEETH, not merely as a label -----------------------------
    //
    // Text-level, like `handled_client_msgs` above and for the same reason: both gates
    // live in functions that need a live WebSocket, so no unit test can reach them. What
    // needs pinning is nonetheless a literal — whether the skip consults the verdict at
    // all — and the verdict's own logic is covered properly in deps.rs.
    //
    // ⚠️ These guard against the exact failure mode the whole change addresses: a
    // requirement that is COMPUTED and then only shown. `requires_reason` was already
    // computed at server.rs:1604 before this work and fed nothing but the row's text,
    // which is why a Microsoft redistributable kept an install button on a Mac.

    #[test]
    fn a_brew_pin_is_asked_at_plan_time_and_again_at_the_step() {
        // The brew tests above are pure: removing the wiring would leave them green.
        let plan = fn_body("apply_diff");
        let at = |b: &str, s: &str| b.find(s).unwrap_or_else(|| panic!("no `{s}`"));
        let asked = at(&plan, "ask_brew(a, step, os)");
        assert!(asked < at(&plan, "brew_plan(a, step, verdict.as_ref())"));
        let held = at(&plan, "send_held(socket, i, &why)");
        assert!(asked < held && held < at(&plan, "visual_plan.push((i, a))"));
        assert!(
            at(&plan, "drop_requiring_held(") < at(&plan, "topo_sort("),
            "what requires a held row leaves the plan before it is ordered"
        );
        // Second review: the plan's answer is kept even when it is "unknown", or the step asks
        // brew again and may hold a row whose dependents are already ordered after it; and no
        // answer outlives its Apply, or a later retry acts on it.
        assert!(plan.contains(".insert(step.id.clone(), verdict);"));
        assert!(plan.contains("brew_pins.lock().await.remove(&steps[i].id)"));
        let cleared = plan.rfind("brew_pins.lock().await.clear()").unwrap_or(0);
        assert!(
            cleared > at(&plan, "flush_behaviour(state)"),
            "cleared when the Apply ends"
        );
        let step = fn_body("do_step");
        let rebuilt = at(&step, "brew_plan(action, step, verdict.as_ref())");
        assert!(at(&step, "winget_in_place(") < rebuilt);
        assert!(rebuilt < at(&step, "run_in_pty("));
    }

    /// The body of a named async fn in this source file, up to the next top-level item,
    /// with every line comment STRIPPED.
    ///
    /// ⚠️ The stripping is not tidiness, it is the whole validity of the technique. The
    /// first draft of these guards passed the moment a COMMENT elsewhere in `apply_diff`
    /// mentioned `requires_blocked` — a green light for code that did not exist. A
    /// text-level guard that reads comments proves nothing about behaviour, and this repo
    /// has been burned by exactly that before (an exposure audit that counted comments as
    /// assertions). Read code, never prose.
    fn fn_body(name: &str) -> String {
        let src = include_str!("server.rs");
        let start = src
            .find(&format!("async fn {name}("))
            .unwrap_or_else(|| panic!("`{name}` moved or was renamed — re-point this test"));
        let end = src[start..]
            .find("\n}\n")
            .expect("a top-level fn body ends at a column-0 brace");
        src[start..start + end]
            .lines()
            .map(|l| match l.find("//") {
                Some(at) => &l[..at],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The three wiring sites of "done for the session", each one a revert that no behaviour
    /// test can see without a socket: the step arms (and a removal forgets), the Apply reads the
    /// fact into `action_for`, and the Refresh button — ONLY it — empties the set.
    #[test]
    fn a_session_done_row_is_armed_by_its_step_read_by_the_apply_and_rearmed_only_by_refresh() {
        let step = fn_body("do_step");
        assert!(
            step.contains("arms_session_done("),
            "do_step no longer arms the row"
        );
        assert!(
            step.contains(".insert(step.id.clone())"),
            "do_step no longer records it"
        );
        assert!(
            step.contains(".remove(&step.id)"),
            "a removal no longer forgets it"
        );
        assert!(
            step.contains(r#""done": false"#),
            "a removal forgets it without telling the front"
        );
        let apply = fn_body("apply_diff");
        assert!(
            apply.contains("updated_this_session: updated_this_session(state, step)"),
            "the Apply no longer hands the fact to action_for"
        );
        let squash = |t: &str| t.split_whitespace().collect::<String>();
        let clear = ".updated_this_session.lock().unwrap_or_else(|e|e.into_inner()).clear()";
        let socket = squash(&fn_body("handle_socket"));
        let refresh = &socket[socket.find("\"rescan\"=>").expect("the rescan arm")..];
        let cleared = refresh.find(clear).expect("the Refresh no longer re-arms");
        let planned = refresh
            .find("plan_message(")
            .expect("the Refresh sends a plan");
        assert!(
            cleared < planned,
            "the set is emptied after the plan is built: the front would keep rows done"
        );
        let src = include_str!("server.rs");
        let code = &src[..src
            .find("#[cfg(test)]\nmod tests")
            .expect("the test module")];
        assert_eq!(
            squash(code).matches(clear).count(),
            1,
            "something other than the Refresh re-arms the rows"
        );
    }

    #[test]
    fn the_source_guards_read_code_and_not_comments() {
        // The guard ON the guards. `fn_body` is only meaningful if a mention in prose
        // cannot satisfy it, and that property is invisible until it fails — it failed
        // here first, silently, which is why it is now pinned.
        let body = fn_body("apply_diff");
        assert!(
            !body.contains("the two facts `will_be_present` folds away"),
            "fn_body still returns comment text → every guard built on it can be \
             satisfied by prose"
        );
        assert!(
            body.contains("let os = state.os;"),
            "fn_body stripped too much — it must still return the actual statements"
        );
    }

    #[test]
    fn the_batch_apply_refuses_a_requirement_blocked_row() {
        // A row Apply can never satisfy must not be in the plan. Without this the front
        // is the only guard, and a front-only guard is cosmetic: the SERVER builds the
        // plan (the Git-hazard lesson, talos-scope-second-axis).
        let body = fn_body("apply_diff");
        assert!(
            body.contains("requires_blocked"),
            "apply_diff computes the dependency graph and never asks whether a row is \
             possible → a row whose requirement cannot hold is still offered for install"
        );
    }

    #[test]
    fn a_retry_refuses_a_requirement_blocked_row_too() {
        // A retry never sees the wire lists, so it needs its own teeth — exactly the
        // asymmetry `scope_refuses` was added to close, now closed on the second
        // derivation as well.
        let body = fn_body("retry_attempt");
        assert!(
            body.contains("requirement_blocks_row"),
            "retry_attempt has teeth for the user's explicit scope override but not for an \
             impossible requirement → the button stays live on a row the panel greyed out"
        );
        // And the helper must DELEGATE, not re-derive: a second copy of the rule is how
        // the row path and the batch path came to disagree in the first place.
        assert!(
            fn_body("requirement_blocks_row").contains("requires_blocked"),
            "the row gate re-implements the requirement rule instead of calling it → two \
             behaviours for one question, which is the defect `scope_refuses` exists for"
        );
    }

    // ---- winget: a version change lands where the package must live ----------------------
    //
    // `winget_plan` is the decision, pure: what the probes saw goes in, the command and the
    // row's line come out. None = the stored command stands.

    use crate::managers::InstallScope;

    fn winget_step(pin: Option<&str>, scope: Option<InstallScope>) -> Step {
        let mgr = crate::managers::native_manager(Os::Windows).unwrap();
        let id = "Nushell.Nushell";
        let mut s = brew_step(id, pin);
        s.route = Some("winget".into());
        s.scope = scope;
        s.install = Some(match pin {
            Some(p) => mgr.install_pinned_in(id, p, scope),
            None => mgr.install_in(id, scope),
        });
        s.uninstall = Some(mgr.uninstall(id));
        s.upgrade = Some(match (pin, scope) {
            (Some(p), _) => mgr.install_pinned_in(id, p, scope),
            (None, Some(_)) => mgr.install_in(id, scope),
            (None, None) => mgr.upgrade(id, false),
        });
        s.downgrade = pin.map(|p| mgr.downgrade_in(id, p, scope));
        s
    }

    use crate::decision::Action;
    use crate::managers::InstallScope::{Machine, User};

    #[test]
    fn a_declared_scope_upgrades_in_place_only_when_the_copy_is_already_there() {
        // MEASURED (Windows VM, 2026-10-21): `winget install --scope user` over a machine-wide
        // 0.113.1 installed 0.116.1 per-user BESIDE it — two nu. So the in-place install runs
        // only when the copy already lives in the declared scope; otherwise the copy is removed
        // first, like a downgrade, and the new version lands where it was declared.
        let step = winget_step(None, Some(User));
        let listed = Some(("Nushell", "0.113.1"));
        assert_eq!(
            super::winget_plan(Action::Upgrade, &step, listed, Some(User), None),
            None
        );
        for found in [Some(Machine), None] {
            let (cmd, note) =
                super::winget_plan(Action::Upgrade, &step, listed, found, None).expect("rebuilt");
            assert!(
                cmd.starts_with("winget uninstall --id Nushell.Nushell"),
                "{cmd}"
            );
            assert!(cmd.ends_with(step.upgrade.as_deref().unwrap()), "{cmd}");
            assert!(note.contains("--scope user"), "{note}");
        }
        // No declared scope: `winget upgrade` keeps doing what it did.
        let plain = winget_step(None, None);
        assert_eq!(
            super::winget_plan(Action::Upgrade, &plain, listed, Some(Machine), None),
            None
        );
    }

    #[test]
    fn a_retried_version_change_is_judged_like_the_batch() {
        // The review's finding 4: the retry passed `Aim::default()` — no `before`, no
        // announcement — so off a pin its exit 0 lifted the hold and journalled a success that
        // moved nothing, while its comment claimed "no verdict". `do_step` needs a live socket
        // and a pty, so the wiring is pinned on the source, as `requirement_blocks_row` is.
        let body = fn_body("retry_attempt");
        assert!(
            !body.contains("Aim::default()"),
            "the retry gives no verdict again"
        );
        assert!(
            body.contains("before: before.as_ref()"),
            "no fresh probe to judge from"
        );
        assert!(
            body.contains("announced: od"),
            "no announcement to judge toward"
        );
    }

    #[test]
    fn a_tool_updated_on_every_apply_is_never_held_by_a_remembered_change() {
        // A hold recorded before the author added `update-on: apply` survives a Refresh (the
        // memory is keyed by id). The Apply must still run the row: it is never judged by the
        // version it leaves, so it is never held by it either.
        let mut memory = std::collections::HashMap::new();
        memory.insert(
            "copilot-cli".to_string(),
            Unmoved {
                found: "1.0.102".into(),
                target: "1.0.103".into(),
                reason: "it moved nothing".into(),
                // `change_command`: the upgrade, then the downgrade (none here).
                cmd: "copilot update\n".into(),
                ahead: false,
            },
        );
        let mut step = brew_step("copilot-cli", Some("self"));
        step.upgrade = Some("copilot update".into());
        let hold = |s: &Step| {
            hold_for(
                &memory,
                s,
                Action::Upgrade,
                Some("1.0.102"),
                Some("1.0.103"),
            )
        };
        assert!(hold(&step).is_some(), "behind-only: held as before");
        step.update_each_apply = true;
        assert!(hold(&step).is_none(), "updated on every Apply: never held");
    }

    #[test]
    fn a_scan_repeats_a_hold_only_for_a_row_that_would_still_act() {
        // The scan re-sends a remembered hold so the front keeps the row greyed. A row the
        // manager no longer lists as behind would do nothing: re-sending the hold there made
        // the front fabricate an outdated row the server would never touch.
        let od = crate::managers::Outdated {
            current: "2.50.1".into(),
            available: String::new(),
            is_cask: false,
        };
        let plain = brew_step("git", None);
        assert!(hold_still_in_view(&plain, Some(&od)));
        assert!(!hold_still_in_view(&plain, None));
        // An exact pin owns its target: the pin is in view as long as it is declared.
        assert!(hold_still_in_view(
            &brew_step("nushell", Some("0.113.1")),
            None
        ));
    }

    #[test]
    fn a_tool_updated_on_every_apply_is_looked_at_first_and_never_judged_unchanged() {
        let mut each = brew_step("claude-code", Some("self"));
        each.upgrade = Some("claude update".into());
        each.update_each_apply = true;
        // Acted on at every Apply, so re-observed before, like an exact pin.
        assert!(always_probed(&each));
        assert!(always_probed(&brew_step("nushell", Some("0.113.1"))));
        assert!(!always_probed(&brew_step("nushell", Some("latest"))));
        let mut behind = each.clone();
        behind.update_each_apply = false;
        assert!(!always_probed(&behind));
        // "Nothing to do" is the usual answer, and a success: no `unchanged` verdict.
        assert!(!judges_version(&each, Action::Upgrade));
        assert!(judges_version(&behind, Action::Upgrade));
        assert!(judges_version(
            &brew_step("nushell", Some("0.113.1")),
            Action::Downgrade
        ));
    }

    #[test]
    fn a_self_updating_row_runs_its_own_update_and_nothing_the_manager_rebuilds() {
        // Two sites rebuild a row's upgrade just before acting: the forced cask command, and
        // a winget upgrade moved to its declared scope. Either would replace `copilot update`
        // with the very manager upgrade `self` exists to keep away from the tool's copy.
        let mut cask = brew_step("visual-studio-code", Some("self"));
        cask.upgrade = Some("code --update".into());
        assert_eq!(forced_cask_upgrade_cmd(&cask, Os::Darwin, true), None);

        let mut step = winget_step(None, Some(User));
        step.pin = Some("self".into());
        step.upgrade = Some("copilot update".into());
        let listed = Some(("GitHub Copilot", "1.0.5"));
        for found in [Some(Machine), Some(User), None] {
            assert_eq!(
                super::winget_plan(Action::Upgrade, &step, listed, found, None),
                None,
                "found {found:?}"
            );
        }
    }

    #[test]
    fn a_downgrade_reinstalls_where_declared_else_where_found() {
        let listed = Some(("Nushell", "0.116.1"));
        let declared = winget_step(Some("0.113.1"), Some(User));
        let (cmd, _) =
            super::winget_plan(Action::Downgrade, &declared, listed, Some(Machine), None).unwrap();
        assert!(
            cmd.ends_with("--scope user"),
            "the catalogue wins over the copy: {cmd}"
        );
        let free = winget_step(Some("0.113.1"), None);
        let (cmd, _) =
            super::winget_plan(Action::Downgrade, &free, listed, Some(Machine), None).unwrap();
        assert!(
            cmd.starts_with("winget uninstall") && cmd.ends_with("--scope machine"),
            "{cmd}"
        );
        let (cmd, note) = super::winget_plan(Action::Downgrade, &free, listed, None, None).unwrap();
        assert!(!cmd.contains("--scope"), "{cmd}");
        assert!(note.contains("could not be read"), "{note}");
    }

    #[test]
    fn winget_not_found_is_success_only_for_a_removal() {
        // MEASURED: `winget install --id <no such id>` exits -1978335212 (no package found);
        // read as benign for every action, it journalled `ok:true` for an install that never
        // happened. Gone is what an uninstall wants; it is what an install or upgrade failed at.
        const NOT_FOUND: i32 = -1978335212;
        assert!(benign_code(Action::Uninstall, NOT_FOUND));
        for a in [Action::Install, Action::Upgrade, Action::Downgrade] {
            assert!(!benign_code(a, NOT_FOUND), "{a:?}");
        }
        // "No applicable upgrade": already current, whatever asked.
        for a in [
            Action::Install,
            Action::Upgrade,
            Action::Downgrade,
            Action::Uninstall,
        ] {
            assert!(benign_code(a, -1978335189), "{a:?}");
            assert!(!benign_code(a, 1), "{a:?}");
        }
    }

    #[test]
    fn a_downgrade_on_a_package_already_gone_only_installs() {
        // MEASURED: `winget uninstall` on an absent package exits -1978335212, which
        // `benign_code` accepts — so the chain stopped there and the step read ok with
        // nothing reinstalled (a 403 Retry of the reinstall half). Gone → install, nothing else.
        let free = winget_step(Some("0.113.1"), None);
        let (cmd, note) =
            super::winget_plan(Action::Downgrade, &free, None, None, Some(Machine)).unwrap();
        assert!(
            cmd.starts_with("winget install") && !cmd.contains("uninstall"),
            "{cmd}"
        );
        assert!(
            cmd.ends_with("--scope machine"),
            "the scope found before it went: {cmd}"
        );
        assert!(note.contains("not installed"), "{note}");
    }

    #[test]
    fn an_install_after_a_failed_downgrade_keeps_the_scope_it_found() {
        // A retry that finds the package gone turns into Install (`retry_action`); the scope
        // the downgrade's probe found minutes earlier must survive it.
        let free = winget_step(Some("0.113.1"), None);
        let (cmd, _) =
            super::winget_plan(Action::Install, &free, None, None, Some(Machine)).unwrap();
        assert!(cmd.ends_with("--version 0.113.1 --source winget --accept-source-agreements --accept-package-agreements --scope machine"), "{cmd}");
        assert_eq!(
            super::winget_plan(Action::Install, &free, None, None, None),
            None
        );
        // Declared: the stored install already carries it.
        let declared = winget_step(Some("0.113.1"), Some(User));
        assert_eq!(
            super::winget_plan(Action::Install, &declared, None, None, Some(Machine)),
            None
        );
        // Off winget nothing changes.
        assert_eq!(
            super::winget_plan(
                Action::Downgrade,
                &brew_step("nushell", Some("1")),
                None,
                None,
                None
            ),
            None
        );
    }

    #[test]
    fn what_a_probe_learnt_about_a_row_does_not_outlive_the_plan_it_was_read_in() {
        // Review 2 of beta.49: `found_scope` was keyed by row INDEX, and a Refresh that adds
        // a package shifts every index — another package then inherited Nushell's scope and
        // a false note. Keyed by the package's id, and the queued notes go with the plan.
        let flat: String = include_str!("server.rs").split_whitespace().collect();
        assert!(
            flat.contains("found_scope:tokio::sync::Mutex<std::collections::HashMap<String,"),
            "found_scope is keyed by something a Refresh renumbers"
        );
        let refresh = fn_body("handle_socket");
        let at = refresh
            .find("seen.clear()")
            .expect("the Refresh's own reset moved");
        assert!(
            refresh[at..].contains("notes.lock().await.clear()"),
            "a queued note survives a Refresh"
        );
        // And a [repair] line is queued only for a row that has an install to run.
        let apply = fn_body("apply_diff");
        let q = apply
            .find("queue_note(state, d")
            .expect("the repair note moved");
        assert!(
            apply[..q].contains("steps[d].install.is_some()"),
            "a repair note can leak to a later step"
        );
    }

    #[test]
    fn a_row_note_is_written_once_the_command_can_no_longer_clear_it() {
        // Review of beta.49: a line sent before the command was wiped as it was written — the
        // `[downgrade]` note and the dependents pass's `[repair]` line alike. Twice over: the
        // front resets the row's terminal on the RUNNING status, and on Windows ConPTY clears
        // the screen as the command starts (MEASURED in the VM: the note sent after the status
        // was gone, the terminal began at winget's first line). So notes are queued per row
        // and do_step writes them when the command has ENDED, above its exit line.
        let body = fn_body("do_step");
        let at = |s: &str| {
            body.find(s)
                .unwrap_or_else(|| panic!("`{s}` missing from do_step"))
        };
        assert!(
            at(r#""status": running"#) < at("take_notes("),
            "a note before the status is wiped"
        );
        assert!(
            at("run_in_pty(") < at("take_notes("),
            "a note before the command is cleared by ConPTY"
        );
        assert!(
            body.contains("winget_in_place("),
            "do_step runs the stored command as is"
        );
        let repair = fn_body("apply_diff");
        let pass = repair
            .find("dependents_to_recheck(")
            .expect("the dependents pass moved");
        assert!(
            repair[pass..].contains("queue_note("),
            "the [repair] line is sent, then wiped"
        );
        assert!(
            !repair[pass..].contains(r#""type": "out""#),
            "the [repair] line is sent directly"
        );
    }

    #[test]
    fn every_ending_of_a_retry_unfreezes_the_front() {
        // The front sets `applyRunning` when it sends `retry-step` and only `done` clears it.
        // So `done` is sent ONCE, by the wrapper, after whatever the attempt did: an early
        // return inside the attempt (an unknown row or verb, a missing App Management
        // permission) can no longer leave the window frozen.
        let wrapper = fn_body("retry_step");
        assert!(
            wrapper.contains("retry_attempt(") && wrapper.contains(r#""type": "done""#),
            "retry_step must run the attempt and THEN send done, whatever it returned"
        );
        assert!(
            !fn_body("retry_attempt").contains(r#""type": "done""#),
            "a `done` inside the attempt means some other return path can still skip it"
        );
    }

    #[test]
    fn the_batch_gate_sits_where_the_other_two_scope_gates_sit() {
        // ONE skip, three sources, one place. A second `continue` further down would be
        // the drift that made the row path and the batch path disagree once already.
        let body = fn_body("apply_diff");
        let gate = body
            .find("if out_of_scope.contains(&i)")
            .expect("the scope gate moved — re-point this test");
        let end = body[gate..].find("{\n").expect("the gate opens a block");
        assert!(
            body[gate..gate + end].contains("requires_blocked"),
            "the requirement gate must join the existing scope condition, not add a \
             second skip elsewhere in the loop"
        );
    }
}
