// In release on Windows, GUI subsystem: otherwise Windows opens a black console
// window next to the native window (the binary would start in the "console" subsystem).
// `not(debug_assertions)` → in dev we KEEP the console (logs/panics visible).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_content;
mod assets;
mod behaviour;
mod behaviour_io;
mod build_info;
mod bundles;
mod catalog;
mod check;
mod consent;
mod content;
mod decision;
mod deps;
mod detect;
mod distribution;
mod forbidden;
mod machine;
mod managers;
mod outdated;
mod plan_facts;
mod platform;
mod profiles;
mod pty;
mod relocate;
mod selection;
mod server;
mod tail;
mod timings;
mod vscode;
mod watch;

fn main() {
    // `Talos --check [catalog_dir bundles_dir]`: read the content and exit, no window.
    // What an integrator runs before dropping a kit on the share, and what a content
    // repo's pipeline runs on every push. Exit code 0 = clean, 1 = findings. ⚠️ On Windows
    // the release exe is a GUI-subsystem program: its stdout reaches a REDIRECTED handle
    // (`Talos.exe --check > check.txt`) but not the bare console, so the exit code is the
    // contract there and the text is a bonus.
    //
    // `--kit <dir>` is taken out first: it is what a relocated engine is started with, and it
    // stands for the folder beside the exe from here on (relocate.rs, platform::exe_sibling) —
    // `--check` included. Passed by hand, it also means "run here": no relocation.
    let launch = relocate::split_kit(&std::env::args().skip(1).collect::<Vec<_>>());
    if let Some(kit) = &launch.kit {
        platform::set_kit(kit.clone());
    }
    let args = launch.rest.clone();
    if args.first().map(String::as_str) == Some("--check") {
        // With no folders named, the kit's own content — through its archive when it has one
        // (content.rs). An archive that is unusable or missing FAILS the check rather than
        // checking the previous extraction: after composing a kit, this is what proves the
        // archive, and passing on stale content would prove nothing.
        let (catalog_dir, bundles_dir) = match (args.get(1), args.get(2)) {
            (Some(c), Some(b)) => (std::path::PathBuf::from(c), std::path::PathBuf::from(b)),
            _ => {
                let content = content::resolve();
                if let Some(problem) = content.problem() {
                    // `error: content archive:` is a STABLE prefix: get-talos tells this cause
                    // from the others by it, whatever the sentence after says.
                    println!("error: content archive: {problem}");
                    std::process::exit(1);
                }
                (content.catalog, content.bundles)
            }
        };
        let report = check::run(&catalog_dir, &bundles_dir);
        print!("{}", report.render());
        std::process::exit(if report.ok() { 0 } else { 1 });
    }
    // A release on Windows started from the share hands over to a local copy and leaves, so
    // the share's exe is never held open for a whole session. Every other case runs here.
    match relocate::launch(platform::current_os(), &launch) {
        relocate::Launched::HandedOver => return,
        relocate::Launched::Here(Some(kit)) => platform::set_kit(kit),
        relocate::Launched::Here(None) => {}
    }
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
        // NATIVE window title = "<distribution> · Talos <tag>", or "Talos · <tag>" when the
        // content has no `distribution.yaml`. On path A (WS, no IPC), the front does not
        // drive the native window — the document.title from app.js does NOT propagate to
        // the OS title bar. So we set it here, on the Rust side, with the same rule the
        // front applies (`distribution::window_title`), from the build stamp (answers
        // "which binary is really running?") and the file beside the catalog (answers
        // "which content?"). Read once, here: a name edited mid-session reaches the
        // document title on Refresh, the native one at the next launch.
        //
        // The TAG is the human-facing version, and locally it is the describe string, so
        // it already says "ahead of the tag by n" and "built dirty" — nothing else from
        // the stamp belongs in a title. It used to trail a jj change id, which read
        // "Talos · git" on every released build: CI builds from a plain git clone, so the
        // title printed the name of the fallback instead of a version.
        .setup(|app| {
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let (catalog_dir, _) = platform::content_dirs();
                let dist = distribution::read_beside(&catalog_dir);
                let _ = win.set_title(&distribution::window_title(dist.as_ref(), build_info::TAG));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error launching Tauri");
}
