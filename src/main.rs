mod assets;
mod build_info;
mod bundles;
mod consent;
mod decision;
mod deps;
mod detect;
mod forbidden;
mod managers;
mod outdated;
mod platform;
mod pty;
mod selection;
mod server;
mod watch;

fn main() {
    // Le serveur HTTP+WS tourne dans son propre runtime tokio, en tâche de fond.
    // La fenêtre Tauri (webview) charge http://127.0.0.1:1420 → app.js s'y connecte
    // en WS comme aujourd'hui (location.host = 127.0.0.1:1420), inchangé.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // TALOS_PUBLIC = échappatoire DEV : posé → on sert le front depuis ce
            // dossier disque (édition live sans recompiler). Absent (release, .app/.exe
            // lancé par Finder/Explorer) → None → assets SCELLÉS dans le binaire.
            let disk_root = std::env::var_os("TALOS_PUBLIC").map(std::path::PathBuf::from);
            // Pont oneshot(async) → mpsc(sync) : le serveur tire `bound` quand le port
            // écoute, on relaie au thread principal (qui n'a pas de runtime async).
            let (bound_tx, bound_rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                if bound_rx.await.is_ok() {
                    let _ = ready_tx.send(());
                }
            });
            server::serve(disk_root, Some(bound_tx)).await;
        });
    });
    // Attendre que le port écoute VRAIMENT avant de charger l'URL (raccourci #4 :
    // fini le sleep aveugle). Garde-fou : si le bind traîne au-delà de 5s (anormal),
    // on continue quand même — mieux vaut une fenêtre qui retente qu'un blocage dur.
    let _ = ready_rx.recv_timeout(std::time::Duration::from_secs(5));
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Tauri");
}
