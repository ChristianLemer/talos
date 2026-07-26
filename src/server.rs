use axum::{
    body::Body,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE},
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

use crate::bundles::{load_from_catalog, Plan};
use crate::consent::{
    append_history, clear_history, read_consent, read_history, write_consent, ConsentStore,
    HistEntry,
};
use crate::detect::detect_present_detailed;
use crate::outdated::{outdated_for, scan_outdated};
use crate::platform::{current_os, local_data_dir, Os};
use crate::profiles::{load_profiles, Profiles};
use crate::selection::{read_selection, write_selection, Selection};

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
/// guard → exit 1). Returns None for formulae, non-brew routes, and PINNED
/// steps (a pin owns the direction — keep its precomputed install-pinned cmd).
/// `is_cask` comes from the brew outdated scan bucket (the single source).
fn forced_cask_upgrade_cmd(
    step: &crate::bundles::Step,
    os: crate::platform::Os,
    is_cask: bool,
) -> Option<String> {
    use crate::managers::{native_manager, IdField};
    if !is_cask || step.pin.is_some() {
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

/// The server's shared state: the Plan scanned ONCE at startup (pure data,
/// no pty/network), plus the current OS. Cloned (Arc) into each connection.
struct AppState {
    plan: Plan,
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
    // The top-panel "needs" (profiles.yaml lives INSIDE the same bundles dir; the
    // bundle scan skips it because it only reads subfolders with a bundle.yaml).
    let profiles = load_profiles(bundles_dir.to_str().unwrap_or("bundles"));
    println!("[plan] {} profiles", profiles.items.len());
    // Per-machine local data-dir + consent store. exe_dir = the SAME folder
    // next to the exe (the one that contains bundles/) — that's where the consented
    // shared copy lands. host/user name the shared log; env best-effort.
    let data_dir = local_data_dir(os);
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
        plan,
        profiles,
        os,
        last_seen,
        appmgmt: crate::platform::app_management_status(os),
        disk_root,
        data_dir,
        consent,
        sudo_pw: tokio::sync::Mutex::new(None),
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
// (like src/server.ts:909 which upgrades on the header, not on a fixed pathname).
async fn root_or_ws(ws: Option<WebSocketUpgrade>, state: Arc<AppState>) -> Response {
    match ws {
        Some(ws) => ws.on_upgrade(move |socket| handle_socket(socket, state)),
        // index.html comes from the sealed assets (or from disk in dev) — no more
        // read_to_string on a path relative to the cwd (shortcut #3 fixed).
        None => serve_asset("/", &state),
    }
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let steps = &state.plan.steps;

    // 1) REAL plan — flat steps scanned from the catalog (no more hardcoding). Same keys
    // the front expects (see app.js render/ws.onmessage). Bundles are the top cards,
    // sent separately as `profiles` (profiles.rs) — steps no longer carry a bundle layer.
    let steps_json: Vec<_> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "i": i, "name": s.name, "description": s.description, "bundle": s.bundle,
                "canUninstall": s.uninstall.is_some(), "posture": s.posture.as_str(),
                "isConfig": s.is_config, "pin": s.pin, "categories": s.categories,
                "requires": s.requires // package names this one needs (B5 transitive pull, §8)
            })
        })
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
    // opens the sharing dialog. REAL selection: the persisted toggles the
    // front restores (yellow). Intent is remembered, presence is re-detected.
    let plan = json!({
        "type": "plan",
        "steps": steps_json,
        "selection": read_selection(&state.data_dir),
        "profiles": profiles_json,
        "profileColumns": state.profiles.columns,
        "consent": read_consent(&state.consent),
        "build": crate::build_info::build_json(), // exact stamp of the source snapshot (no more hardcoding)
        "appmgmt": appmgmt_wire(state.appmgmt)
    });
    let _ = socket.send(Message::Text(plan.to_string())).await;

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
                apply_diff(&mut socket, &state, on, off).await;
            }
            "install" | "uninstall" | "upgrade" | "downgrade" => {
                if let Some(i) = parsed.get("i").and_then(|v| v.as_u64()).map(|n| n as usize) {
                    row_action(&mut socket, &state, i, kind).await;
                }
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
                scan_and_emit(&mut socket, &state).await;
            }
            // open-forbidden: the "Open blocked page" button of the 403 banner → opens
            // the blocked URL in the default browser, next to the panel, so the
            // user approves the firewall access then Retry. Handler MISSING in the
            // Deno→Rust port → the click fell into _ => {} and did nothing.
            "open-forbidden" => {
                if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                    if let Err(e) = crate::platform::open_url(url) {
                        println!("[forbidden] failed to open {url}: {e}");
                    } else {
                        println!("[forbidden] opening browser: {url}");
                    }
                }
            }
            // Deep-link straight to System Settings → Privacy & Security → App
            // Management, so the user can grant the permission a cask upgrade needs.
            "open-appmgmt-settings" => {
                let url = "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AppBundles";
                if let Err(e) = crate::platform::open_url(url) {
                    println!("[appmgmt] failed to open settings: {e}");
                }
            }
            _ => {}
        }
    }
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
    let steps = &state.plan.steps;
    let started = std::time::Instant::now();
    let scan = tokio::task::spawn_blocking(move || scan_outdated(os))
        .await
        .unwrap_or_default();
    println!("[scan] outdated (batched) in {:?}", started.elapsed());
    for (i, step) in steps.iter().enumerate() {
        let step_owned = step.clone();
        let probe_started = std::time::Instant::now();
        let p = tokio::task::spawn_blocking(move || detect_present_detailed(&step_owned, os))
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

/// ROW action: a single button on a row (install/uninstall/upgrade/
/// downgrade). Runs do_step on that index then emits `done` to unfreeze the UI.
/// downgrade is allowed (explicit manual click, the only destructive path outside the batch).
async fn row_action(socket: &mut WebSocket, state: &AppState, i: usize, action: &str) {
    use crate::decision::Action;
    let Some(step) = state.plan.steps.get(i) else {
        return;
    };
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
        let scan = tokio::task::spawn_blocking(move || scan_outdated(os))
            .await
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
    clear_sudo_pw(state).await; // row action finished: clear the cached password
    let _ = socket
        .send(Message::Text(json!({ "type": "done" }).to_string()))
        .await;
}

/// The heart: applies the tri-state decision against the machine reality.
///   on  = indices wanted PRESENT; off = indices wanted ABSENT.
/// Re-detects presence NOW (repaint-at-apply: re-observes before acting,
/// does not trust the connection scan), repaints the pills, asks the
/// SHARED rule action_for what to do, orders by dependencies (topo_sort), emits
/// `apply-plan` (⚠️ which REMOVES the "Plotting the gallop…" veil — shortcut #1 from
/// the spike fixed), then runs each step. Port of applyDiff (src/server.ts).
async fn apply_diff(socket: &mut WebSocket, state: &AppState, on: Vec<usize>, off: Vec<usize>) {
    use crate::decision::{action_for, Action, Desired, MachineFacts};
    use crate::deps::{index_of_names, make_index, requires_reason, topo_sort, DepNode};
    use std::collections::HashSet;

    let os = state.os;
    let steps = &state.plan.steps;
    let want_on: HashSet<usize> = on.iter().copied().collect();
    let want_off: HashSet<usize> = off.iter().copied().collect();

    // WHAT to re-probe: the diff (on ∪ off) plus what its `requires` pull, transitively.
    // Not all 30. The repaint-at-apply guarantee (re-observe before acting, catch a
    // manual removal — memory repaint-at-apply) is only MEANINGFUL for rows we are
    // about to touch: probing Miro to install Bun buys nothing and costs a process.
    // Requirements are in, because will_be_present decides both the `requires` reasons
    // and the execution order. Dependents are out — untouched, unchanged.
    let seeds: Vec<usize> = on.iter().chain(off.iter()).copied().collect();
    let name_idx = index_of_names(steps.iter().map(|s| s.name.as_str()));
    let requires: Vec<Vec<String>> = steps.iter().map(|s| s.requires.clone()).collect();
    let scope = crate::deps::rescan_scope(&requires, &name_idx, &seeds);
    // Catalogue order, so the narration counts up the screen and not at random
    // (a HashSet iterates in whatever order it likes).
    let mut to_probe: Vec<usize> = scope.into_iter().collect();
    to_probe.sort_unstable();

    // SERIAL live re-scan (presence) + batched outdated, like at connect.
    // ⚠️ Serial on purpose — same reason as scan_and_emit: concurrent winget probes
    // contend over winget's shared state. Do not fan this back out. If the scoped
    // scan is ever still too slow, the fix is BATCH (one list command), never threads.
    let started = std::time::Instant::now();
    // Name the batched step too: it is ONE command for the whole machine, ~1-2 s, and
    // it runs BEFORE the first probe — so without this the veil opens on a frozen
    // phrase for exactly as long as the wait people complained about. No `total` yet
    // (the count belongs to the probes), so the bar stays indeterminate here.
    let _ = socket
        .send(Message::Text(
            json!({ "type": "rescan-progress", "name": "what's out of date" }).to_string(),
        ))
        .await;
    let scan = tokio::task::spawn_blocking(move || scan_outdated(os))
        .await
        .unwrap_or_default();
    let mut probed: std::collections::HashMap<usize, crate::detect::Presence> =
        std::collections::HashMap::with_capacity(to_probe.len());
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
        let p = tokio::task::spawn_blocking(move || detect_present_detailed(&step_owned, os))
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
    // Note the invariant that makes this safe rather than merely better: every presence
    // this function actually READS is fresh. `requires_reason` runs only over the probed
    // rows, and it reads the requirements of those rows — which rescan_scope pulled in,
    // so they are probed too. `topo_sort` only draws edges between packages IN the plan,
    // and the plan is a subset of the diff. The remembered values fill the vector so the
    // indices line up; they are not what the decisions rest on.
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
    for (i, step) in steps.iter().enumerate() {
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
            visual_plan.push((i, a));
        }
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
        let outcome = do_step(socket, state, *i, *action, &steps[*i], is_cask).await;
        // A 403 is REPAIRABLE, not a plain failure: the user opens the blocked page
        // and unblocks it. So we do NOT run on — carrying on would (a) make them read
        // a page while installs scroll past behind, and (b) burn every FOLLOWING
        // package that goes through the same blocked source, when one unblock up front
        // would have saved them all. Instead: the action that was running FINISHES
        // (never kill a pty mid-flight — a half-installed package is the worst case;
        // the exit code is the truth), and THEN we wait for the user.
        //
        // Resuming is gated on a DECISION, not on success: they may fail to unblock.
        // Either way it's their call — nothing restarts until they say so.
        if outcome.blocked && !await_forbidden_decision(socket, *i).await {
            break; // "stop everything" → abandon the rest of the plan
        }
    }
    clear_sudo_pw(state).await; // end of Apply: the cached password is cleared (model C)
    let _ = socket
        .send(Message::Text(json!({ "type": "done" }).to_string()))
        .await;
}

