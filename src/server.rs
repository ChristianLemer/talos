use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::header::{HeaderValue, CACHE_CONTROL},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::json;
use std::sync::Arc;
use tower_http::services::ServeDir;

use crate::bundles::{load_bundles, Plan};
use crate::detect::detect_present_detailed;
use crate::outdated::{outdated_for, scan_outdated};
use crate::platform::{current_os, Os};

/// L'état partagé du serveur : le Plan scanné UNE fois au démarrage (données pures,
/// pas de pty/réseau), plus l'OS courant. Cloné (Arc) dans chaque connexion.
struct AppState {
    plan: Plan,
    os: Os,
    /// Cache du mot de passe sudo — modèle C : demandé la 1re fois qu'un step en a
    /// besoin, réutilisé pour les steps suivants du même Apply, EFFACÉ à la fin.
    /// RAM SEULEMENT, jamais disque/log/journal. tokio Mutex (accès async).
    sudo_pw: tokio::sync::Mutex<Option<String>>,
}

/// Démarre le serveur HTTP+WS sur 127.0.0.1:1420.
/// - `/`         → sert index.html (ou upgrade WS si l'en-tête Upgrade est présent)
/// - autres      → ServeDir sur public/ (app.js, vendor/, css…)
pub async fn serve(public_dir: String) {
    let os = current_os();
    // Scan des bundles UNE fois au boot (pur : lecture disque + YAML). Le dossier
    // bundles/ vit À CÔTÉ du binaire (frontière hermétique). Absent → plan vide.
    let plan = load_bundles("bundles", os, &|m| println!("[bundles] {m}"));
    println!(
        "[plan] {} bundles, {} steps",
        plan.bundles.len(),
        plan.steps.len()
    );
    let state = Arc::new(AppState { plan, os, sudo_pw: tokio::sync::Mutex::new(None) });

    let public = Arc::new(public_dir);
    let public_for_root = public.clone();
    let state_for_root = state.clone();
    let app = Router::new()
        .route(
            "/",
            get(move |ws: Option<WebSocketUpgrade>| {
                let public = public_for_root.clone();
                let state = state_for_root.clone();
                async move { root_or_ws(ws, public, state).await }
            }),
        )
        .fallback_service(ServeDir::new(public.as_str()))
        // ANTI-CACHE : le webview (WKWebView macOS / WebView2 Windows) garde app.js
        // en cache disque entre deux lancements → un vieux app.js sans le dernier
        // handler (ex. sudo-prompt) survivait aux rebuilds → le message arrivait mais
        // tombait dans le default du switch, sans erreur. no-store force le frais.
        .layer(axum::middleware::from_fn(no_cache));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:1420")
        .await
        .expect("bind 127.0.0.1:1420 failed");
    axum::serve(listener, app).await.expect("axum serve failed");
}

