use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::json;
use std::sync::Arc;
use tower_http::services::ServeDir;

/// Démarre le serveur HTTP+WS sur 127.0.0.1:1420.
/// - `/`         → sert index.html (ou upgrade WS si l'en-tête Upgrade est présent)
/// - autres      → ServeDir sur public/ (app.js, vendor/, css…)
pub async fn serve(public_dir: String) {
    let public = Arc::new(public_dir);
    let public_for_root = public.clone();
    let app = Router::new()
        .route(
            "/",
            get(move |ws: Option<WebSocketUpgrade>| {
                let public = public_for_root.clone();
                async move { root_or_ws(ws, public).await }
            }),
        )
        .fallback_service(ServeDir::new(public.as_str().to_string()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:1420")
        .await
        .expect("bind 127.0.0.1:1420 failed");
    axum::serve(listener, app).await.expect("axum serve failed");
}

// app.js fait `new WebSocket(ws://location.host)` → chemin racine "/". On distingue
// un upgrade WS d'une requête HTML normale par la présence de l'en-tête Upgrade
// (comme src/server.ts:909 qui upgrade sur l'en-tête, pas sur un pathname fixe).
async fn root_or_ws(ws: Option<WebSocketUpgrade>, public: Arc<String>) -> Response {
    match ws {
        Some(ws) => ws.on_upgrade(handle_socket),
        None => {
            let html = std::fs::read_to_string(format!("{}/index.html", public.as_str()))
                .unwrap_or_else(|_| "<h1>index.html introuvable</h1>".into());
            Html(html).into_response()
        }
    }
}

async fn handle_socket(mut socket: WebSocket) {
    // 1) plan — données MINIMALES en dur (le spike prouve le rendu, pas les vraies données)
    let plan = json!({
        "type": "plan",
        "bundles": [{ "name": "spike", "description": "démo Tauri", "posture": "opt-in" }],
        "steps": [
            { "i": 0, "name": "brew", "description": "gestionnaire", "bundle": "spike",
              "canUninstall": false, "posture": "opt-in", "isConfig": false, "pin": null },
            { "i": 1, "name": "node", "description": "runtime", "bundle": "spike",
              "canUninstall": false, "posture": "opt-in", "isConfig": false, "pin": null }
        ],
        "selection": {},
        "profiles": [],
        "profileColumns": 2,
        "consent": { "decided": true },
        "build": { "sha": "spike", "change": "spike", "builtAt": "spike" }
    });
    let _ = socket.send(Message::Text(plan.to_string())).await;

    // 2) un state par step (présence en dur pour la démo)
    for (i, present) in [(0, true), (1, false)] {
        let state = json!({ "type": "state", "i": i, "present": present,
                            "reason": null, "version": if present {"1.0"} else {""}, "external": false });
        let _ = socket.send(Message::Text(state.to_string())).await;
    }

    // 3) state-done — dé-fige l'UI (app.js:1104)
    let _ = socket
        .send(Message::Text(json!({ "type": "state-done" }).to_string()))
        .await;

    // 4) Attendre que le front demande l'install (clic Apply → app.js:499 envoie
    // {type:"apply", on, off, scope}). On matche le type et on ignore on/off/scope :
    // le spike installe un paquet fixe pour exercer le CHEMIN, pas le vrai diff.
    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(txt) = msg else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
        if parsed.get("type").and_then(|t| t.as_str()) != Some("apply") {
            continue;
        }
        run_in_pty(&mut socket, 0, install_cmdline()).await;
    }
}

/// La commande d'install de démo, par plateforme (le vrai Talos passe la commande
/// du manager). Windows : un vrai `winget install` (long, UAC, fenêtre d'assistant).
/// Mac/Linux : commande inoffensive pour que le CHEMIN compile et streame (le
/// comportement réel — UAC, watcher — ne se teste que sur Windows, Task 5).
fn install_cmdline() -> &'static str {
    if cfg!(target_os = "windows") {
        "winget install --id 7zip.7zip -e --source winget \
         --accept-source-agreements --accept-package-agreements"
    } else {
        // Mac/Linux : simule un install qui prend un peu de temps et streame.
        "echo 'spike: simulated install (real winget path is Windows-only)'; sleep 1; echo done"
    }
}

/// Port minimal de runInPty (src/server.ts:266-348) : streame la commande dans un
/// pty, scanne le 403 au fil de l'eau, tick le watcher (Windows), renvoie l'exit
/// code via un message `done`. Le pty tourne dans un thread bloquant ; canal mpsc → async.
async fn run_in_pty(socket: &mut WebSocket, i: u32, cmdline: &str) {
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
    std::thread::spawn(move || {
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let code = crate::pty::run(&program, &arg_refs, |bytes| {
            let _ = tx.send(bytes.to_vec());
        })
        .unwrap_or(-1);
        let _ = code_tx.send(code);
    });

    // Scan 403 sur un buffer courant ; verdict laissé à la fin (comme runInPty).
    let mut buf = String::new();
    let mut forbidden = false;
    while let Some(chunk) = rx.recv().await {
        if !forbidden {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            if crate::forbidden::is403(&buf) {
                forbidden = true;
            }
        }
        let out = json!({ "type": "out", "i": i, "data": STANDARD.encode(&chunk) });
        if socket.send(Message::Text(out.to_string())).await.is_err() {
            return;
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
    // Verdict 403 seulement si l'install a ÉCHOUÉ (comme doStep) : sinon un 403
    // récupéré en interne par winget flasherait à tort la bannière.
    if forbidden && code != 0 {
        let url = crate::forbidden::extract_url(&buf);
        let _ = socket
            .send(Message::Text(
                json!({ "type": "overlay", "i": i, "forbidden": true, "url": url }).to_string(),
            ))
            .await;
    }
    let _ = socket
        .send(Message::Text(json!({ "type": "done", "i": i, "code": code }).to_string()))
        .await;
}
