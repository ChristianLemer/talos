//! unblock.rs — the page a corporate firewall puts in front of a blocked download, shown
//! INSIDE Talos's window instead of in the user's browser.
//!
//! A 403 on a download (forbidden.rs) used to open the blocked URL in the default browser:
//! the user approved the access there, came back, pressed Retry. It worked, but Talos lost
//! the thread — another application in front, a tab left open, and the installer the proxy
//! then let through downloaded by the browser for nothing. Here the page is a CHILD WEBVIEW
//! laid over a rectangle of Talos's own window (the front's 403 panel says where): no
//! address bar, no tabs, opened and closed by Talos.
//!
//! WHY a child webview and not an iframe: a proxy's block page forbids framing
//! (X-Frame-Options / frame-ancestors), and the front is a page on 127.0.0.1 — an iframe
//! would stay blank. A webview of its own is a top-level page; nothing forbids it.
//!
//! ⚠️ `Window::add_child` is behind Tauri's `unstable` feature (Cargo.toml). Its API may move
//! between Tauri minors; `Cargo.lock` pins it, so the risk is only met at a deliberate update.
//!
//! ⚠️ WINDOWS ONLY, by measurement. On Linux Tauri packs a child webview into the window's
//! GTK box instead of placing it (tauri-runtime-wry `build_gtk(vbox)`): MEASURED under Xvfb,
//! the page took the lower half and squeezed the panel into the upper one, whatever rectangle
//! was asked; WebKitGTK also never reported the download. macOS places it like Windows on
//! paper, but no Mac measured it. Elsewhere `open` says no — a 403 happens on Windows — and the
//! panel offers the browser without opening the page inside.
//!
//! The approval only helps winget if the proxy remembers it per user or per machine (the
//! child uses the system proxy and Windows authentication, like winget). A proxy that keys
//! it on a browser cookie would want the browser: the panel keeps "open in my browser".
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewBuilder};

static APP: OnceLock<AppHandle> = OnceLock::new();
static VIEW: Mutex<Option<Webview>> = Mutex::new(None);
/// What happened in the page shown NOW, for the front (`next_event`). Opening or closing a page
/// empties it: an event left by the previous page must never act on a block nobody approved.
static EVENTS: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());
/// Rung on every `push`. A ring nobody awaited is kept, and only counts if EVENTS has something.
static RING: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

/// An event of the page inside Talos, told to the front.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// The page let its download through: the approval worked, the front counts down.
    Released,
    /// "Retry now", pressed in Talos's page.
    RetryNow,
    /// "Cancel", pressed in Talos's page: no automatic retry.
    Cancel,
}

impl Event {
    /// The word on the wire (`unblock-event`) and on `POST /unblock/act` (the two pressable ones).
    pub fn word(self) -> &'static str {
        match self {
            Event::Released => "released",
            Event::RetryNow => "retry",
            Event::Cancel => "cancel",
        }
    }

    /// What Talos's page may ask for — never `released`, which only the download can say.
    pub fn from_page(word: &str) -> Option<Event> {
        match word.trim() {
            "retry" => Some(Event::RetryNow),
            "cancel" => Some(Event::Cancel),
            _ => None,
        }
    }
}

/// The label of the one child webview. One at a time: a second block replaces the first.
const LABEL: &str = "unblock";

/// Where the view goes when the page lets the download through: the approval worked, and the
/// installer must not be downloaded by a webview — Talos fetches it itself on Retry.
const LET_THROUGH: &str = "http://127.0.0.1:1420/unblocked.html";
/// The same page when no retry is left for this block: it says so, with no count and no button.
const LET_THROUGH_NO_RETRY: &str = "http://127.0.0.1:1420/unblocked.html?left=0";

/// Whether the block shown may still be retried. The 403 pause knows (it caps the retries) and
/// says so for as long as it holds (`RetryLeft`); the page let through must not promise a retry
/// that will not come (review).
static RETRY_LEFT: AtomicBool = AtomicBool::new(true);