/// Middleware : ajoute `Cache-Control: no-store` à toute réponse, pour que le webview
/// ne serve jamais un app.js/index.html périmé après un rebuild (cause du modal sudo
/// qui "ne s'affichait pas" en Tauri alors qu'il marchait en Chrome rechargé à neuf).
async fn no_cache(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

// app.js fait `new WebSocket(ws://location.host)` → chemin racine "/". On distingue
// un upgrade WS d'une requête HTML normale par la présence de l'en-tête Upgrade
// (comme src/server.ts:909 qui upgrade sur l'en-tête, pas sur un pathname fixe).
async fn root_or_ws(
    ws: Option<WebSocketUpgrade>,
    public: Arc<String>,
    state: Arc<AppState>,
) -> Response {
    match ws {
        Some(ws) => ws.on_upgrade(move |socket| handle_socket(socket, state)),
        None => {
            let html = std::fs::read_to_string(format!("{}/index.html", public.as_str()))
                .unwrap_or_else(|_| "<h1>index.html introuvable</h1>".into());
            Html(html).into_response()
        }
    }
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let os = state.os;
    let steps = &state.plan.steps;

    // 1) plan RÉEL — bundles + steps scannés de bundles/ (fini le dur). Mêmes clés
    // que le front attend (voir app.js render/ws.onmessage).
    let bundles_json: Vec<_> = state
        .plan
        .bundles
        .iter()
        .map(|b| {
            json!({
                "name": b.name, "emoji": b.emoji, "description": b.description,
                "priority": b.priority, "selectable": b.selectable, "posture": b.posture.as_str()
            })
        })
        .collect();
    let steps_json: Vec<_> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "i": i, "name": s.name, "description": s.description, "bundle": s.bundle,
                "canUninstall": s.uninstall.is_some(), "posture": s.posture.as_str(),
                "isConfig": s.is_config, "pin": s.pin
            })
        })
        .collect();
    let plan = json!({
        "type": "plan",
        "bundles": bundles_json,
        "steps": steps_json,
        "selection": {},
        "profiles": [],
        "profileColumns": 2,
        "consent": { "decided": true },
        "build": { "sha": "tauri", "change": "phase1", "builtAt": "dev" }
    });
    let _ = socket.send(Message::Text(plan.to_string())).await;

    // 2) SCAN RÉEL, CONCURRENT — chaque probe lance un login shell (lent : source
    // /etc/profile), donc on ne SÉRIALISE PAS (40s → ~3s). Chaque détection part
    // dans un spawn_blocking ; l'outdated machine-wide (BATCHÉ, une commande) tourne
    // en parallèle. On collecte tout, puis on émet dans l'ordre. "Detect, don't remember".
    let scan_task = tokio::task::spawn_blocking(move || scan_outdated(os));
    let mut probe_tasks = Vec::with_capacity(steps.len());
    for step in steps.iter() {
        let step = step.clone();
        probe_tasks.push(tokio::task::spawn_blocking(move || {
            detect_present_detailed(&step, os)
        }));
    }
    let scan = scan_task.await.unwrap_or_default();
    for (i, task) in probe_tasks.into_iter().enumerate() {
        let p = task.await.unwrap_or_default();
        let sid = steps[i].system_id.clone();
        let state_msg = json!({
            "type": "state", "i": i,
            "present": p.present, "reason": p.reason,
            "version": p.version.unwrap_or_default(), "external": p.external
        });
        let _ = socket.send(Message::Text(state_msg.to_string())).await;
        if p.present == Some(true) {
            if let Some(od) = outdated_for(sid.as_deref(), &scan) {
                let _ = socket
                    .send(Message::Text(
                        json!({ "type": "outdated", "i": i, "current": od.current, "available": od.available }).to_string(),
                    ))
                    .await;
            }
        }
    }

    // 3) state-done — dé-fige l'UI (app.js:1104)
    let _ = socket
        .send(Message::Text(json!({ "type": "state-done" }).to_string()))
        .await;

    // 4) Boucle de messages du front. Deux familles :
    //  - `apply` (bouton Apply global, app.js:499) → apply_diff (re-scan, plan, exécution).
    //  - actions de LIGNE (app.js:598) → un seul bouton sur une ligne envoie
    //    {type:"install"|"uninstall"|"upgrade"|"downgrade", i}. + `retry-step`
    //    {type, i, action}. On exécute do_step sur cet index (downgrade AUTORISÉ ici :
    //    c'est un clic manuel explicite, alors que l'Apply batch l'exclut).
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
            // retry-step : réexécute l'action nommée sur la ligne i (après un échec 403).
            "retry-step" => {
                let i = parsed.get("i").and_then(|v| v.as_u64()).map(|n| n as usize);
                let action = parsed.get("action").and_then(|v| v.as_str());
                if let (Some(i), Some(action)) = (i, action) {
                    row_action(&mut socket, &state, i, action).await;
                }
            }
            _ => {}
        }
    }
}

/// Extrait un tableau d'indices d'un champ JSON ("on"/"off").
fn json_indices(v: &serde_json::Value, key: &str) -> Vec<usize> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect())
        .unwrap_or_default()
}

/// Action de LIGNE : un seul bouton sur une ligne (install/uninstall/upgrade/
/// downgrade). Exécute do_step sur cet index puis émet `done` pour dé-figer l'UI.
/// downgrade est autorisé (clic manuel explicite, le seul chemin destructif hors batch).
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
    do_step(socket, state, i, act, step).await;
    clear_sudo_pw(state).await; // action de ligne finie : effacer le mot de passe caché
    let _ = socket
        .send(Message::Text(json!({ "type": "done" }).to_string()))
        .await;
}

