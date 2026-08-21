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
    append_history, clear_history, read_consent, read_history, write_consent, ConsentStore,
    HistEntry,
};
// ⚠️ The two scans call `detect_present_detailed_with_vscode` by its full path rather than
// importing it: the name is long enough that an import would read as an alias, and the full path
// says at the call site which of the three arities is being used.
use crate::detect::detect_present_detailed;
use crate::outdated::{outdated_for, scan_outdated};
use crate::platform::{current_os, local_data_dir, Os};
use crate::profiles::{load_profiles, Profiles};
use crate::selection::{read_selection, write_selection, Selection};

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

/// True when this row is OUT OF SCOPE by the user's explicit hand, so no action may
/// run on it. Reads the persisted overrides only: the DERIVED half (!canUninstall /
/// external) is the front's to compute and it already declines to offer the action,
/// so refusing on it here would break every ordinary row (the map is sparse — an
/// absent entry means "follow the derivation", not "out").
///
/// WHY the server checks at all, when the front already filters: `row_action` is a
/// SEPARATE path that never sees the `on`/`off` lists, and it is the path the
/// per-row button uses. Git declares `brew: git`, so `canUninstall` is true and the
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
    // Keywords (pending, latest) do not own a direction, so fall through.
    if !is_cask || matches!(classify_pin(step.pin.as_deref()), Some(PinKind::Exact(_))) {
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
    /// Where the plan came from, kept so a Refresh can read the same two folders again.
    /// Resolved once at startup (beside the exe, or the cwd in dev) — the LOCATION is
    /// not re-derived, only its contents are re-read.
    catalog_dir: PathBuf,
    bundles_dir: PathBuf,
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
    /// LAST OBSERVED presence per package, written by every scan (connect, Refresh,
    /// Apply re-scan), parallel to `plan.steps`. `None` = never observed.
    ///
    /// This is NOT a cache that spares a probe (a TTL cache was rejected — memory
    /// repaint-at-apply). It exists because the Apply re-scan is SCOPED to the diff:
    /// the rows it doesn't touch still need a presence to feed `will_be_present`, or
    /// every `requires` reason over them would lie. Remembering the last OBSERVATION
    /// is honest; inventing `false` is not.
    last_seen: tokio::sync::Mutex<Vec<Option<crate::detect::Presence>>>,
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
    // Build stamp at the top of the log — "which binary is really running?" (jj log order).
    println!("--- start: {}", crate::build_info::start_line());
    println!(
        "--- appmgmt: {}",
        crate::platform::app_management_status(os).as_str()
    );
    // The "next to the exe" folder (outside the .app if packaged): that's WHERE
    // bundles/ AND the consented shared copy live (hermetic boundary — mirror of
    // the TS compiled BUNDLES_DIR). Resolved from the REAL exe, NOT the cwd: a .app
    // launched by Finder has cwd=/ → a relative "bundles" path opened empty.
    let sibling_dir = std::env::current_exe()
        .ok()
        .map(|p| crate::platform::exe_sibling_dir(&p))
        .unwrap_or_else(|| PathBuf::from("."));
    // Scan the bundles ONCE at boot (pure: disk read + YAML). We look
    // NEXT TO the exe (packaged .app/.exe case); if absent, we fall back to "bundles"
    // relative to the cwd (DEV case: `cargo run`/`tauri dev` runs from the repo root,
    // where the exe is target/debug/talos but the bundles are ./bundles). Absent → empty plan.
    let sibling_bundles = sibling_dir.join("bundles");
    let bundles_dir: PathBuf = if sibling_bundles.is_dir() {
        sibling_bundles
    } else {
        PathBuf::from("bundles")
    };
    println!("[bundles] dir: {}", bundles_dir.display());
    // The flat catalog sits beside bundles/ (same dev/packaged resolution).
    let sibling_catalog = sibling_dir.join("catalog");
    let catalog_dir: PathBuf = if sibling_catalog.is_dir() {
        sibling_catalog
    } else {
        PathBuf::from("catalog")
    };
    println!("[catalog] dir: {}", catalog_dir.display());
    let plan = load_from_catalog(
        catalog_dir.to_str().unwrap_or("catalog"),
        bundles_dir.to_str().unwrap_or("bundles"),
        os,
        &|m| println!("[bundles] {m}"),
    );
    println!("[plan] {} steps", plan.steps.len());
    // The fleet's behaviour facts, read ONCE here beside the catalogue — and never again,
    // so a behaviour file landing mid-session is not seen until the next launch. That is
    // the mirror of the write side, which accumulates in memory and flushes once at the end:
    // neither the plan nor the facts may shift under a running Apply.
    //
    // `TALOS_BEHAVIOUR` is a TEST MODE, not a deployment knob: it points the READ at any
    // folder of `<id>.yaml` files, so a rung can be exercised without installing anything —
    // which is the only way the `uac` rung is verifiable on macOS at all, since watch.rs has
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
    let facts = crate::ladder::resolve_plan_facts(&plan.steps, &collected, os);
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
        // Kept so Refresh can re-read the SAME two folders. The location is resolved once
        // (beside the exe, or the cwd in dev); only its contents are re-read.
        catalog_dir,
        bundles_dir,
        profiles,
        os,
        last_seen,
        appmgmt: crate::platform::app_management_status(os),
        disk_root,
        data_dir,
        consent,
        sudo_pw: tokio::sync::Mutex::new(None),
        observed: tokio::sync::Mutex::new(std::collections::BTreeMap::new()),
        facts,
        timings,
        timed: tokio::sync::Mutex::new(crate::timings::Timings::new()),
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

/// ONE row of the `plan` message, as the front reads it.
///
/// A free function of plain data rather than a closure inside `handle_socket`, so the
/// MAPPING can be pinned by a test. `handle_socket` needs a live WebSocket, which put this
/// out of every test's reach — measured, not assumed: a mutant swapping `uac` for
/// `forbidden` here and hardcoding `slow: false` passed the entire suite. The three ladder
/// discriminants are the front's input for the widget's per-rung counts, so a silent swap
/// would mislabel rows rather than fail anything.
///
/// `uac`/`forbidden` cross as booleans and so does `slow` — the CLASSIFICATION, from the
/// shared file's worst-seen `slow_secs`. `secs` is the other half and a different question:
/// this machine's LAST-SEEN duration, from the local cache. The front needs both because it
/// answers both — how far to go (the rungs) and how long it will take ME (the estimate).
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
        "pin": s.pin, "categories": s.categories,
        "requires": s.requires, // package names this one needs (B5 transitive pull, §8)
        "uac": f.uac, "forbidden": f.forbidden, "slow": crate::ladder::is_slow(&f),
        // How long this took HERE last time, for the ladder's minutes. 0 = no local
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

/// The `plan` message: the whole catalogue as the front needs it.
///
/// ONE builder for both the connect send and the Refresh reload. Duplicating it is how the
/// two drift — a field added for the reload but not at connect (or the reverse) is a defect
/// nobody sees until a specific gesture, since both paths feed the same renderer.
fn plan_message(state: &AppState, steps: &[crate::bundles::Step]) -> Value {
    let steps_json: Vec<_> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| step_json(s, i, &state.facts, &state.timings, state.os))
        .collect();
    // The top-panel "needs" (profiles.yaml), loaded at boot. Same keys the front
    // expects (renderProfiles, app.js:164): name/emoji/usage/highlights/description/packages.
    let profiles_json: Vec<_> = state
        .profiles
        .items
        .iter()
        .map(|p| {
            json!({
                "name": p.name, "emoji": p.emoji, "usage": p.usage,
                "highlights": p.highlights, "description": p.description,
                "packages": p.packages, "needs": p.needs
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
    json!({
        "type": "plan",
        "steps": steps_json,
        "selection": read_selection(&state.data_dir),
        "profiles": profiles_json,
        "profileColumns": state.profiles.columns,
        "consent": read_consent(&state.consent),
        "build": crate::build_info::build_json(), // exact stamp of the source snapshot (no more hardcoding)
        "appmgmt": appmgmt_wire(state.appmgmt)
    })
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

    // 4) Message loop from the front. Two families:
    //  - `apply` (global Apply button, app.js:499) → apply_diff (re-scan, plan, execution).
    //  - ROW actions (app.js:598) → a single button on a row sends
    //    {type:"install"|"uninstall"|"upgrade"|"downgrade", i}. + `retry-step`
    //    {type, i, action}. We run do_step on that index (downgrade ALLOWED here:
    //    it's an explicit manual click, whereas the batch Apply excludes it).
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
                // How far to go. Sent by the front exactly as `unmanaged` is; parsed here
                // so an absent field lands on Everything rather than on nothing.
                let rung = rung_from_wire(&parsed);
                apply_diff(&mut socket, &state, on, off, unmanaged, rung).await;
            }
            "install" | "uninstall" | "upgrade" | "downgrade" => {
                if let Some(i) = parsed.get("i").and_then(|v| v.as_u64()).map(|n| n as usize) {
                    row_action(&mut socket, &state, i, kind).await;
                }
            }
            // diff: a config-atom the user turned OFF is self-managed, so its row
            // button INSPECTS instead of installing (model.js::buttonAction) — the atom
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
            // retry-step: re-runs the named action on row i (after a 403 failure).
            "retry-step" => {
                let i = parsed.get("i").and_then(|v| v.as_u64()).map(|n| n as usize);
                let action = parsed.get("action").and_then(|v| v.as_str());
                if let (Some(i), Some(action)) = (i, action) {
                    row_action(&mut socket, &state, i, action).await;
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
            // set-consent: records the sharing choice (marks consent decided).
            // No response — the UI already closed its dialog / flipped its toggle.
            "set-consent" => {
                let share = parsed
                    .get("share")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                write_consent(&state.consent, share);
                println!("[consent] set: share={share}");
            }
            // get-log: the Log tab requests the consent + the local history.
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
                let reloaded = load_from_catalog(
                    state.catalog_dir.to_str().unwrap_or("catalog"),
                    state.bundles_dir.to_str().unwrap_or("bundles"),
                    state.os,
                    &|m| println!("[bundles] {m}"),
                );
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
    // Settle the row back to a TRUE state: the atom is self-managed either way — a diff
    // never changes presence, so re-emit what we now observe rather than inventing a
    // verdict. `self` is the row state that means "you own this".
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
async fn scan_and_emit(socket: &mut WebSocket, state: &AppState) {
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

    // ⚠️ An unreadable scan is NOT "nothing is outdated". Saying so out loud is
    // the difference between a user who knows the check did not happen and one
    // who trusts a screen of green rows. The rows still work — presence is a
    // separate probe — but nothing claims to know about upgrades.
    let scan = match &scanned {
        Ok(map) => map.clone(),
        Err(reason) => {
            eprintln!("[scan] outdated UNAVAILABLE: {}", reason.message());
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
    match &bulk {
        Some(t) => println!(
            "[scan] presence (batched) {} installed in {:?}",
            t.len(),
            bulk_started.elapsed()
        ),
        None => eprintln!("[scan] presence listing UNAVAILABLE — probing per package"),
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
}

/// `log` message for the Log tab: current consent + local history.
fn log_msg(state: &AppState) -> String {
    json!({
        "type": "log",
        "consent": read_consent(&state.consent),
        "history": read_history(&state.consent),
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
/// The front sets `applyRunning = true` before sending any row-button verb and any
/// `apply` (see app.js: the row `onclick` and `applyScoped`), and `refreshLiveness()`
/// then disables every button in the panel. Nothing but a server message clears it —
/// there is no timeout, by design (a real install can take minutes, so a watchdog
/// would either fire during a legitimate install or be useless).
///
/// ⚠️ THE FAILURE MODE THIS EXISTS TO PREVENT: a verb the front sends and the server's
/// match does not handle falls into `_ => {}`, no `done` is ever emitted, and the panel
/// is dead until the window is closed — no error, no recovery, indistinguishable from a
/// hang. That happened for real with `diff` (config-atom rows): `model.js::buttonAction`
/// returns `{type:"diff"}` for a config-atom the user turned OFF, the front sent it
/// verbatim, and the server had no arm for it. `Starship config` ships in
/// `bundles/terminal.yaml`, so it was reachable with the shipped catalogue.
///
/// Keeping this list next to the handler — and asserting the handler covers it — makes
/// the next added verb a compile-adjacent concern rather than a silent freeze.
const LOCKING_CLIENT_MSGS: &[&str] = &[
    "apply",
    "install",
    "uninstall",
    "upgrade",
    "downgrade",
    "diff",
    "retry-step",
];

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

/// The rung off the wire. Absent, non-numeric or out of range → Everything.
///
/// ⚠️ The default is deliberately the LARGEST rung, not the smallest. Every front shipped
/// before the ladder omits this field, and so does any hand-rolled probe; they must get
/// today's behaviour. Silently doing LESS than the user asked would be the worse error, and
/// a rung is a preference rather than a safety property — unlike `unmanaged`, whose
/// omission the server refuses to trust (see `scope_refuses`, which reads the disk instead).
fn rung_from_wire(v: &serde_json::Value) -> crate::ladder::Rung {
    match v.get("rung").and_then(|r| r.as_u64()) {
        Some(n) => crate::ladder::Rung::from_wire(n),
        None => crate::ladder::Rung::default(),
    }
}

/// Is this row IMPOSSIBLE here because a `requires:` can never hold — the row-button
/// half of the requirement gate, reusing `deps::requires_blocked` rather than
/// re-deriving the rule (one rule, three callers: model.js, apply_diff, here).
///
/// The desires are NOT knowable on this path — `row_action` never receives the wire's
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

/// ROW action: a single button on a row (install/uninstall/upgrade/
/// downgrade). Runs do_step on that index then emits `done` to unfreeze the UI.
/// downgrade is allowed (explicit manual click, the only destructive path outside the batch).
async fn row_action(socket: &mut WebSocket, state: &AppState, i: usize, action: &str) {
    use crate::decision::Action;
    // Cloned out of the lock: the action below runs a pty, and holding a read guard
    // across it would block a Refresh for the whole install.
    let Some(step) = state.plan.read().await.steps.get(i).cloned() else {
        return;
    };
    let step = &step;
    // THE TEETH. A row the user put out of scope is not ours to touch — and this
    // path is the one the per-row button uses, so without this check the button
    // stays live on a row the panel already greyed out.
    let sel = read_selection(&state.data_dir);
    if scope_refuses(&sel, &step.name) {
        println!("[scope] refused {} on {} (out of scope)", action, step.name);
        let _ = socket
            .send(Message::Text(json!({ "type": "done" }).to_string()))
            .await;
        return;
    }
    // THE SECOND SET OF TEETH: an IMPOSSIBLE requirement. This path never sees the wire's
    // `on`/`off`, so it asks a deliberately NARROWER question than the batch does — not
    // "will the requirement be there after this Apply?" but "could it ever be here?".
    // A single button is an explicit gesture about one thing, and refusing it because a
    // requirement is merely not-yet-installed would break the legitimate click that
    // installs a plugin knowing the host is on its way.
    if let Some(why) = requirement_blocks_row(state, step).await {
        println!("[scope] refused {} on {} ({})", action, step.name, why);
        let _ = socket
            .send(Message::Text(
                json!({ "type": "state", "i": i, "reason": why }).to_string(),
            ))
            .await;
        let _ = socket
            .send(Message::Text(json!({ "type": "done" }).to_string()))
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
    // A row-upgrade is only meaningful for an outdated package; determine cask-ness
    // via a fresh outdated scan (blocking → spawn_blocking, like the other sites),
    // ONLY for Upgrade (don't scan on install/uninstall clicks). This makes the
    // single-row button use the SAME forced-cask path as the batch Apply.
    let is_cask = if act == Action::Upgrade {
        let os = state.os;
        // An unreadable scan cannot tell cask from formula. Defaulting to false
        // is the safe direction here: the cask path adds --force, and forcing on
        // a guess is worse than not forcing.
        let scan = tokio::task::spawn_blocking(move || scan_outdated(os))
            .await
            .unwrap_or(Err(crate::outdated::ScanFailure::CommandFailed(
                "the scan task did not finish".into(),
            )))
            .unwrap_or_default();
        outdated_for(step.system_id.as_deref(), &scan)
            .map(|o| o.is_cask)
            .unwrap_or(false)
    } else {
        false
    };
    if cask_upgrade_blocked(act, is_cask, state.appmgmt) {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "needs-appmgmt", "i": i }).to_string(),
            ))
            .await;
        return;
    }
    do_step(socket, state, i, act, step, is_cask).await;
    // A row action is an Apply of ONE step, and what it observed is just as real. So the
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
    clear_sudo_pw(state).await; // row action finished: clear the cached password
    let _ = socket
        .send(Message::Text(json!({ "type": "done" }).to_string()))
        .await;
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
    rung: crate::ladder::Rung,
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

    // The batched outdated scan runs FIRST, because the seed selection needs it: a
    // row that is present and wanted present is still an ACTION when it is behind.
    // It is ONE command for the whole machine (~1-2 s), and naming it matters — the
    // veil would otherwise open on a frozen phrase for exactly as long as the wait
    // people complained about. No `total` yet (the count belongs to the probes), so
    // the bar stays indeterminate here.
    let started = std::time::Instant::now();
    let _ = socket
        .send(Message::Text(
            json!({ "type": "rescan-progress", "name": "what's out of date" }).to_string(),
        ))
        .await;
    // If the scan is unreadable, `outdated(i)` is false for every row — which
    // would NARROW the re-probe set on an assumption. seeds_for_rescan already
    // re-probes anything it has no reliable knowledge of, so the safe reading of
    // a failed scan is an empty map, never a claim.
    let scan = tokio::task::spawn_blocking(move || scan_outdated(os))
        .await
        .unwrap_or(Err(crate::outdated::ScanFailure::CommandFailed(
            "the scan task did not finish".into(),
        )));
    if let Err(reason) = &scan {
        eprintln!("[apply] outdated UNAVAILABLE: {}", reason.message());
        let _ = socket
            .send(Message::Text(
                json!({ "type": "outdated-unavailable", "reason": reason.message() }).to_string(),
            ))
            .await;
    }
    let scan = scan.unwrap_or_default();

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
            let outdated = is_outdated_now(outdated_for(s.system_id.as_deref(), &scan));
            should_seed_outdated_row(s.pin.as_deref(), outdated)
        })
    };
    // An EXACT pin makes the installed VERSION the deciding fact, and presence cannot
    // report it — so an exact-pinned row is always probed. Keywords (pending, latest)
    // do NOT make version the deciding fact, so they must NOT seed — seeding them
    // would re-introduce the serial scan cost the batching work removed. `trim`
    // reasoning stays (the pin is author-written YAML), but is now inside classify_pin.
    let is_pinned_row = |i: usize| -> bool {
        steps.get(i).is_some_and(|s| {
            matches!(
                crate::bundles::classify_pin(s.pin.as_deref()),
                Some(crate::bundles::PinKind::Exact(_))
            )
        })
    };
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
    // ⭐ NO threshold for this one, unlike the bulk listing right above. The snapshot costs one
    // `code --version` plus one file read (~0.2 s measured), where a `winget list` costs ~3.7 s —
    // so there is no break-even below which it is a loss, and the fast path stays fast. And the
    // alternative is not "probe per row" but "no answer at all": without it every extension row
    // in the diff reads unknown, which is precisely the row Apply is about to act on.
    let home = crate::detect::user_home();
    let vs = std::sync::Arc::new(
        tokio::task::spawn_blocking(move || crate::vscode::snapshot(os, &home))
            .await
            .ok(),
    );
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
    let nodes: Vec<DepNode> = steps
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
    // How many rows `action_for` offered an action for, BEFORE the ladder narrowed them.
    // Counted rather than derived from `steps.len()`, which is the catalogue and not the
    // candidate set: a log line reading "2 of 31" would name the wrong denominator.
    let mut candidates = 0usize;
    let sel = read_selection(&state.data_dir);
    for (i, step) in steps.iter().enumerate() {
        // Out of scope → never an action. TWO sources, deliberately, because they
        // know different halves of the rule:
        //   · the front's `unmanaged` list carries the DERIVED half (`external`,
        //     `!canUninstall`) — live facts the server does not hold;
        //   · the persisted overrides carry the user's EXPLICIT half, and reading
        //     them here gives the batch path the same teeth `row_action` has.
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
        };
        // DOWNGRADE excluded from the Apply (only destructive path → manual button).
        if let Some(a @ (Action::Install | Action::Uninstall | Action::Upgrade)) =
            action_for(desired, &facts)
        {
            candidates += 1;
            // THE LADDER, applied HERE and not in the front. `row_action` never sees the
            // wire lists, so a front-only filter would be cosmetic — the Git-hazard lesson
            // (talos-scope-second-axis). The consequence is milder than there (a rung is a
            // preference, not a safety property), but the plan is the server's to build,
            // and building it from a rung it was TOLD is simpler than trusting a
            // pre-filtered list.
            //
            // AFTER `action_for`, not before: the rule takes the ACTION, so which action a
            // row would get has to be known first. Filtering rows earlier would also change
            // the candidate count, and `uninstall` — which no rung filters — is precisely
            // an action, not a property of a row.
            //
            // ⭐ NO FACTS ARE READ HERE ANY MORE. Three rungs replaced six, and the three that
            // went sorted upgrades by `uac` / `403` / `slow` — so this loop used to look each
            // row's facts up from the startup snapshot. The rung now depends only on WHAT KIND
            // of thing the row is, and the facts reach the user another way: `step_json` puts
            // them on the wire per row and the panel writes them in words.
            if !crate::ladder::rung_allows(rung, a, step.is_config, step.is_extension) {
                continue;
            }
            visual_plan.push((i, a));
        }
    }
    // What the rung cost, for a log reader. Both numbers are candidate counts: the
    // denominator is what `action_for` offered, NOT the catalogue's step count.
    println!(
        "[ladder] rung {}: {} of {} candidate action(s) kept",
        rung.as_str(),
        visual_plan.len(),
        candidates
    );

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

    for (i, action) in &plan {
        // is_cask from the scan we already have → do_step re-issues the forced
        // --cask --force command for cask upgrades (single resolution point).
        let is_cask = outdated_for(steps[*i].system_id.as_deref(), &scan)
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
        // A 403 is REPAIRABLE, not a plain failure: the user opens the blocked page
        // and unblocks it. So we do NOT run on — carrying on would (a) make them read
        // a page while installs scroll past behind, and (b) burn every FOLLOWING
        // package that goes through the same blocked source, when one unblock up front
        // would have saved them all. Instead: the action that was running FINISHES
        // (never kill a pty mid-flight — a half-installed package is the worst case;
        // the exit code is the truth), and THEN we wait for the user.
        //
        // Resuming is gated on a DECISION, not on success: they may fail to unblock.
        // Either way it's their call — nothing restarts until they say so. And Retry
        // means RETRY: the same action runs again HERE, in place, before the plan
        // moves on. (It used to mean "continue", which left the user to hunt the row
        // down afterwards — the word on the button lied.)
        let mut attempt: u32 = 0;
        let stop_all = loop {
            attempt += 1;
            let outcome = do_step(socket, state, *i, *action, &steps[*i], is_cask).await;
            if !outcome.blocked {
                break false;
            }
            match await_forbidden_decision(socket, *i, attempt).await {
                ForbiddenChoice::Retry => continue, // same row, same action, again
                ForbiddenChoice::Continue => break false,
                ForbiddenChoice::Stop => break true,
            }
        };
        if stop_all {
            break; // "stop everything" → abandon the rest of the plan
        }
    }
    // ONE write per Apply, here at the end, for the packages it touched. Reached on every
    // way out of the loop above — a normal finish, a "stop everything", and a run whose
    // rows were all cancelled: those `break`s leave the loop, not this function.
    //
    // The `done/nothing` early return above DOES skip it, and so do all four of
    // row_action's guards: unknown index, out of scope, an unknown action verb (reachable
    // — `retry-step` forwards an arbitrary wire string), and needs-appmgmt. Every one of
    // them returns BEFORE any do_step, so there is nothing accumulated to lose — but that
    // is WHY they are safe, not an accident to rely on: a future early exit placed AFTER a
    // do_step would drop that step's facts silently.
    flush_behaviour(state).await;
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
    /// `forbidden::is403` matched the streamed bytes. RAW — not "the step failed with a
    /// 403"; see the observation site in `do_step` for why that distinction is kept.
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

    // 403 scan + sudo prompt detection as it streams. The "Password:" prompt
    // is NOT followed by a newline (sudo writes it raw), so we test the END of the
    // current buffer. Once answered, we don't re-ask for this step.
    let mut buf = String::new();
    let mut forbidden = false;
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
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if !forbidden && crate::forbidden::is403(&buf) {
            forbidden = true;
        }
        let out = json!({ "type": "out", "i": i, "data": STANDARD.encode(&chunk) });
        if socket.send(Message::Text(out.to_string())).await.is_err() {
            return PtyRun {
                code: -1,
                forbidden,
                url: None,
                cancelled,
                saw_window,
            };
        }
        let tail = buf.trim_end().to_lowercase();
        // A REFUSAL invalidates whatever we just sent. If it came from the cache, drop
        // it: the whole Apply would otherwise keep replaying the same wrong secret,
        // every sudo row hanging in turn. Clearing makes the next prompt ask the user.
        if tail.ends_with("try again.") && last_from_cache {
            clear_sudo_pw(state).await;
            last_from_cache = false;
        }
        // sudo writes "Password:" (or "Password for X:") WITHOUT a final newline, so we
        // test the ACCUMULATED BUFFER (the prompt may arrive in its own chunk). We answer
        // EACH prompt up to sudo's own budget — a re-prompt means the last answer was
        // refused, and leaving it unanswered is a permanent hang, not a safeguard.
        // The buffer is TRUNCATED (not cleared) after answering, so the SAME prompt is
        // not re-detected on the next chunk — its tail would still end with ':' and we
        // would answer a question already answered, burning the attempt budget on
        // nothing. A tail is kept because `is403` reads this buffer too: wiping it whole
        // could split a "Forbidden (403)" across the cut and lose it.
        if sudo_tries < SUDO_MAX_TRIES && tail.ends_with(':') && tail.contains("password") {
            if let Some((pw, from_cache)) = obtain_sudo_pw(socket, state, i).await {
                let mut line = pw.into_bytes();
                line.push(b'\n');
                let _ = in_tx.send(line);
                sudo_tries += 1;
                last_from_cache = from_cache;
                buf.push_str("\n[answered]\n"); // breaks the ':' tail without dropping context
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
    }
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

// Benign exit codes (winget: "already installed / no applicable upgrade").
// A non-zero exit in this list = success anyway. Port of BENIGN_CODES.
fn benign_code(code: i32) -> bool {
    matches!(code, -1978335189 | -1978335212)
}

/// The outcome of one step. `ok` = it succeeded (kept for callers that will want it;
/// the front already learns it from the `step` event). `blocked` = it failed BECAUSE
/// the corporate firewall answered 403 — the distinction the Apply loop needs: a plain
/// failure moves on, a 403 is REPAIRABLE by the user, so the loop pauses and waits.
#[derive(Default, Clone, Copy)]
struct StepOutcome {
    #[allow(dead_code)]
    ok: bool,
    blocked: bool,
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
    // A cask Upgrade needs the forced --cask --force command (brew's receipt drift
    // → anti-clobber exit 1 otherwise). Single resolution point for BOTH callers
    // (batch Apply + row button); formulae, non-brew and pinned casks keep step.upgrade.
    let forced = if action == Action::Upgrade {
        forced_cask_upgrade_cmd(step, os, is_cask)
    } else {
        None
    };
    let cmd = match action {
        Action::Install => step.install.as_deref(),
        Action::Uninstall => step.uninstall.as_deref(),
        Action::Upgrade => forced.as_deref().or(step.upgrade.as_deref()),
        Action::Downgrade => step.downgrade.as_deref(),
    };
    let Some(cmd) = cmd else {
        return StepOutcome::default(); // no command for this route → skip
    };
    let running = match action {
        Action::Uninstall => "uninstalling",
        Action::Upgrade => "upgrading",
        Action::Downgrade => "downgrading",
        Action::Install => "installing",
    };
    let _ = socket
        .send(Message::Text(
            json!({ "type": "step", "i": i, "status": running }).to_string(),
        ))
        .await;

    // The bare command is passed to run_in_pty, which wraps it in the native shell
    // (POSIX: user shell in -ilc via pty_shell; Windows: powershell + PATH refresh).
    let PtyRun {
        code,
        forbidden,
        url,
        cancelled,
        saw_window,
    } = run_in_pty(socket, state, i as u32, cmd).await;
    let ok = code == 0 || benign_code(code);

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

    let blocked = !ok && forbidden;
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
    // OBSERVES a presence must write it. There are three: scan_and_emit, the apply
    // re-scan, and here.
    let version = if ok {
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
        if action == Action::Uninstall {
            String::new()
        } else {
            v
        }
    } else {
        String::new()
    };
    // What this step OBSERVED, for the shared behaviour file. Accumulated in memory
    // only; the write happens once, when the Apply ends.
    //
    // `uac` is Windows-only and comes from the watcher (a foreign window appeared during
    // the step) — a signal that cannot distinguish an elevation prompt from an
    // installer's own window, which the monotone design tolerates: a false positive
    // costs one rung of caution, never a wrong promise.
    //
    // `forbidden` is `run_in_pty`'s raw signal: `forbidden::is403` matched the streamed
    // bytes. It is deliberately NOT the `blocked` computed above — that one is
    // `!ok && forbidden`, the UI verdict, which is right for a modal ("offer Retry") and
    // wrong for a fact ("this package's source is blocked here"). A tool that prints a 403
    // for one mirror and then succeeds from another still met the firewall, and the ladder
    // wants to know. The cost is is403's own imprecision — a `\b403\b` near a
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
    append_history(
        &state.consent,
        &HistEntry {
            at: chrono::Utc::now().to_rfc3339(),
            package: step.name.clone(),
            version,
            action: action.as_str().to_string(),
            ok,
            secs: started.elapsed().as_secs(),
        },
    );
    StepOutcome { ok, blocked }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::{Overrides, Posture, Step};
    use crate::managers::Outdated;
    use crate::platform::Os;

    /// A minimal Step for the forced-cask tests: only `system_id`/`pin`/`upgrade`
    /// matter here, the rest are inert defaults.
    fn brew_step(system_id: &str, pin: Option<&str>) -> Step {
        Step {
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
            requires: Vec::new(),
            posture: Posture::OptIn,
            categories: Vec::new(),
            overrides: Overrides::default(),
        }
    }

    fn seen(present: bool, version: &str) -> crate::detect::Presence {
        crate::detect::Presence {
            present: Some(present),
            version: Some(version.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_rows_behaviour_crosses_the_wire_under_the_names_the_front_reads() {
        // The three ladder discriminants, keyed and mapped. Written because a mutant that
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
                slow_secs: crate::ladder::SLOW_SECS,
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
            slow_secs: crate::ladder::SLOW_SECS + 182,
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

    // ---- the rung on the wire ------------------------------------------------------
    //
    // `apply_diff` needs a live WebSocket, so the FILTER SITE inside it is not reachable
    // from a unit test — `ladder::rung_allows` is tested exhaustively in its own module
    // (a table, one row per candidate, one column per rung), and Task 8 verifies the
    // wiring at real clicks. What IS testable here is the wire parse, which is where a
    // rung can silently become the wrong one.
    //
    // Read this beside `scope_refuses_from_disk_so_omitting_the_wire_list_is_not_a_bypass`
    // just above: the two assert OPPOSITE stances on an omitted field, on purpose. Scope is
    // a safety property, so its omission is not trusted; a rung is a preference, so its
    // omission means the largest one.
    #[test]
    fn the_rung_comes_off_the_wire_and_an_absent_one_means_everything() {
        use crate::ladder::Rung;
        let msg = |raw: &str| -> Rung {
            let v: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
            super::rung_from_wire(&v)
        };
        assert_eq!(msg(r#"{"type":"apply","rung":0}"#), Rung::ConfigOnly);
        // ⚠️ THE NUMBERS SHIFTED when Extensions was inserted at 1: what used to be rung 2
        // (unattended) is now 3, and 4 is no longer the top. Safe because the rung is NOT
        // persisted (app.js says so explicitly), so no stored preference can be re-read under
        // the new numbering — the only exposure is a stale front within one launch, which the
        // absent-means-everything case below already covers.
        assert_eq!(msg(r#"{"type":"apply","rung":1}"#), Rung::Extensions);
        assert_eq!(msg(r#"{"type":"apply","rung":2}"#), Rung::Apps);
        // THE case that matters: a client that says nothing about the rung must get
        // today's behaviour, not the smallest one. Doing silently LESS than the user asked
        // is the worse direction of error, and every front shipped before this change
        // omits the field.
        assert_eq!(
            msg(r#"{"type":"apply","on":[1],"off":[]}"#),
            Rung::Apps,
            "an absent rung must not shrink the Apply"
        );
        // And garbage is not trusted into a smaller rung either. `as_u64` returns None for
        // a string and for a negative, so both land on the default rather than on rung 0 —
        // which is only safe BECAUSE the default is the largest rung.
        assert_eq!(msg(r#"{"type":"apply","rung":"config"}"#), Rung::Apps);
        assert_eq!(msg(r#"{"type":"apply","rung":-1}"#), Rung::Apps);
        assert_eq!(msg(r#"{"type":"apply","rung":99}"#), Rung::Apps);
        // Not even a message that failed to parse at all: `unwrap_or_default` gives Null,
        // and Null has no `rung`. (The `apply` arm reaches this with the same value the
        // `type` match read, so a Null can never actually get here — asserted so the
        // function stays total if that ever changes.)
        assert_eq!(msg("not json at all"), Rung::Apps);
    }

    /// WHERE the filter is, and where it must NOT be. Text-level for the same reason as
    /// `every_ui_locking_message_has_a_handler` and `every_presence_observation_is_recorded`:
    /// both sites need a live WebSocket, so no unit test can call them — and both defects are
    /// about the PRESENCE or POSITION of a call, which the source shows exactly.
    ///
    /// Verified to bite. FOUR mutants, each of which the rest of the suite passed happily
    /// and this test failed on: consulting `rung_allows` inside `row_action`; filtering
    /// BEFORE `action_for` on a presumed action; applying the filter to a hardcoded
    /// `Action::Install` instead of to `a`; and deleting the filter outright.
    #[test]
    fn the_ladder_filters_the_batch_after_action_for_and_never_a_row_button() {
        // ⚠️ `\r` stripped FIRST. `include_str!` embeds the file as it sits on disk, and
        // with no `.gitattributes` git checks this tree out CRLF on Windows — so the
        // `"\n}\n"` this test looks for to find the end of a function body matched nothing
        // there and the `expect` panicked. Normalised at the source rather than at the one
        // search, so every assertion below reasons about the same text on every platform.
        //
        // ⭐ A PRE-EXISTING defect, found on the FIRST run of the windows-latest CI job
        // added in the same commit as this note — 225 tests green, this one red, and
        // ubuntu-only CI could never have seen it. That is the class of thing that job is
        // for, and it earned its cost immediately.
        let src = include_str!("server.rs").replace('\r', "");
        // Ignore this test module, or its own mentions would satisfy the assertions.
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];

        // ---- the per-row button IGNORES the ladder --------------------------------
        // Clicking install on one row is an explicit gesture about one thing. The ladder
        // calibrates the BATCH; a row button that consulted it would refuse a click the
        // user just made, which is the opposite of what a button is for.
        let at = code
            .find("async fn row_action(")
            .expect("row_action moved — re-point this test");
        let body = &code[at..at + code[at..].find("\n}\n").expect("row_action's body ends")];
        // The CALL, not the word: a future comment in there is free to explain why the
        // ladder is absent, and should not fail this.
        assert!(
            !body.contains("rung_allows(") && !body.contains("rung_from_wire("),
            "row_action consults the ladder → a per-row click, which is an explicit gesture \
             about ONE package, would be refused by a preference about the batch"
        );

        // ---- the batch filter sits AFTER action_for -------------------------------
        // `rung_allows` takes the ACTION, so which action a row would get has to be known
        // first. Filtering earlier would decide on a PRESUMED action (and would change the
        // candidate count the log line reports).
        //
        // Both `find`s are FIRST occurrences, which is exact here only because each string
        // appears once in the non-test source — asserted, so a second call site cannot make
        // this comparison quietly meaningless.
        assert_eq!(
            code.matches("rung_allows(").count(),
            1,
            "a second rung_allows call site appeared — this ordering check reads only the \
             first, so re-write it rather than trusting it"
        );
        let decides = code
            .find("action_for(desired, &facts)")
            .expect("the visual_plan loop's action_for call moved — re-point this test");
        let filters = code
            .find("rung_allows(")
            .expect("nothing calls rung_allows → the ladder governs nothing at all");
        assert!(
            filters > decides,
            "the rung filter runs BEFORE action_for, so it judges a presumed action rather \
             than the one the shared rule chose"
        );
        // And on THAT action, not on one reconstructed beside it.
        //
        // ⚠️ The literal call is pinned on purpose, so ADDING an argument turns this red and
        // forces a human to confirm the new one is threaded from the right place. It bit when
        // `is_extension` was added — which is the guard working, not a nuisance: a rung
        // criterion read off the wrong value would filter the wrong rows, silently.
        assert!(
            code[filters..].starts_with("rung_allows(rung, a, step.is_config, step.is_extension)"),
            "the filter must be applied to `a` (the action action_for returned) and to THIS \
             step's own flags — re-check the argument order if this just started failing"
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
            catalog_dir: share.join("catalog"),
            bundles_dir: share.join("bundles"),
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
            observed: tokio::sync::Mutex::new(std::collections::BTreeMap::new()),
            // Empty, matching the empty plan above: these tests exercise the WRITE side,
            // and the resolved beliefs are exactly what must never travel that way.
            facts: Vec::new(),
            // Empty for the same reason: `timings` is the READ snapshot, and the write side
            // must never consult it — a flush that merged the boot snapshot back in would
            // re-write, on every Apply, durations this Apply never measured.
            timings: crate::timings::Timings::new(),
            timed: tokio::sync::Mutex::new(crate::timings::Timings::new()),
        }
    }

    fn share(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&d);
        d
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
    fn the_row_button_refuses_a_requirement_blocked_row_too() {
        // The per-row button never sees the wire lists, so it needs its own teeth —
        // exactly the asymmetry `scope_refuses` was added to close, now closed on the
        // second derivation as well.
        let body = fn_body("row_action");
        assert!(
            body.contains("requirement_blocks_row"),
            "row_action has teeth for the user's explicit scope override but not for an \
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

    #[test]
    fn the_batch_gate_sits_where_the_other_two_scope_gates_sit() {
        // ONE skip, three sources, one place. A second `continue` further down would be
        // the drift that made `row_action` and the batch path disagree once already.
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
