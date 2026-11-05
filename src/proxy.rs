//! proxy.rs — which road the OS would give one URL: direct, or a proxy. The download probe
//! (passage.rs) asks this at every hop, so it takes the application's road, not its own.
//!
//! WHY: a corporate client agent often runs in proxy auto-config mode. The OS is set to a PAC
//! script served on the machine, the script sends the hosts it filters to a local proxy, and
//! what does not follow the script leaves unfiltered. The application (Chromium's stack) and the
//! browser follow the script; an HTTP client that reads only a FIXED proxy goes direct and gets
//! the file the application is refused. Measured on a Mac: direct `206`, through the script's
//! proxy `403 text/html` — the same file, the same second.
//!
//! So the OS evaluates its own script, with no JavaScript engine in Talos:
//! - macOS: `CFNetworkCopyProxiesForURL` on the system settings; a PAC entry is run with
//!   `CFNetworkExecuteProxyAutoConfigurationURL` (or `…Script`). A fixed proxy and its
//!   exceptions come out of the same call.
//! - Windows: `WinHttpGetIEProxyConfigForCurrentUser`, then `WinHttpGetProxyForUrl` when the
//!   user's settings name a script or ask for WPAD. With neither, the fixed `ProxyServer` stays
//!   reqwest's own reading (`Route::System`), as before.
//! - Linux: `Route::System` (the environment's proxy), as before. Unmeasured against a PAC.
//!
//! The first entry the OS gives is the road (a browser falls back to the next one only when the
//! first does not answer). A lookup that fails is an `Err`, said on the row: the probe never
//! claims a road it did not get.
use reqwest::Url;

/// The road for one request.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// No proxy: the OS (or its script) says `DIRECT`.
    Direct,
    /// Through this proxy (`http://host:port`), as the OS or its script says.
    Proxy(String),
    /// No script to follow: the HTTP client's own reading of the OS's fixed proxy setting.
    System,
}

impl std::fmt::Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Route::Direct => write!(f, "direct"),
            Route::Proxy(p) => write!(f, "through {p}"),
            Route::System => write!(f, "the proxy setting (fixed, or the environment's), if any"),
        }
    }
}

/// Asks the OS which road `url` takes. Blocking (a script may be fetched and run): call it off
/// the async threads.
pub fn route_for(url: &Url) -> Result<Route, String> {
    os::route_for(url)
}

/// One answer of the OS, before it is a road.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Entry {
    Direct,
    /// An HTTP or HTTPS proxy: both are spoken to in plain HTTP (`CONNECT` for an https URL).
    Http(String, u16),
    Socks(String, u16),
    /// Something else this probe cannot take (a nested script, an unknown kind).
    Other(String),
}

/// The road from the OS's ordered answer: the first entry. An empty answer is direct.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn pick(entries: &[Entry]) -> Result<Route, String> {
    match entries.first() {
        None | Some(Entry::Direct) => Ok(Route::Direct),
        Some(Entry::Http(host, port)) => Ok(Route::Proxy(format!("http://{host}:{port}"))),
        Some(Entry::Socks(host, port)) => Err(format!(
            "the system sends it to a SOCKS proxy ({host}:{port}), which the probe does not speak"
        )),
        Some(Entry::Other(kind)) => Err(format!("the system answered a proxy of kind {kind}")),
    }
}

/// WinHTTP's proxy list (`host:port`, several separated by `;` or spaces, possibly
/// `scheme=host:port` per scheme) as a road: the `https=` entry when there is one, since the
/// probed addresses are https, else the first entry for every scheme, else the `http=` one. An
/// empty list, or `DIRECT`, is direct. A list that names only another scheme (`socks=h:p`, what
/// a script's `SOCKS` becomes) is an error: the application goes through it, so direct would
/// be another road.
#[cfg_attr(not(windows), allow(dead_code))]
fn from_winhttp_list(list: &str) -> Result<Route, String> {
    let entries: Vec<&str> = list
        .split(|c: char| c == ';' || c.is_whitespace())
        .filter(|e| !e.is_empty())
        .collect();
    let chosen = entries
        .iter()
        .find_map(|e| e.strip_prefix("https="))
        .or_else(|| entries.iter().find(|e| !e.contains('=')).copied())
        .or_else(|| entries.iter().find_map(|e| e.strip_prefix("http=")));
    match chosen {
        None if entries.is_empty() => Ok(Route::Direct),
        None => Err(format!(
            "the system sends it to `{list}`, a proxy the probe does not speak"
        )),
        Some(p) if p.eq_ignore_ascii_case("direct") => Ok(Route::Direct),
        Some(p) if p.contains("://") => Ok(Route::Proxy(p.to_string())),
        Some(p) => Ok(Route::Proxy(format!("http://{p}"))),
    }
}