/// Says, for as long as it lives, whether the held block may still be retried. Dropped when the
/// pause ends, by whichever way, and the default — a retry is possible — comes back.
pub struct RetryLeft;

impl RetryLeft {
    pub fn hold(left: bool) -> RetryLeft {
        RETRY_LEFT.store(left, Ordering::SeqCst);
        RetryLeft
    }
}

impl Drop for RetryLeft {
    fn drop(&mut self) {
        RETRY_LEFT.store(true, Ordering::SeqCst);
    }
}

/// A rectangle of the main window, in CSS (= logical) pixels from its top-left corner — what
/// `getBoundingClientRect` gives the front.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The rectangle a front message carries (`rect: {x, y, w, h}`), or None when it is not one
/// a view can take: a missing or non-finite number, an empty size.
pub fn rect_of(v: &serde_json::Value) -> Option<Rect> {
    let r = v.get("rect")?;
    let n = |k: &str| r.get(k)?.as_f64().filter(|f| f.is_finite());
    let rect = Rect {
        x: n("x")?,
        y: n("y")?,
        w: n("w")?,
        h: n("h")?,
    };
    (rect.w >= 1.0 && rect.h >= 1.0).then_some(rect)
}

/// The URL the view may open: http or https only. The address comes from a manager's
/// output (`forbidden::extract_url`), and a webview must not be pointed at `file:` or
/// `javascript:` by whatever a tool printed.
pub fn page_url(url: &str) -> Result<tauri::Url, String> {
    let parsed: tauri::Url = url.parse().map_err(|e| format!("not a URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(format!("refused scheme `{other}:`")),
    }
}

/// Can a page open inside Talos here? Windows, and a Talos window in this process. The front
/// then opens it the moment a download is blocked, without a step to click first.
pub fn available() -> bool {
    crate::platform::current_os() == crate::platform::Os::Windows && APP.get().is_some()
}

fn events() -> std::sync::MutexGuard<'static, VecDeque<Event>> {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Queues an event of the page shown now. Talos's own page pushes only while one is shown.
pub fn push(event: Event) {
    events().push_back(event);
    RING.notify_one();
}

/// Queues an event of Talos's page, if a page is shown: under the view's lock, so a close
/// racing the click cannot leave the event behind it (review).
pub fn push_if_shown(event: Event) -> bool {
    let view = VIEW.lock().unwrap_or_else(|e| e.into_inner());
    if view.is_none() {
        return false;
    }
    push(event);
    true
}

/// The next event of the page shown now. A ring kept from a page since closed finds the queue
/// empty and waits again.
pub async fn next_event() -> Event {
    loop {
        if let Some(event) = events().pop_front() {
            return event;
        }
        RING.notified().await;
    }
}

/// Called once, from Tauri's setup: the server runs on its own thread and has no other way
/// to reach the window.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// Opens the blocked page over `rect` of the main window, replacing any page already shown.
/// `Err(reason)`: no Talos window here (the dev server in a browser, a headless run) or the
/// webview could not be made — the front then falls back to the browser.
pub fn open(url: &str, rect: Rect) -> Result<(), String> {
    let url = page_url(url)?;
    if crate::platform::current_os() != crate::platform::Os::Windows {
        return Err("a page inside the window is measured on Windows only".into());
    }
    let app = APP
        .get()
        .ok_or("no Talos window in this process (a browser on the dev server?)")?;
    let window = app.get_window("main").ok_or("the main window is gone")?;
    close();
    let builder = WebviewBuilder::new(LABEL, tauri::WebviewUrl::External(url))
        .on_download(|view, event| {
            if let tauri::webview::DownloadEvent::Requested { url, .. } = event {
                println!("[unblock] the page let {url} through — not downloaded here");
                let page = if RETRY_LEFT.load(Ordering::SeqCst) {
                    LET_THROUGH
                } else {
                    LET_THROUGH_NO_RETRY
                };
                if let Ok(done) = page.parse() {
                    let _ = view.navigate(done);
                }
                push(Event::Released);
            }
            false
        })
        // A link or `window.open` of the block page (an approval step, a sign-in) stays in
        // the panel instead of opening a window Talos would never close. Found by label, not
        // through VIEW: this runs on the main thread, which must never wait on that lock.
        .on_new_window(|url, _| {
            if let Some(view) = APP.get().and_then(|app| app.get_webview(LABEL)) {
                let _ = view.navigate(url);
            }
            tauri::webview::NewWindowResponse::Deny
        });
    let view = window
        .add_child(
            builder,
            LogicalPosition::new(rect.x, rect.y),
            LogicalSize::new(rect.w, rect.h),
        )
        .map_err(|e| format!("could not open the page in Talos: {e}"))?;
    *VIEW.lock().unwrap_or_else(|e| e.into_inner()) = Some(view);
    Ok(())
}

/// Follows the panel when the window is resized. Nothing shown → nothing to do.
pub fn place(rect: Rect) {
    if let Some(view) = VIEW.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = view.set_position(LogicalPosition::new(rect.x, rect.y));
        let _ = view.set_size(LogicalSize::new(rect.w, rect.h));
    }
}

