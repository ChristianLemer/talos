//! passage.rs — does a corporate firewall let through the download an application will make
//! ITSELF? Talos installs nothing here: it asks for the file the way the application's own
//! updater will, and says whether the file's bytes came back.
//!
//! WHY: an application that updates itself (`version: "self"`, an Electron app) cannot show the
//! warning page a corporate firewall puts in front of some downloads, so its update fails with
//! a bare "cannot download". The person can approve that page — in Talos's 403 panel — and the
//! application's next try goes through. The manager cannot do the update instead: winget closes
//! every process of the application it upgrades, its terminals included (measured).
//!
//! ⭐ THE RULE (the operator's): one `GET` of byte 0 (`Range: bytes=0-0`), following redirects.
//! Only `206 Partial Content` with a type that is not text is the file — reachable, and its own
//! bytes rather than a page in its place: `application/octet-stream` as often as not, but a host
//! may name the kind (`application/zip`, `application/x-msdownload`). A `206 text/html` (or any
//! html type) is the firewall's page, never a pass; a 206 that names no type is not taken on trust. Any other
//! answer is a block: a 403, a redirect to the firewall's gateway (it ends on the gateway's
//! page), a `200 text/html` (the page instead of the file). So a block is recognised by what it is NOT, and no vendor's hostname is
//! written here. A network that does not answer at all is not a block — there is no page to
//! approve — and is said as a failure.
//!
//! The probe goes through the engine's own HTTP client, never a shell: the OS's TLS stack
//! (which trusts the root a corporate firewall installs), on THE APPLICATION'S ROAD. A corporate
//! client agent commonly runs in proxy auto-config mode: the OS follows a script that sends the
//! filtered hosts to a local proxy, and whatever does not follow it leaves unfiltered — so a
//! probe that went direct got `206` while the application, through the script's proxy, got the
//! firewall's `403` (measured on a Mac). The road is therefore the OS's answer for each address
//! (proxy.rs), asked again at EVERY redirect, since the host changes (`github.com` → the release
//! assets' host): redirects are followed here, not by the client. A road the OS cannot give is a
//! failure, never a pass. Linux keeps the environment's proxy (no script read; unmeasured).
//! Through a proxy, an https address is a `CONNECT` tunnel: the measured agent accepts it and
//! answers inside (its `307` to the gateway's page), which is classified as usual. A proxy that
//! refused the tunnel itself would be a failure — no page comes back, so none can be approved.
//! Probing through the webview instead was ruled out: its `fetch` cannot read a cross-origin
//! status, so it could not tell a `206` from the firewall's page.
use crate::bundles::Passage;
use crate::platform::Os;
use crate::proxy::Route;
use reqwest::Url;
use std::sync::Arc;
use std::time::Duration;

/// What one probe found.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// 206 with the file's bytes: the application's own update can take it.
    Passes,
    /// Anything else that answered: the firewall's page is the thing to approve.
    Blocked(String),
    /// Nothing usable came back (no network, a real feed with no file for this machine): no page
    /// to approve.
    Failed(String),
}

/// A probe's answer, and the address it is about: the file, or the feed when the feed itself
/// was held (approving the feed's page is then the gesture, and the next probe goes further).
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub url: String,
    pub verdict: Verdict,
    /// The road each hop took, in order (`github.com: through http://127.0.0.1:9000`): what the
    /// row says, so a pass can be checked against the application's own road.
    pub roads: Vec<String>,
}

/// The rule, on the final answer after redirects. Pure, so every case is tested offline.
pub fn classify(status: u16, content_type: Option<&str>) -> Verdict {
    let mime = content_type
        .and_then(|c| c.split(';').next())
        .map(|m| m.trim().to_ascii_lowercase())
        .unwrap_or_default();
    // A page is never the file, whatever prefix it wears (`application/xhtml+xml`).
    let page = mime.starts_with("text/") || mime.contains("html");
    if status == 206 && !mime.is_empty() && !page {
        return Verdict::Passes;
    }
    let what = if mime.is_empty() {
        "no content type".to_string()
    } else {
        mime.clone()
    };
    Verdict::Blocked(match status {
        403 => "the firewall refused it (403)".to_string(),
        200 if mime.starts_with("text/html") => {
            "a web page came back instead of the file (200 text/html)".to_string()
        }
        _ => format!("it answered {status} with {what}, not the file's bytes (206)"),
    })
}