#[cfg(target_os = "linux")]
mod os {
    use super::Route;
    use reqwest::Url;

    /// Unchanged: the environment's proxy, read by the HTTP client.
    pub fn route_for(_url: &Url) -> Result<Route, String> {
        Ok(Route::System)
    }
}

#[cfg(windows)]
mod os {
    use super::{from_winhttp_list, Route};
    use reqwest::Url;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{GetLastError, GlobalFree};
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl,
        WinHttpOpen, ERROR_WINHTTP_AUTODETECTION_FAILED, WINHTTP_ACCESS_TYPE_NO_PROXY,
        WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL, WINHTTP_AUTOPROXY_OPTIONS,
        WINHTTP_AUTO_DETECT_TYPE_DHCP, WINHTTP_AUTO_DETECT_TYPE_DNS_A,
        WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WINHTTP_PROXY_INFO,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Reads a WinHTTP-allocated string and frees it.
    unsafe fn take(p: *mut u16) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        GlobalFree(p.cast());
        Some(s)
    }

    pub fn route_for(url: &Url) -> Result<Route, String> {
        // The user's settings: the ones Edge and Chromium-based applications follow.
        let mut ie: WINHTTP_CURRENT_USER_IE_PROXY_CONFIG = unsafe { std::mem::zeroed() };
        if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut ie) } == 0 {
            return Err(format!(
                "the user's proxy settings could not be read (error {})",
                unsafe { GetLastError() }
            ));
        }
        let auto_detect = ie.fAutoDetect != 0;
        let script = unsafe { take(ie.lpszAutoConfigUrl) }.filter(|s| !s.trim().is_empty());
        unsafe {
            take(ie.lpszProxy);
            take(ie.lpszProxyBypass);
        }
        if !auto_detect && script.is_none() {
            return Ok(Route::System);
        }
        let agent = wide("Talos");
        let session = unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                null(),
                null(),
                0,
            )
        };
        if session.is_null() {
            return Err(format!("no WinHTTP session (error {})", unsafe {
                GetLastError()
            }));
        }
        let target = wide(url.as_str());
        // Err: WinHTTP's error code; Ok: the list read as a road (or why it is not one).
        let ask = |options: &mut WINHTTP_AUTOPROXY_OPTIONS| -> Result<Result<Route, String>, u32> {
            let mut info: WINHTTP_PROXY_INFO = unsafe { std::mem::zeroed() };
            if unsafe { WinHttpGetProxyForUrl(session, target.as_ptr(), options, &mut info) } == 0 {
                return Err(unsafe { GetLastError() });
            }
            let list = unsafe { take(info.lpszProxy) }.unwrap_or_default();
            unsafe { take(info.lpszProxyBypass) };
            Ok(match info.dwAccessType {
                WINHTTP_ACCESS_TYPE_NO_PROXY => Ok(Route::Direct),
                _ => from_winhttp_list(&list),
            })
        };
        let mut result = Ok(Route::System);
        // WPAD first, as Windows does; a network with no WPAD is not a failure — the script
        // named in the settings, or else the fixed setting, decides.
        if auto_detect {
            let mut options = WINHTTP_AUTOPROXY_OPTIONS {
                dwFlags: WINHTTP_AUTOPROXY_AUTO_DETECT,
                dwAutoDetectFlags: WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A,
                lpszAutoConfigUrl: null(),
                lpvReserved: null_mut(),
                dwReserved: 0,
                fAutoLogonIfChallenged: 1,
            };
            match ask(&mut options) {
                Ok(route) => {
                    unsafe { WinHttpCloseHandle(session) };
                    return route;
                }
                Err(ERROR_WINHTTP_AUTODETECTION_FAILED) => {}
                Err(code) => result = Err(format!("WPAD lookup failed (error {code})")),
            }
        }
        if let Some(script) = script {
            let script_w = wide(&script);
            let mut options = WINHTTP_AUTOPROXY_OPTIONS {
                dwFlags: WINHTTP_AUTOPROXY_CONFIG_URL,
                dwAutoDetectFlags: 0,
                lpszAutoConfigUrl: script_w.as_ptr(),
                lpvReserved: null_mut(),
                dwReserved: 0,
                fAutoLogonIfChallenged: 1,
            };
            // A script the settings name and that cannot be run is a failure: the application
            // follows that script, and the probe does not know where it would go.
            result = ask(&mut options)
                .map_err(|code| {
                    format!("the proxy script {script} could not be run (error {code})")
                })
                .and_then(|route| route);
        }
        unsafe { WinHttpCloseHandle(session) };
        result
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::{pick, Entry, Route};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::string::CFString;
    use core_foundation::url::CFURL;
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation_sys::base::{CFEqual, CFGetTypeID, CFIndex, CFRelease, CFTypeRef};
    use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
    use core_foundation_sys::error::CFErrorRef;
    use core_foundation_sys::number::{kCFNumberSInt32Type, CFNumberGetTypeID, CFNumberGetValue};
    use core_foundation_sys::runloop::{
        kCFRunLoopRunHandledSource, CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRemoveSource,
        CFRunLoopRunInMode, CFRunLoopSourceInvalidate, CFRunLoopSourceRef, CFRunLoopStop,
    };
    use core_foundation_sys::string::{CFStringGetTypeID, CFStringRef};
    use core_foundation_sys::url::{CFURLGetTypeID, CFURLRef};
    use reqwest::Url;
    use std::ffi::c_void;

    type ResultCallback =
        extern "C" fn(client: *mut c_void, proxies: CFArrayRef, error: CFErrorRef);

    /// `CFStreamClientContext`, with the callbacks it allows to be NULL.
    #[repr(C)]
    struct ClientContext {
        version: CFIndex,
        info: *mut c_void,
        retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
        release: Option<extern "C" fn(*const c_void)>,
        copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
    }

    #[link(name = "CFNetwork", kind = "framework")]
    extern "C" {
        fn CFNetworkCopySystemProxySettings() -> CFDictionaryRef;
        fn CFNetworkCopyProxiesForURL(url: CFURLRef, settings: CFDictionaryRef) -> CFArrayRef;
        fn CFNetworkExecuteProxyAutoConfigurationURL(
            script_url: CFURLRef,
            target: CFURLRef,
            cb: ResultCallback,
            ctx: *mut ClientContext,
        ) -> CFRunLoopSourceRef;
        fn CFNetworkExecuteProxyAutoConfigurationScript(
            script: CFStringRef,
            target: CFURLRef,
            cb: ResultCallback,
            ctx: *mut ClientContext,
        ) -> CFRunLoopSourceRef;
        static kCFProxyTypeKey: CFStringRef;
        static kCFProxyHostNameKey: CFStringRef;
        static kCFProxyPortNumberKey: CFStringRef;
        static kCFProxyAutoConfigurationURLKey: CFStringRef;
        static kCFProxyAutoConfigurationJavaScriptKey: CFStringRef;
        static kCFProxyTypeNone: CFStringRef;
        static kCFProxyTypeHTTP: CFStringRef;
        static kCFProxyTypeHTTPS: CFStringRef;
        static kCFProxyTypeSOCKS: CFStringRef;
        static kCFProxyTypeAutoConfigurationURL: CFStringRef;
        static kCFProxyTypeAutoConfigurationJavaScript: CFStringRef;
    }

    /// One entry as the OS gives it, a script still to run included.
    enum Raw {
        Entry(Entry),
        Script(CFURLRef),
        ScriptText(CFStringRef),
    }

    unsafe fn get(dict: CFDictionaryRef, key: CFStringRef) -> CFTypeRef {
        CFDictionaryGetValue(dict, key.cast())
    }

    unsafe fn string(v: CFTypeRef) -> Option<String> {
        (!v.is_null() && CFGetTypeID(v) == CFStringGetTypeID())
            .then(|| CFString::wrap_under_get_rule(v as CFStringRef).to_string())
    }

    unsafe fn read(dict: CFDictionaryRef) -> Raw {
        let kind = get(dict, kCFProxyTypeKey);
        let is = |k: CFStringRef| !kind.is_null() && CFEqual(kind, k.cast()) != 0;
        let host = || string(get(dict, kCFProxyHostNameKey)).unwrap_or_default();
        let port = || {
            let n = get(dict, kCFProxyPortNumberKey);
            let mut p: i32 = 0;
            if !n.is_null() && CFGetTypeID(n) == CFNumberGetTypeID() {
                CFNumberGetValue(n.cast(), kCFNumberSInt32Type, (&mut p as *mut i32).cast());
            }
            u16::try_from(p).unwrap_or(0)
        };
        if is(kCFProxyTypeNone) {
            Raw::Entry(Entry::Direct)
        } else if is(kCFProxyTypeHTTP) || is(kCFProxyTypeHTTPS) {
            Raw::Entry(Entry::Http(host(), port()))
        } else if is(kCFProxyTypeSOCKS) {
            Raw::Entry(Entry::Socks(host(), port()))
        } else if is(kCFProxyTypeAutoConfigurationURL) {
            let u = get(dict, kCFProxyAutoConfigurationURLKey);
            if !u.is_null() && CFGetTypeID(u) == CFURLGetTypeID() {
                Raw::Script(u as CFURLRef)
            } else {
                Raw::Entry(Entry::Other("a script with no address".into()))
            }
        } else if is(kCFProxyTypeAutoConfigurationJavaScript) {
            let s = get(dict, kCFProxyAutoConfigurationJavaScriptKey);
            if !s.is_null() && CFGetTypeID(s) == CFStringGetTypeID() {
                Raw::ScriptText(s as CFStringRef)
            } else {
                Raw::Entry(Entry::Other("a script with no text".into()))
            }
        } else {
            Raw::Entry(Entry::Other(
                string(kind).unwrap_or_else(|| "unknown".into()),
            ))
        }
    }

    unsafe fn entries(list: CFArrayRef) -> Vec<Entry> {
        (0..CFArrayGetCount(list))
            .map(
                |i| match read(CFArrayGetValueAtIndex(list, i) as CFDictionaryRef) {
                    Raw::Entry(e) => e,
                    Raw::Script(_) | Raw::ScriptText(_) => {
                        Entry::Other("a script inside a script".into())
                    }
                },
            )
            .collect()
    }

    /// Where the script's answer lands; the callback fills it and stops the run loop.
    type Slot = Option<Result<Vec<Entry>, String>>;

    extern "C" fn on_result(client: *mut c_void, proxies: CFArrayRef, error: CFErrorRef) {
        let slot = unsafe { &mut *(client as *mut Slot) };
        *slot = Some(if !error.is_null() {
            let e = unsafe { CFType::wrap_under_get_rule(error.cast()) };
            Err(format!("{e:?}"))
        } else if proxies.is_null() {
            Ok(vec![])
        } else {
            Ok(unsafe { entries(proxies) })
        });
        unsafe { CFRunLoopStop(CFRunLoopGetCurrent()) };
    }

    /// One script run at a time in the process: Chromium serialises them too, since concurrent
    /// runs on separate threads misbehave.
    static SCRIPT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Runs a script on this thread's own run loop, for 20 s at most.
    unsafe fn run_script(raw: &Raw, target: CFURLRef) -> Result<Vec<Entry>, String> {
        let _one = SCRIPT.lock().unwrap_or_else(|e| e.into_inner());
        let mut slot: Slot = None;
        let mut ctx = ClientContext {
            version: 0,
            info: (&mut slot as *mut Slot).cast(),
            retain: None,
            release: None,
            copy_description: None,
        };
        let source = match raw {
            Raw::Script(u) => {
                CFNetworkExecuteProxyAutoConfigurationURL(*u, target, on_result, &mut ctx)
            }
            Raw::ScriptText(s) => {
                CFNetworkExecuteProxyAutoConfigurationScript(*s, target, on_result, &mut ctx)
            }
            Raw::Entry(_) => return Err("not a script".into()),
        };
        if source.is_null() {
            return Err("the system would not run its proxy script".into());
        }
        let mode = CFString::new("talos.proxy-script");
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, mode.as_concrete_TypeRef());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        // Stopped (the callback ran), timed out, or finished (nothing left to run): all end the
        // wait. Only a source handled without the answer keeps it going.
        while slot.is_none() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero()
                || CFRunLoopRunInMode(mode.as_concrete_TypeRef(), left.as_secs_f64(), 0)
                    != kCFRunLoopRunHandledSource
            {
                break;
            }
        }
        // Invalidated before release: a late answer can never reach `slot` once this returns.
        CFRunLoopSourceInvalidate(source);
        CFRunLoopRemoveSource(run_loop, source, mode.as_concrete_TypeRef());
        CFRelease(source.cast());
        slot.unwrap_or_else(|| Err("the proxy script gave no answer in 20 s".into()))
    }

    pub fn route_for(url: &Url) -> Result<Route, String> {
        let target = unsafe {
            let s = CFString::new(url.as_str());
            let u = core_foundation_sys::url::CFURLCreateWithString(
                std::ptr::null(),
                s.as_concrete_TypeRef(),
                std::ptr::null(),
            );
            if u.is_null() {
                return Err(format!("{url} is not an address the system reads"));
            }
            CFURL::wrap_under_create_rule(u)
        };
        unsafe {
            let settings = CFNetworkCopySystemProxySettings();
            if settings.is_null() {
                return Err("the system's proxy settings could not be read".into());
            }
            let list = CFNetworkCopyProxiesForURL(target.as_concrete_TypeRef(), settings);
            if list.is_null() {
                CFRelease(settings.cast());
                return Err("the system gave no proxy answer".into());
            }
            // The first entry decides; a script is run first, and its own first entry decides.
            // A script that cannot be run gives way to the next entry, as CFNetwork does (an
            // auto-discovery with no WPAD server, followed by "none"); the last one cannot.
            let count = CFArrayGetCount(list);
            let mut result = Ok(Route::Direct);
            for i in 0..count {
                result = match read(CFArrayGetValueAtIndex(list, i) as CFDictionaryRef) {
                    Raw::Entry(e) => pick(&[e]),
                    raw => run_script(&raw, target.as_concrete_TypeRef())
                        .map_err(|e| format!("the system's proxy script could not be run: {e}"))
                        .and_then(|found| pick(&found)),
                };
                let script_failed = result
                    .as_ref()
                    .is_err_and(|e| e.starts_with("the system's proxy script"));
                if !script_failed {
                    break;
                }
            }
            CFRelease(list.cast());
            CFRelease(settings.cast());
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_entry_is_the_road() {
        assert_eq!(pick(&[]), Ok(Route::Direct));
        assert_eq!(pick(&[Entry::Direct]), Ok(Route::Direct));
        assert_eq!(
            pick(&[Entry::Http("127.0.0.1".into(), 9000), Entry::Direct]),
            Ok(Route::Proxy("http://127.0.0.1:9000".into()))
        );
        assert_eq!(
            pick(&[Entry::Direct, Entry::Http("p".into(), 1)]),
            Ok(Route::Direct)
        );
        // A road the probe cannot take is said, never replaced by direct.
        assert!(pick(&[Entry::Socks("s".into(), 1080)]).is_err());
        assert!(pick(&[Entry::Other("x".into())]).is_err());
    }

    #[test]
    fn a_winhttp_list_gives_its_first_proxy() {
        let proxy = |p: &str| Ok(Route::Proxy(p.into()));
        assert_eq!(
            from_winhttp_list("127.0.0.1:9000"),
            proxy("http://127.0.0.1:9000")
        );
        assert_eq!(from_winhttp_list("a:1; b:2"), proxy("http://a:1"));
        assert_eq!(
            from_winhttp_list("http=h:80;https=s:443"),
            proxy("http://s:443")
        );
        assert_eq!(
            from_winhttp_list("ftp=f:21;http=h:80"),
            proxy("http://h:80")
        );
        assert_eq!(from_winhttp_list(""), Ok(Route::Direct));
        assert_eq!(from_winhttp_list("DIRECT"), Ok(Route::Direct));
        // A script's SOCKS answer: the application goes through it, so never direct.
        assert!(from_winhttp_list("socks=s:1080").is_err());
        assert!(from_winhttp_list("ftp=f:21").is_err());
    }
}