/// Closes the page if one is shown. Called by the front when its panel goes, by the server
/// when the user decides (retry, skip, stop) and when a connection starts — a reloaded
/// front must not find a page from the previous one still laid over it.
pub fn close() {
    let mut view = VIEW.lock().unwrap_or_else(|e| e.into_inner());
    events().clear();
    if let Some(view) = view.take() {
        let _ = view.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_rect_is_four_finite_numbers_and_a_real_size() {
        let msg = json!({ "rect": { "x": 24, "y": 120.5, "w": 900, "h": 400 } });
        assert_eq!(
            rect_of(&msg),
            Some(Rect {
                x: 24.0,
                y: 120.5,
                w: 900.0,
                h: 400.0
            })
        );
        assert_eq!(rect_of(&json!({})), None);
        assert_eq!(
            rect_of(&json!({ "rect": { "x": 0, "y": 0, "w": 0, "h": 400 } })),
            None
        );
        assert_eq!(
            rect_of(&json!({ "rect": { "x": "a", "y": 0, "w": 10, "h": 10 } })),
            None
        );
    }

    #[test]
    fn only_a_web_page_is_opened() {
        assert!(page_url("https://proxy.example/block?u=x").is_ok());
        assert!(page_url("http://example.com/pkg.exe").is_ok());
        for bad in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "not a url",
        ] {
            assert!(page_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn without_a_talos_window_the_page_is_not_opened_here() {
        // Tests run no Tauri app: `open` must say so, never panic, so the front falls back.
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        assert!(open("https://example.com", rect).is_err());
        close();
        place(rect);
    }

    #[test]
    fn an_event_left_by_a_closed_page_acts_on_nothing() {
        // Review: a Notify keeps a ring nobody awaited; the next 403 would have counted down
        // and retried a row nobody approved. Closing empties the queue; only a fresh event counts.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let soon = std::time::Duration::from_millis(50);
        rt.block_on(async {
            push(Event::Released);
            close();
            let stale = tokio::time::timeout(soon, next_event());
            assert!(stale.await.is_err(), "a cleared event must not resolve");
            push(Event::Released);
            push(Event::Cancel);
            assert_eq!(
                tokio::time::timeout(soon, next_event()).await,
                Ok(Event::Released)
            );
            assert_eq!(
                tokio::time::timeout(soon, next_event()).await,
                Ok(Event::Cancel)
            );
        });
    }

    #[test]
    fn the_page_may_ask_for_a_retry_or_a_cancel_only() {
        assert_eq!(Event::from_page("retry"), Some(Event::RetryNow));
        assert_eq!(Event::from_page("cancel\n"), Some(Event::Cancel));
        assert_eq!(
            Event::from_page("released"),
            None,
            "only the download says that"
        );
        assert_eq!(Event::from_page("quit"), None);
    }
}
