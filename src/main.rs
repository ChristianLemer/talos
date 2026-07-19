// In release on Windows, GUI subsystem: otherwise Windows opens a black console
// window next to the native window (the binary would start in the "console" subsystem).
// `not(debug_assertions)` → in dev we KEEP the console (logs/panics visible).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_content;
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
mod profiles;
mod pty;
mod selection;
mod server;
mod watch;

fn main() {
    // The HTTP+WS server runs in its own tokio runtime, in the background.
    // The Tauri window (webview) loads http://127.0.0.1:1420 → app.js connects to it
    // via WS as it does today (location.host = 127.0.0.1:1420), unchanged.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // TALOS_PUBLIC = DEV escape hatch: set → we serve the front from this
            // disk folder (live editing without recompiling). Absent (release, .app/.exe
            // launched by Finder/Explorer) → None → assets SEALED in the binary.
            let disk_root = std::env::var_os("TALOS_PUBLIC").map(std::path::PathBuf::from);
            // oneshot(async) → mpsc(sync) bridge: the server fires `bound` when the port
            // is listening, we relay it to the main thread (which has no async runtime).
            let (bound_tx, bound_rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                if bound_rx.await.is_ok() {
                    let _ = ready_tx.send(());
                }
            });
            server::serve(disk_root, Some(bound_tx)).await;
        });
    });
    // Wait until the port is REALLY listening before loading the URL (shortcut #4:
    // no more blind sleep). Guard: if the bind drags on beyond 5s (abnormal),
    // we continue anyway — better a window that retries than a hard block.
    let _ = ready_rx.recv_timeout(std::time::Duration::from_secs(5));
    tauri::Builder::default()
        // Size/position persistence (see Cargo.toml). Registered BEFORE setup: the
        // plugin restores the saved state at the moment the "main" window is created.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // NATIVE window title = "Talos · <change>" (like the old Deno). On
        // path A (WS, no IPC), the front does not drive the native window — the
        // document.title from app.js does NOT propagate to the OS title bar. So we set it
        // here, on the Rust side, with the build stamp (answers "which binary is running?").
        .setup(|app| {
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_title(&format!("Talos · {}", build_info::CHANGE));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error launching Tauri");
}
