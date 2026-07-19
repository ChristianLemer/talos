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
            let public = std::env::var("TALOS_PUBLIC").unwrap_or_else(|_| "public".into());
            server::serve(public).await;
        });
    });
    // Laisser au serveur le temps d'écouter avant que le webview charge l'URL.
    std::thread::sleep(std::time::Duration::from_millis(500));
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Tauri");
}