/// Le cœur : applique la décision tri-state contre la réalité machine.
///   on  = indices voulus PRÉSENTS ; off = indices voulus ABSENTS.
/// Re-détecte la présence MAINTENANT (repaint-at-apply : re-constate avant d'agir,
/// ne fait pas confiance au scan de connexion), repeint les pills, demande à la
/// règle PARTAGÉE action_for quoi faire, ordonne par dépendances (topo_sort), émet
/// `apply-plan` (⚠️ ce qui RETIRE le voile "Plotting the gallop…" — raccourci #1 du
/// spike corrigé), puis exécute chaque étape. Port de applyDiff (src/server.ts).
async fn apply_diff(socket: &mut WebSocket, state: &AppState, on: Vec<usize>, off: Vec<usize>) {
    use crate::decision::{action_for, Action, Desired, MachineFacts};
    use crate::deps::{make_index, requires_reason, topo_sort, DepNode};
    use std::collections::HashSet;

    let os = state.os;
    let steps = &state.plan.steps;
    let want_on: HashSet<usize> = on.into_iter().collect();
    let want_off: HashSet<usize> = off.into_iter().collect();

    // Re-scan live CONCURRENT (présence) + outdated batché, comme au connect.
    let scan_task = tokio::task::spawn_blocking(move || scan_outdated(os));
    let mut probe_tasks = Vec::with_capacity(steps.len());
    for step in steps.iter() {
        let step = step.clone();
        probe_tasks.push(tokio::task::spawn_blocking(move || detect_present_detailed(&step, os)));
    }
    let scan = scan_task.await.unwrap_or_default();
    let mut presences = Vec::with_capacity(steps.len());
    for task in probe_tasks {
        presences.push(task.await.unwrap_or_default());
    }

    // État futur par paquet : présent maintenant OU voulu-on, jamais si voulu-off.
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

    // Repeindre les pills AVANT d'agir (repaint-at-apply).
    for (i, p) in presences.iter().enumerate() {
        let reason = requires_reason(&nodes[i], &nodes, &idx).or_else(|| p.reason.clone());
        let _ = socket
            .send(Message::Text(json!({
                "type": "state", "i": i, "present": p.present, "reason": reason,
                "version": p.version.clone().unwrap_or_default(), "external": p.external
            }).to_string()))
            .await;
        if p.present == Some(true) {
            if let Some(od) = outdated_for(steps[i].system_id.as_deref(), &scan) {
                let _ = socket
                    .send(Message::Text(json!({ "type": "outdated", "i": i, "current": od.current, "available": od.available }).to_string()))
                    .await;
            }
        }
    }

    // Action par paquet : désir (on→present, off→absent, ni l'un ni l'autre→auto=None).
    let mut visual_plan: Vec<(usize, Action)> = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let desired = if want_on.contains(&i) {
            Some(Desired::Present)
        } else if want_off.contains(&i) {
            Some(Desired::Absent)
        } else {
            None // auto → jamais touché
        };
        let Some(desired) = desired else { continue };
        let facts = MachineFacts {
            present: presences[i].present == Some(true),
            outdated: outdated_for(step.system_id.as_deref(), &scan).is_some(),
            can_uninstall: step.uninstall.is_some(),
            pin: step.pin.as_deref(),
            installed_version: presences[i].version.as_deref().unwrap_or(""),
        };
        // DOWNGRADE exclu de l'Apply (seul chemin destructif → bouton manuel).
        if let Some(a @ (Action::Install | Action::Uninstall | Action::Upgrade)) =
            action_for(desired, &facts)
        {
            visual_plan.push((i, a));
        }
    }

    // Ordonner par dépendances (requis avant dépendants ; ordre visuel = tie-break).
    let plan = topo_sort(&visual_plan, &nodes, &idx);
    if plan.is_empty() {
        let _ = socket.send(Message::Text(json!({ "type": "done", "nothing": true }).to_string())).await;
        return;
    }
    // Annoncer TOUT le plan en ordre d'exécution → le front entre en focus-mode et
    // RETIRE le voile "Plotting the gallop…" (raccourci #1 corrigé).
    let plan_json: Vec<_> = plan.iter().map(|(i, a)| json!({ "i": i, "action": a.as_str() })).collect();
    let _ = socket.send(Message::Text(json!({ "type": "apply-plan", "plan": plan_json }).to_string())).await;

    for (i, action) in &plan {
        do_step(socket, state, *i, *action, &steps[*i]).await;
    }
    clear_sudo_pw(state).await; // fin d'Apply : le mot de passe caché est effacé (modèle C)
    let _ = socket.send(Message::Text(json!({ "type": "done" }).to_string())).await;
}

