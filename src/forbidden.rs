// 403 firewall detector, PURE, manager-agnostic.
use regex::Regex;
use std::sync::LazyLock;

// winget : "Forbidden (403)"
static WINGET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Forbidden \(403\)").unwrap());
// brew/curl: a bare 403 near a download failure
static BARE403: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b403\b").unwrap());
static CONTEXT: LazyLock<Regex> =
    // `\s{0,8}`, not `\s*`: every pattern must stay shorter than `Watch403::OVERLAP`.
    LazyLock::new(|| {
        Regex::new(r"(?i)curl:\s{0,8}\(22\)|Download failed|returned error").unwrap()
    });

/// 403 corporate-firewall in the output (streamed chunk or whole buffer).
/// Deliberately broad: a false positive only costs a useless Retry.
/// The streamed step reads it through `Watch403`; this whole-text form is the rule's
/// reference, which the tests hold `Watch403` to.
#[cfg(test)]
pub fn is403(text: &str) -> bool {
    WINGET.is_match(text) || (BARE403.is_match(text) && CONTEXT.is_match(text))
}

/// `is403` for a buffer that grows chunk by chunk, reading only what each chunk added — the
/// whole buffer re-read on every chunk made a long step quadratic. Each pattern is
/// remembered once seen, so the verdict is the whole buffer's, as `is403` would give it.
#[derive(Debug, Default)]
pub struct Watch403 {
    winget: bool,
    bare: bool,
    context: bool,
}

impl Watch403 {
    /// Longer than any of the patterns, so one cut between two chunks is still matched.
    const OVERLAP: usize = 64;

    /// `buf` has just grown from `appended_from`; true once the 403 is seen. A `403` that
    /// ends the buffer is held back: the next chunk may make it `4035` (`finish` decides
    /// when there is none).
    pub fn feed(&mut self, buf: &str, appended_from: usize) -> bool {
        self.scan(buf, appended_from, false)
    }

    /// The output is complete: a `403` that ended it counts.
    pub fn finish(&mut self, buf: &str) -> bool {
        self.scan(buf, buf.len(), true)
    }

    fn scan(&mut self, buf: &str, appended_from: usize, complete: bool) -> bool {
        let mut from = appended_from.saturating_sub(Self::OVERLAP).min(buf.len());
        while !buf.is_char_boundary(from) {
            from -= 1;
        }
        let new = &buf[from..];
        self.winget = self.winget || WINGET.is_match(new);
        // The slice starts mid-text: `\b` sees a boundary at its first byte that the whole
        // buffer may not have (`51403` cut before the `4`). A match there counts only if what
        // precedes it in the buffer is not part of a word — and one at the very end only once
        // nothing more can follow it.
        let glued = buf[..from]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        self.bare = self.bare
            || BARE403
                .find_iter(new)
                .any(|m| (m.start() > 0 || !glued) && (complete || m.end() < new.len()));
        self.context = self.context || CONTEXT.is_match(new);
        self.winget || (self.bare && self.context)
    }
}

/// First http(s) URL, stopping at control characters (winget's OSC progress
/// escape sticks to the URL with no space → \S+ would swallow it), then
/// trims trailing punctuation on which a URL never really ends.
pub fn extract_url(text: &str) -> Option<String> {
    let re = Regex::new(r"https?://[^\s\x00-\x1f\x7f]+").unwrap();
    let m = re.find(text)?;
    let trimmed = m
        .as_str()
        .trim_end_matches(|c: char| ".,;:!?)]}>'\"".contains(c));
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_403_winget() {
        assert!(is403("... Forbidden (403) ..."));
    }

    #[test]
    fn detects_403_curl() {
        assert!(is403("curl: (22) The requested URL returned error: 403"));
    }

    #[test]
    fn ignores_bare_403_without_context() {
        assert!(!is403("build 403 succeeded")); // bare 403 without download failure
    }

    /// Streamed, the verdict is the whole buffer's: a pattern cut between two chunks is still
    /// seen, and a 403 in one chunk meets its download-failure context in a later one.
    #[test]
    fn the_streamed_verdict_is_the_whole_buffers() {
        for whole in [
            "Downloading...\nForbidden (403)\nbye",
            "curl: (22) The requested URL returned error: 403",
            "HTTP 403\n...lots of output...\nDownload failed",
            "build 403 succeeded",
            // And one that ENDS the output still counts.
            "Download failed: 403",
            // A slice cut inside `51403` must not see a word starting at `403`.
            // A `403` that ends one chunk and turns into `4035` in the next is no 403.
            "the size 4035 bytes was returned error 500 from the mirror, over and over and over",
            "copied 51403 bytes\n.......................................................\
             ............ and later the request returned error 500",
        ] {
            for size in [1, 2, 3, 7, 64, 66, 67, 68, 69, 73] {
                let mut buf = String::new();
                let mut watch = Watch403::default();
                let mut seen = false;
                for chunk in whole.as_bytes().chunks(size) {
                    let from = buf.len();
                    buf.push_str(std::str::from_utf8(chunk).unwrap());
                    seen |= watch.feed(&buf, from);
                }
                seen |= watch.finish(&buf);
                assert_eq!(seen, is403(whole), "{whole:?} in chunks of {size}");
            }
        }
    }

    #[test]
    fn extract_url_cuts_osc() {
        // the OSC escape \x1b]9;4… sticks to the URL
        let s = "Downloading https://example.com/pkg.exe\x1b]9;4;3;0\x1b\\";
        assert_eq!(
            extract_url(s).as_deref(),
            Some("https://example.com/pkg.exe")
        );
    }

    #[test]
    fn extract_url_trims_punctuation() {
        assert_eq!(
            extract_url("failed: https://example.com/x.").as_deref(),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn no_url() {
        assert_eq!(extract_url("no link here"), None);
    }
}
