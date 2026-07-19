// Port of src/forbidden.ts — 403 firewall detector, PURE, manager-agnostic.
use regex::Regex;

/// 403 corporate-firewall in the output (streamed chunk or whole buffer).
/// Deliberately broad: a false positive only costs a useless Retry.
pub fn is403(text: &str) -> bool {
    // winget : "Forbidden (403)"
    let winget = Regex::new(r"(?i)Forbidden \(403\)").unwrap();
    if winget.is_match(text) {
        return true;
    }
    // brew/curl: a bare 403 near a download failure
    let bare403 = Regex::new(r"\b403\b").unwrap();
    let ctx = Regex::new(r"(?i)curl:\s*\(22\)|Download failed|returned error").unwrap();
    bare403.is_match(text) && ctx.is_match(text)
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