/// Whether the feed's own answer is held, from its status and type: the feed is a YAML text, so a
/// 200 that is not a web page may be it (the body says — `not_a_feed`). Anything else is held the
/// way a file is: its page is what to approve first. The type is read in any case (`Text/HTML`).
pub fn feed_held(status: u16, content_type: Option<&str>) -> Option<Verdict> {
    let html = content_type.is_some_and(|c| c.to_ascii_lowercase().contains("text/html"));
    if status == 200 && !html {
        return None;
    }
    Some(match classify(status, content_type) {
        Verdict::Passes => Verdict::Blocked(format!("the feed answered {status}")),
        v => v,
    })
}

/// A 200 that is not the feed at all — neither `files:` nor `path:` in it: a page served in its
/// place under another type (`text/plain`, none). That is a block with a page to approve, not a
/// failure. A real feed that names no file for this machine stays a failure (`file_from_feed`).
pub fn not_a_feed(body: &str) -> Option<Verdict> {
    let doc: Option<serde_yaml::Value> = serde_yaml::from_str(body).ok();
    let is_feed = doc.is_some_and(|d| d.get("files").is_some() || d.get("path").is_some());
    (!is_feed).then(|| {
        Verdict::Blocked("the feed's address answered something that is not a feed".into())
    })
}

/// The file an electron-updater feed (`latest.yml`, `latest-mac.yml`, `latest-linux.yml`) names —
/// the one the application's updater takes — resolved against the feed's address. The updater
/// keeps one kind per OS (a `.zip` on macOS, an `.exe` on Windows, an `.AppImage` on Linux), then
/// the file for the machine's architecture when the feed lists one per architecture.
pub fn file_from_feed(feed: &Url, body: &str, os: Os, arch: &str) -> Result<Url, String> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(body).map_err(|e| format!("the feed is not YAML: {e}"))?;
    let mut names: Vec<String> = doc
        .get("files")
        .and_then(|f| f.as_sequence())
        .map(|files| {
            files
                .iter()
                .filter_map(|f| f.get("url").and_then(|u| u.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        if let Some(path) = doc.get("path").and_then(|p| p.as_str()) {
            names.push(path.to_string());
        }
    }
    let kind = match os {
        Os::Darwin => ".zip",
        Os::Windows => ".exe",
        Os::Linux => ".appimage",
    };
    names.retain(|n| n.to_ascii_lowercase().ends_with(kind));
    // A feed listing an arm64 file beside another: this machine takes its own architecture's. A
    // machine of the other architecture never takes an arm64 file it would not run.
    if names.iter().any(|n| n.contains("arm64")) {
        let want_arm = arch == "aarch64";
        names.retain(|n| n.contains("arm64") == want_arm);
    }
    let name = names.first().ok_or("the feed names no file to download")?;
    feed.join(name)
        .map_err(|e| format!("the feed names `{name}`, not an address: {e}"))
}

/// Which road an address takes. The OS's answer in the engine; a stand-in in the tests.
pub type Resolver = Arc<dyn Fn(&Url) -> Result<Route, String> + Send + Sync>;

/// As many redirects as reqwest's own default policy follows.
const MAX_REDIRECTS: usize = 10;

fn client(route: &Route) -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("Talos/", env!("CARGO_PKG_VERSION")));
    let builder = match route {
        Route::Direct => builder.no_proxy(),
        Route::Proxy(p) => builder.proxy(
            reqwest::Proxy::all(p.as_str())
                .map_err(|e| format!("the proxy {p} is unusable: {e}"))?,
        ),
        Route::System => builder,
    };
    builder.build().map_err(|e| format!("no HTTP client: {e}"))
}

/// A request that got no answer — no route, no name, a timeout, a TLS refusal — is the network,
/// and no page helps.
fn unanswered(e: &reqwest::Error) -> Verdict {
    Verdict::Failed(format!("no answer: {e}"))
}

/// One GET, its redirects followed by hand: the road is asked again at every hop. The answer
/// returned is the last one, which is not a redirect (or a redirect with nowhere to go).
async fn fetch(
    resolve: &Resolver,
    start: &Url,
    range: bool,
    roads: &mut Vec<String>,
) -> Result<reqwest::Response, Verdict> {
    let mut url = start.clone();
    for _ in 0..=MAX_REDIRECTS {
        let host = url.host_str().unwrap_or("?").to_string();
        let (r, u) = (resolve.clone(), url.clone());
        let route = tokio::task::spawn_blocking(move || r(&u))
            .await
            .map_err(|e| e.to_string())
            .and_then(|found| found)
            .map_err(|e| {
                Verdict::Failed(format!(
                    "the system could not say which road {host} takes: {e}"
                ))
            })?;
        roads.push(format!("{host}: {route}"));
        let mut req = client(&route).map_err(Verdict::Failed)?.get(url.clone());
        if range {
            req = req.header(reqwest::header::RANGE, "bytes=0-0");
        }
        let resp = req.send().await.map_err(|e| unanswered(&e))?;
        let next = resp
            .status()
            .is_redirection()
            .then(|| resp.headers().get(reqwest::header::LOCATION))
            .flatten()
            .and_then(|l| l.to_str().ok())
            .and_then(|l| url.join(l).ok());
        match next {
            Some(n) => url = n,
            None => return Ok(resp),
        }
    }
    Err(Verdict::Blocked("it never stopped redirecting".into()))
}