/// Port de runInPty (src/server.ts:266-348) : streame la commande dans un pty,
/// scanne le 403 au fil de l'eau, tick le watcher (Windows). Le pty tourne dans un
/// thread bloquant ; canal mpsc → async. RETOURNE (code, forbidden, url) — le
/// verdict (done/step/overlay) est laissé à l'appelant (do_step), comme le TS.
async fn run_in_pty(
    socket: &mut WebSocket,
    state: &AppState,
    i: u32,
    cmdline: &str,
) -> (i32, bool, Option<String>) {
    use base64::{engine::general_purpose::STANDARD, Engine};

    // ptyShell : Windows → powershell + PATH refresh + exit $LASTEXITCODE ; POSIX → bash -lc.
    #[cfg(target_os = "windows")]
    let (program, args): (String, Vec<String>) = (
        "powershell.exe".into(),
        vec![
            "-NoProfile".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-Command".into(),
            format!(
                "$env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+\
                 [Environment]::GetEnvironmentVariable('Path','User'); {}; exit $LASTEXITCODE",
                cmdline
            ),
        ],
    );
    #[cfg(not(target_os = "windows"))]
    let (program, args): (String, Vec<String>) =
        ("/bin/bash".into(), vec!["-lc".into(), cmdline.to_string()]);

    // Watcher (Windows uniquement) : tick 1200ms → cherche une fenêtre d'assistant
    // surgie derrière le panneau, dedup par titre. Racine = notre pid (comme Deno.pid).
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
    // Canal d'ENTRÉE vers le pty (std::sync::mpsc : le thread pty le draine en
    // bloquant). C'est par là que le mot de passe sudo est injecté.
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let code = crate::pty::run(&program, &arg_refs, Some(in_rx), |bytes| {
            let _ = tx.send(bytes.to_vec());
        })
        .unwrap_or(-1);
        let _ = code_tx.send(code);
    });

    // Scan 403 + détection d'un prompt sudo au fil de l'eau. Le prompt "Password:"
    // n'est PAS suivi d'un newline (sudo l'écrit brut), donc on teste la FIN du
    // buffer courant. Une fois répondu, on ne re-demande pas pour ce step.
    let mut buf = String::new();
    let mut forbidden = false;
    let mut sudo_answered = false;
    while let Some(chunk) = rx.recv().await {
        // Toujours accumuler (le prompt sudo peut être découpé sur plusieurs chunks,
        // ou "Password:" arriver dans un morceau distinct → tester le chunk seul rate).
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if !forbidden && crate::forbidden::is403(&buf) {
            forbidden = true;
        }
        let out = json!({ "type": "out", "i": i, "data": STANDARD.encode(&chunk) });
        if socket.send(Message::Text(out.to_string())).await.is_err() {
            return (-1, forbidden, None);
        }
        // Prompt sudo ? sudo écrit "Password:" (ou "Password for X:") SANS newline
        // final — le fix qui compte est de tester le BUFFER ACCUMULÉ (le prompt peut
        // arriver dans un chunk séparé), pas de multiplier les motifs. Répond UNE fois
        // par step ; mot de passe du cache (modèle C) ou demandé au front.
        let tail = buf.trim_end().to_lowercase();
        if !sudo_answered && tail.ends_with(':') && tail.contains("password") {
            if let Some(pw) = obtain_sudo_pw(socket, state, i).await {
                let mut line = pw.into_bytes();
                line.push(b'\n');
                let _ = in_tx.send(line);
                sudo_answered = true;
            }
        }
        // Relayer les signaux du watcher (non bloquant) au fil de l'eau.
        #[cfg(target_os = "windows")]
        while let Ok(wmsg) = watch_rx.try_recv() {
            let _ = socket.send(Message::Text(wmsg.to_string())).await;
        }
    }

    // pty fini : stopper le watcher + wait-clear (comme watcher.stop()).
    #[cfg(target_os = "windows")]
    {
        watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        while let Ok(wmsg) = watch_rx.try_recv() {
            let _ = socket.send(Message::Text(wmsg.to_string())).await;
        }
        let _ = socket
            .send(Message::Text(json!({ "type": "wait-clear", "i": i }).to_string()))
            .await;
    }

    let code = code_rx.await.unwrap_or(-1);
    let url = if forbidden { crate::forbidden::extract_url(&buf) } else { None };
    (code, forbidden, url)
}

