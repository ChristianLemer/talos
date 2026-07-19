mod assets;
mod bundles;
mod decision;
mod deps;
mod detect;
mod forbidden;
mod managers;
mod outdated;
mod platform;
mod pty;
mod server;
mod watch;

fn main() {
    // Le serveur HTTP+WS tourne dans son propre runtime tokio, en tâche de fond.
    // La fenêtre Tauri (webview) charge http://127.0.0.1:1420 → app.js s'y connecte
    // en WS comme aujourd'hui (location.host = 127.0.0.1:1420), inchangé.
    std::thread::spawn(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // TALOS_PUBLIC = échappatoire DEV : posé → on sert le front depuis ce
            // dossier disque (édition live sans recompiler). Absent (release, .app/.exe
            // lancé par Finder/Explorer) → None → assets SCELLÉS dans le binaire.
            let disk_root = std::env::var_os("TALOS_PUBLIC").map(std::path::PathBuf::from);
            server::serve(disk_root).await;
        });
    });
    // Laisser au serveur le temps d'écouter avant que le webview charge l'URL.
    std::thread::sleep(std::time::Duration::from_millis(500));
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Tauri");
}