/// Probes the passage: the feed first when there is one (for the file's name), then the file.
/// Each hop on the road the OS gives it.
pub async fn probe(passage: &Passage, os: Os) -> Probe {
    probe_with(passage, os, Arc::new(crate::proxy::route_for)).await
}

/// `probe`, with the road given by `resolve`.
pub async fn probe_with(passage: &Passage, os: Os, resolve: Resolver) -> Probe {
    let mut roads = Vec::new();
    let file = match (&passage.url, &passage.feed) {
        (Some(url), _) => url.clone(),
        (None, Some(feed)) => {
            let Ok(feed_url) = Url::parse(feed) else {
                return done(
                    feed,
                    Verdict::Failed("the feed is not an address".into()),
                    roads,
                );
            };
            let resp = match fetch(&resolve, &feed_url, false, &mut roads).await {
                Ok(r) => r,
                Err(verdict) => return done(feed, verdict, roads),
            };
            if let Some(verdict) = feed_held(resp.status().as_u16(), header(&resp).as_deref()) {
                return done(feed, verdict, roads);
            }
            let body = match resp.text().await {
                Ok(b) => b,
                Err(e) => {
                    let why = format!("the feed could not be read: {e}");
                    return done(feed, Verdict::Failed(why), roads);
                }
            };
            if let Some(verdict) = not_a_feed(&body) {
                return done(feed, verdict, roads);
            }
            match file_from_feed(&feed_url, &body, os, std::env::consts::ARCH) {
                Ok(u) => u.to_string(),
                Err(e) => return done(feed, Verdict::Failed(e), roads),
            }
        }
        (None, None) => {
            let why = "neither `feed:` nor `url:` is declared".into();
            return done("", Verdict::Failed(why), roads);
        }
    };
    let Ok(file_url) = Url::parse(&file) else {
        return done(
            &file,
            Verdict::Failed("the file is not an address".into()),
            roads,
        );
    };
    let verdict = match fetch(&resolve, &file_url, true, &mut roads).await {
        Ok(resp) => classify(resp.status().as_u16(), header(&resp).as_deref()),
        Err(verdict) => verdict,
    };
    done(&file, verdict, roads)
}

fn done(url: &str, verdict: Verdict, roads: Vec<String>) -> Probe {
    Probe {
        url: url.to_string(),
        verdict,
        roads,
    }
}