/// Obtient le mot de passe sudo — MODÈLE C. Si le cache RAM le porte déjà (saisi
/// plus tôt dans cet Apply), on le réutilise sans redemander. Sinon on demande au
/// front (message `sudo-prompt`), on attend sa réponse (`sudo-pw`), on met en cache.
/// Le mot de passe ne touche JAMAIS le disque/log/journal. Effacé par clear_sudo_pw
/// à la fin de l'Apply. None si le front annule (ferme le modal → `sudo-cancel`).
async fn obtain_sudo_pw(socket: &mut WebSocket, state: &AppState, i: u32) -> Option<String> {
    // 1) cache ?
    {
        let guard = state.sudo_pw.lock().await;
        if let Some(pw) = guard.as_ref() {
            return Some(pw.clone());
        }
    }
    // 2) demander au front (champ masqué).
    let _ = socket
        .send(Message::Text(
            json!({ "type": "sudo-prompt", "i": i }).to_string(),
        ))
        .await;
    // 3) attendre la réponse sur la MÊME socket (run_in_pty en a l'usage exclusif ici).
    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(txt) = msg else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
        match parsed.get("type").and_then(|t| t.as_str()) {
            Some("sudo-pw") => {
                let pw = parsed.get("pw").and_then(|v| v.as_str()).unwrap_or("").to_string();
                *state.sudo_pw.lock().await = Some(pw.clone()); // cache RAM le temps de l'Apply
                return Some(pw);
            }
            Some("sudo-cancel") => return None,
            _ => {} // ignorer tout autre message pendant l'attente du mot de passe
        }
    }
    None
}

/// Efface le mot de passe caché (fin d'Apply / fermeture). RAM remise à None.
async fn clear_sudo_pw(state: &AppState) {
    *state.sudo_pw.lock().await = None;
}

// Codes de sortie bénins (winget : "déjà installé / pas d'upgrade applicable").
// Un exit non-zéro dans cette liste = succès quand même. Port de BENIGN_CODES.
fn benign_code(code: i32) -> bool {
    matches!(code, -1978335189 | -1978335212)
}

/// Exécute UNE étape (install/upgrade/uninstall) : streame la commande, lit l'exit
/// code, émet `step` (running → ok/absent/fail/forbidden) et le verdict 403.
/// Port de doStep (src/server.ts:400-460). Retourne true si l'étape a réussi.
async fn do_step(
    socket: &mut WebSocket,
    state: &AppState,
    i: usize,
    action: crate::decision::Action,
    step: &crate::bundles::Step,
) -> bool {
    use crate::decision::Action;
    let os = state.os;
    let cmd = match action {
        Action::Install => step.install.as_deref(),
        Action::Uninstall => step.uninstall.as_deref(),
        Action::Upgrade => step.upgrade.as_deref(),
        Action::Downgrade => step.downgrade.as_deref(),
    };
    let Some(cmd) = cmd else {
        return false; // pas de commande pour cette route → skip
    };
    let running = match action {
        Action::Uninstall => "uninstalling",
        Action::Upgrade => "upgrading",
        Action::Downgrade => "downgrading",
        Action::Install => "installing",
    };
    let _ = socket
        .send(Message::Text(json!({ "type": "step", "i": i, "status": running }).to_string()))
        .await;

    // Wrapper ptyShell natif (PATH refresh Windows + exit code réel).
    let probe = crate::platform::pty_shell(os, cmd);
    let cmdline = if probe.cmd == "/bin/bash" || probe.cmd == "/bin/sh" {
        // POSIX : run_in_pty gère déjà bash -lc via cfg ; on lui passe la commande nue.
        cmd.to_string()
    } else {
        cmd.to_string()
    };
    let (code, forbidden, url) = run_in_pty(socket, state, i as u32, &cmdline).await;
    let ok = code == 0 || benign_code(code);

    // Ligne de diagnostic dans le terminal de la ligne.
    let line = format!("\r\n\x1b[2m[{}] exit {code} → {}\x1b[0m\r\n", action.as_str(), if ok { "ok" } else { "failed" });
    {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let _ = socket
            .send(Message::Text(json!({ "type": "out", "i": i, "data": STANDARD.encode(line.as_bytes()) }).to_string()))
            .await;
    }

    let blocked = !ok && forbidden;
    let status = if ok {
        if action == Action::Uninstall { "absent" } else { "ok" }
    } else if blocked {
        "forbidden"
    } else {
        "fail"
    };
    let _ = socket
        .send(Message::Text(json!({ "type": "step", "i": i, "status": status }).to_string()))
        .await;
    if blocked {
        let _ = socket
            .send(Message::Text(json!({ "type": "forbidden", "i": i, "url": url }).to_string()))
            .await;
    }
    ok
}
