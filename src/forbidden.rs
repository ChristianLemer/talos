// 403 firewall detector, PURE, manager-agnostic.
use regex::Regex;
use std::sync::LazyLock;

// winget : "Forbidden (403)"
static WINGET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Forbidden \(403\)").unwrap());
// brew/curl: a bare 403 near a download failure
static BARE403: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b403\b").unwrap());
static CONTEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)curl:\s*\(22\)|Download failed|returned error").unwrap());

/// 403 corporate-firewall in the output (streamed chunk or whole buffer).
/// Deliberately broad: a false positive only costs a useless Retry.
/// Read once over a step's whole output, when the step ends.
pub fn is403(text: &str) -> bool {
    WINGET.is_match(text) || (BARE403.is_match(text) && CONTEXT.is_match(text))
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