/// Port of runInPty (src/server.ts:266-348): streams the command into a pty,
/// scans the 403 as it streams, ticks the watcher (Windows). The pty runs in a
/// blocking thread; mpsc channel → async. RETURNS (code, forbidden, url) — the
/// verdict (done/step/overlay) is left to the caller (do_step), like the TS.
async fn run_in_pty(
    socket: &mut WebSocket,
    state: &AppState,
    i: u32,
    cmdline: &str,
) -> (i32, bool, Option<String>) {
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
    // surfaced behind the panel, dedup by title. Root = our pid (like Deno.pid).
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
    std::thread::spawn(move || {
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let code = crate::pty::run(&program, &arg_refs, Some(in_rx), |bytes| {
            let _ = tx.send(bytes.to_vec());
        })
        .unwrap_or(-1);
        let _ = code_tx.send(code);
    });

    // 403 scan + sudo prompt detection as it streams. The "Password:" prompt
    // is NOT followed by a newline (sudo writes it raw), so we test the END of the
    // current buffer. Once answered, we don't re-ask for this step.
    let mut buf = String::new();
    let mut forbidden = false;
    let mut sudo_answered = false;
    while let Some(chunk) = rx.recv().await {
        // Always accumulate (the sudo prompt may be split across several chunks,
        // or "Password:" arrive in a separate piece → testing the chunk alone misses it).
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if !forbidden && crate::forbidden::is403(&buf) {
            forbidden = true;
        }
        let out = json!({ "type": "out", "i": i, "data": STANDARD.encode(&chunk) });
        if socket.send(Message::Text(out.to_string())).await.is_err() {
            return (-1, forbidden, None);
        }
        // Sudo prompt? sudo writes "Password:" (or "Password for X:") WITHOUT a final
        // newline — the fix that matters is to test the ACCUMULATED BUFFER (the prompt may
        // arrive in a separate chunk), not to multiply patterns. Answers ONCE
        // per step; password from the cache (model C) or asked to the front.
        let tail = buf.trim_end().to_lowercase();
        if !sudo_answered && tail.ends_with(':') && tail.contains("password") {
            if let Some(pw) = obtain_sudo_pw(socket, state, i).await {
                let mut line = pw.into_bytes();
                line.push(b'\n');
                let _ = in_tx.send(line);
                sudo_answered = true;
            }
        }
        // Relay the watcher signals (non-blocking) as they stream.
        #[cfg(target_os = "windows")]
        while let Ok(wmsg) = watch_rx.try_recv() {
            let _ = socket.send(Message::Text(wmsg.to_string())).await;
        }
    }

    // pty finished: stop the watcher + wait-clear (like watcher.stop()).
    #[cfg(target_os = "windows")]
    {
        watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        while let Ok(wmsg) = watch_rx.try_recv() {
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
    (code, forbidden, url)
}

/// Obtains the sudo password — MODEL C. If the RAM cache already holds it (entered
/// earlier in this Apply), we reuse it without re-asking. Otherwise we ask the
/// front (message `sudo-prompt`), wait for its response (`sudo-pw`), cache it.
/// The password NEVER touches disk/log/journal. Cleared by clear_sudo_pw
/// at the end of the Apply. None if the front cancels (closes the modal → `sudo-cancel`).
async fn obtain_sudo_pw(socket: &mut WebSocket, state: &AppState, i: u32) -> Option<String> {
    // 1) cache?
    {
        let guard = state.sudo_pw.lock().await;
        if let Some(pw) = guard.as_ref() {
            return Some(pw.clone());
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
                return Some(pw);
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

/// A step was blocked by the firewall (403). PAUSE the Apply and wait for the user
/// to decide — they go unblock the page, then tell us to carry on. Same shape as
/// obtain_sudo_pw: send a prompt, block on THIS socket until the answer arrives.
///
/// Returns true to keep going with the rest of the plan, false to stop everything.
/// A closed socket (window shut mid-pause) reads as "stop" — nothing should run on
/// with nobody watching.
async fn await_forbidden_decision(socket: &mut WebSocket, i: usize) -> bool {
    let _ = socket
        .send(Message::Text(
            json!({ "type": "forbidden-pause", "i": i }).to_string(),
        ))
        .await;
    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(txt) = msg else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
        match parsed.get("type").and_then(|t| t.as_str()) {
            // The user dealt with it (unblocked or gave up — their call either way).
            Some("forbidden-continue") => return true,
            Some("forbidden-stop") => return false,
            // "Open blocked page" MUST keep working while we hold: it is the very
            // gesture the pause exists for. Served here too, because the main
            // handler loop isn't reading the socket while we own it.
            Some("open-forbidden") => {
                if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                    if let Err(e) = crate::platform::open_url(url) {
                        println!("[forbidden] failed to open {url}: {e}");
                    } else {
                        println!("[forbidden] opening browser: {url}");
                    }
                }
            }
            _ => {} // anything else is ignored while paused — nothing advances
        }
    }
    false
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
/// Port of doStep (src/server.ts:400-460).
async fn do_step(
    socket: &mut WebSocket,
    state: &AppState,
    i: usize,
    action: crate::decision::Action,
    step: &crate::bundles::Step,
    is_cask: bool,
) -> StepOutcome {
    use crate::decision::Action;
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
    let (code, forbidden, url) = run_in_pty(socket, state, i as u32, cmd).await;
    let ok = code == 0 || benign_code(code);

    // Diagnostic line in the row's terminal.
    let line = format!(
        "\r\n\x1b[2m[{}] exit {code} → {}\x1b[0m\r\n",
        action.as_str(),
        if ok { "ok" } else { "failed" }
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
    let status = if ok {
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

    // Re-detect presence AFTER a successful install/upgrade (like captureVersion) —
    // and RE-EMIT a `state` to the front, so the fresh version shows RIGHT
    // AWAY (before, it was only computed for the journal → the front kept
    // "absent/no version" until a manual refresh). The front's `state` handler
    // does setInstalledVersion + paintVersion, so nothing to change on the UI side.
    let version = if ok && action != Action::Uninstall {
        let step_c = step.clone();
        let p = tokio::task::spawn_blocking(move || detect_present_detailed(&step_c, os))
            .await
            .ok()
            .unwrap_or_default();
        let v = p.version.clone().unwrap_or_default();
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
        v
    } else {
        String::new()
    };
    append_history(
        &state.consent,
        &HistEntry {
            at: chrono::Utc::now().to_rfc3339(),
            package: step.name.clone(),
            version,
            action: action.as_str().to_string(),
            ok,
        },
    );
    StepOutcome { ok, blocked }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::{Posture, Step};
    use crate::managers::Outdated;
    use crate::platform::Os;

    /// A minimal Step for the forced-cask tests: only `system_id`/`pin`/`upgrade`
    /// matter here, the rest are inert defaults.
    fn brew_step(system_id: &str, pin: Option<&str>) -> Step {
        Step {
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
            version_regex: None,
            pin: pin.map(|p| p.into()),
            requires: Vec::new(),
            posture: Posture::OptIn,
            categories: Vec::new(),
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
        // pinned cask → None (a pin owns the direction, keep install-pinned cmd)
        let pinned = brew_step("visual-studio-code", Some("1.130.0"));
        assert_eq!(forced_cask_upgrade_cmd(&pinned, Os::Darwin, true), None);
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
}