fn header(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_206_with_the_files_bytes_passes() {
        // Whatever kind the host names the file by — but not text.
        for ct in [
            "application/octet-stream",
            "Application/Octet-Stream; charset=binary",
            "application/zip",
            "application/x-msdownload",
            "application/x-apple-diskimage",
        ] {
            assert_eq!(classify(206, Some(ct)), Verdict::Passes, "{ct}");
        }
        // The firewall's refusal, its page in place of the file, its gateway's page after a
        // redirect, a server ignoring the range: all held.
        for (status, ct) in [
            (403, Some("text/html")),
            (200, Some("text/html; charset=utf-8")),
            (200, Some("application/octet-stream")),
            (206, Some("text/html")),
            (206, Some("Text/HTML; charset=utf-8")),
            (206, Some("text/plain")),
            (206, Some("application/xhtml+xml")),
            (206, None),
            (302, None),
            (500, Some("text/plain")),
        ] {
            assert!(
                matches!(classify(status, ct), Verdict::Blocked(_)),
                "{status} {ct:?} must not pass"
            );
        }
        let Verdict::Blocked(why) = classify(403, Some("text/html")) else {
            unreachable!()
        };
        assert!(why.contains("403"), "{why}");
        let Verdict::Blocked(why) = classify(200, Some("text/html")) else {
            unreachable!()
        };
        assert!(why.contains("web page"), "{why}");
    }

    #[test]
    fn a_feed_answered_by_a_page_is_held_whatever_its_type_says() {
        // The type read in any case: the firewall's page is held before any YAML is parsed.
        for ct in [Some("text/html"), Some("Text/HTML; charset=UTF-8")] {
            assert!(
                matches!(feed_held(200, ct), Some(Verdict::Blocked(_))),
                "{ct:?}"
            );
        }
        assert!(matches!(feed_held(403, None), Some(Verdict::Blocked(_))));
        assert!(matches!(
            feed_held(206, Some("application/octet-stream")),
            Some(Verdict::Blocked(_))
        ));
        // A 200 that is not a web page may be the feed: its body says.
        for ct in [None, Some("text/plain"), Some("application/x-yaml")] {
            assert_eq!(feed_held(200, ct), None, "{ct:?}");
        }
        // A page under another type, or no type, names no file: a block, with a page to approve.
        for body in [
            "<html><body>Access to this site is blocked</body></html>",
            "Blocked by policy.",
            "",
            "- a\n- b\n",
        ] {
            assert!(
                matches!(not_a_feed(body), Some(Verdict::Blocked(_))),
                "{body:?}"
            );
        }
        // A real feed is not held, even one with no file for this machine (that one fails).
        assert_eq!(not_a_feed(MAC_FEED), None);
        assert_eq!(not_a_feed("files:\n  - url: App.dmg\n"), None);
        assert_eq!(not_a_feed("path: x.exe\n"), None);
    }

    const MAC_FEED: &str = "version: 1.4.246\nfiles:\n  - url: Orca-1.4.246-arm64-mac.zip\n    sha512: x\n    size: 1\n  - url: Orca-1.4.246-mac.zip\n    size: 1\n  - url: Orca-1.4.246-arm64.dmg\n    size: 1\npath: Orca-1.4.246-arm64-mac.zip\n";

    #[test]
    fn a_feed_names_the_file_the_updater_takes() {
        let feed =
            Url::parse("https://github.com/o/r/releases/latest/download/latest-mac.yml").unwrap();
        let arm = file_from_feed(&feed, MAC_FEED, Os::Darwin, "aarch64").unwrap();
        assert_eq!(
            arm.as_str(),
            "https://github.com/o/r/releases/latest/download/Orca-1.4.246-arm64-mac.zip"
        );
        let intel = file_from_feed(&feed, MAC_FEED, Os::Darwin, "x86_64").unwrap();
        assert!(intel.as_str().ends_with("/Orca-1.4.246-mac.zip"), "{intel}");
        let win = "version: 1.4.246\nfiles:\n  - url: orca-windows-setup.exe\n    size: 1\npath: orca-windows-setup.exe\n";
        let w = file_from_feed(&feed, win, Os::Windows, "x86_64").unwrap();
        assert!(
            w.as_str().ends_with("/download/orca-windows-setup.exe"),
            "{w}"
        );
        // An older feed with only `path:`; an absolute address is taken as it is.
        // One installer per architecture, and a file that is not an installer: the updater's own.
        let both =
            "files:\n  - url: App.blockmap\n  - url: App-Setup-arm64.exe\n  - url: App-Setup.exe\n";
        let w = file_from_feed(&feed, both, Os::Windows, "x86_64").unwrap();
        assert!(w.as_str().ends_with("/App-Setup.exe"), "{w}");
        let a = file_from_feed(&feed, both, Os::Windows, "aarch64").unwrap();
        assert!(a.as_str().ends_with("/App-Setup-arm64.exe"), "{a}");
        let linux = "files:\n  - url: app_1.0_amd64.deb\n  - url: App-1.0.AppImage\n";
        let l = file_from_feed(&feed, linux, Os::Linux, "x86_64").unwrap();
        assert!(l.as_str().ends_with("/App-1.0.AppImage"), "{l}");
        // An Intel Mac never takes an arm64-only zip: nothing for it is a failure, not a guess.
        let arm_only = "files:\n  - url: App-arm64-mac.zip\n";
        assert!(file_from_feed(&feed, arm_only, Os::Darwin, "x86_64").is_err());
        let old = file_from_feed(
            &feed,
            "path: https://cdn.example/x.exe\n",
            Os::Windows,
            "x86_64",
        )
        .unwrap();
        assert_eq!(old.as_str(), "https://cdn.example/x.exe");
    }

    #[test]
    fn a_feed_with_no_file_for_this_os_is_a_failure_not_a_guess() {
        let feed = Url::parse("https://example.com/latest-mac.yml").unwrap();
        let dmg_only = "files:\n  - url: App.dmg\n";
        assert!(file_from_feed(&feed, dmg_only, Os::Darwin, "aarch64").is_err());
        assert!(file_from_feed(&feed, "<html>blocked</html>", Os::Darwin, "aarch64").is_err());
    }

    #[test]
    fn no_answer_is_a_failure_not_a_block() {
        // Nothing listens on port 9 of the loopback: the network fails, there is no page.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let p = Passage {
            feed: None,
            url: Some("http://127.0.0.1:9/x.zip".into()),
        };
        let probe = rt.block_on(probe(&p, Os::Linux));
        assert!(matches!(probe.verdict, Verdict::Failed(_)), "{probe:?}");
        assert_eq!(probe.url, "http://127.0.0.1:9/x.zip");
    }

    /// A one-thread HTTP stand-in: each connection's request line → the raw answer `reply` gives.
    async fn stand_in(reply: fn(&str) -> String) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let answer = reply(req.lines().next().unwrap_or(""));
                let _ = sock.write_all(answer.as_bytes()).await;
            }
        });
        port
    }

    fn answer(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// The origin: `/latest` redirects to a host the machine cannot resolve (only a proxy can
    /// reach it, like a release-assets host behind the script's proxy); `/direct.zip` is the file.
    fn origin(line: &str) -> String {
        if line.contains("/latest") {
            answer(
                "302 Found",
                "Location: http://assets.invalid/held.zip\r\n",
                "",
            )
        } else if line.contains("/to-open") {
            answer(
                "302 Found",
                "Location: http://assets.invalid/open.zip\r\n",
                "",
            )
        } else {
            answer(
                "206 Partial Content",
                "Content-Type: application/octet-stream\r\n",
                "x",
            )
        }
    }

    /// The script's proxy: the firewall's page for `held`, the file's byte otherwise.
    fn firewall(line: &str) -> String {
        if line.contains("held") {
            answer(
                "403 Forbidden",
                "Content-Type: text/html\r\n",
                "<html>warning</html>",
            )
        } else {
            answer(
                "206 Partial Content",
                "Content-Type: application/octet-stream\r\n",
                "x",
            )
        }
    }

    #[test]
    fn every_hop_takes_the_road_the_system_gives_it() {
        // ⭐ The field case: the first host is direct, the host it redirects to goes through
        // the script's proxy, and the proxy answers the firewall's page. The probe must see the
        // page, not the direct road's file.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let o = stand_in(origin).await;
            let f = stand_in(firewall).await;
            let proxy = format!("http://127.0.0.1:{f}");
            let script = {
                let proxy = proxy.clone();
                Arc::new(move |u: &Url| {
                    Ok(if u.host_str() == Some("assets.invalid") {
                        Route::Proxy(proxy.clone())
                    } else {
                        Route::Direct
                    })
                }) as Resolver
            };
            let at = |path: &str| Passage {
                feed: None,
                url: Some(format!("http://127.0.0.1:{o}{path}")),
            };

            let held = probe_with(&at("/latest"), Os::Linux, script.clone()).await;
            assert!(
                matches!(&held.verdict, Verdict::Blocked(why) if why.contains("403")),
                "{held:?}"
            );
            assert_eq!(
                held.roads,
                vec![
                    "127.0.0.1: direct".to_string(),
                    format!("assets.invalid: through {proxy}")
                ]
            );
            // Through the proxy, a file it lets by passes: the second hop really went there.
            let open = probe_with(&at("/to-open"), Os::Linux, script.clone()).await;
            assert_eq!(open.verdict, Verdict::Passes, "{open:?}");
            // Direct all the way, the file passes, as before.
            let direct = probe_with(&at("/direct.zip"), Os::Linux, script).await;
            assert_eq!(direct.verdict, Verdict::Passes, "{direct:?}");
            assert_eq!(direct.roads, vec!["127.0.0.1: direct".to_string()]);

            // A system that cannot say the road: a failure, never a pass on the direct road.
            let blind: Resolver = Arc::new(|_: &Url| Err("no script".to_string()));
            let unknown = probe_with(&at("/direct.zip"), Os::Linux, blind).await;
            assert!(
                matches!(&unknown.verdict, Verdict::Failed(why) if why.contains("which road")),
                "{unknown:?}"
            );
        });
    }

    #[test]
    fn a_redirect_loop_is_a_block() {
        fn looping(_: &str) -> String {
            answer("302 Found", "Location: /again\r\n", "")
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let o = stand_in(looping).await;
            let p = Passage {
                feed: None,
                url: Some(format!("http://127.0.0.1:{o}/x.zip")),
            };
            let direct: Resolver = Arc::new(|_: &Url| Ok(Route::Direct));
            let probe = probe_with(&p, Os::Linux, direct).await;
            assert!(
                matches!(&probe.verdict, Verdict::Blocked(why) if why.contains("redirecting")),
                "{probe:?}"
            );
            assert_eq!(probe.roads.len(), MAX_REDIRECTS + 1);
        });
    }
}
