// Port de src/forbidden.ts — détecteur 403 pare-feu, PUR, manager-agnostic.
use regex::Regex;

/// 403 corporate-firewall dans la sortie (chunk streamé ou buffer entier).
/// Volontairement large : un faux positif ne coûte qu'un Retry inutile.
pub fn is403(text: &str) -> bool {
    // winget : "Forbidden (403)"
    let winget = Regex::new(r"(?i)Forbidden \(403\)").unwrap();
    if winget.is_match(text) {
        return true;
    }
    // brew/curl : un 403 nu près d'un échec de download
    let bare403 = Regex::new(r"\b403\b").unwrap();
    let ctx = Regex::new(r"(?i)curl:\s*\(22\)|Download failed|returned error").unwrap();
    bare403.is_match(text) && ctx.is_match(text)
}

/// Première URL http(s), en s'arrêtant aux caractères de contrôle (l'échappement
/// OSC de progression winget colle à l'URL sans espace → \S+ l'avalerait), puis
/// trim la ponctuation finale sur laquelle une URL ne finit jamais vraiment.
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
    fn detecte_403_winget() {
        assert!(is403("... Forbidden (403) ..."));
    }

    #[test]
    fn detecte_403_curl() {
        assert!(is403("curl: (22) The requested URL returned error: 403"));
    }

    #[test]
    fn ignore_403_isole_sans_contexte() {
        assert!(!is403("build 403 succeeded")); // 403 nu sans échec download
    }

    #[test]
    fn extrait_url_et_coupe_osc() {
        // l'échappement OSC \x1b]9;4… colle à l'URL
        let s = "Downloading https://example.com/pkg.exe\x1b]9;4;3;0\x1b\\";
        assert_eq!(
            extract_url(s).as_deref(),
            Some("https://example.com/pkg.exe")
        );
    }

    #[test]
    fn extrait_url_trim_ponctuation() {
        assert_eq!(
            extract_url("failed: https://example.com/x.").as_deref(),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn pas_durl() {
        assert_eq!(extract_url("no link here"), None);
    }
}
