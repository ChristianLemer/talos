//! Assets front SCELLÉS dans le binaire (raccourci #3 du spike).
//!
//! Le spike servait `public/` via un chemin RELATIF (`TALOS_PUBLIC`, défaut `"public"`),
//! lu au runtime par `ServeDir`/`read_to_string`. Lancé par Finder/Explorer, le cwd
//! n'est PAS la racine du projet → `public/` introuvable → `<h1>index.html introuvable</h1>`.
//!
//! Ici on embarque `public/` DANS le binaire à la compilation (comme
//! `deno compile --include public`), relatif au crate root — donc indépendant du cwd
//! au runtime. `TALOS_PUBLIC` survit comme ÉCHAPPATOIRE DEV : présent → on lit le
//! disque (édition live d'`app.js` sans recompiler) ; absent → assets scellés (release).

use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::path::Path;

/// `public/` figé dans le binaire à la compilation (relatif au crate root).
#[derive(RustEmbed)]
#[folder = "public/"]
pub struct Assets;

/// Résout un asset par chemin de requête HTTP.
/// - `disk_root = Some(dir)` (dev, `TALOS_PUBLIC` posé) → lit le disque sous `dir`.
/// - `disk_root = None` (release) → sert l'asset SCELLÉ dans le binaire.
///   `None` de retour = asset absent (→ l'appelant répond 404).
pub fn resolve(disk_root: Option<&Path>, path: &str) -> Option<Cow<'static, [u8]>> {
    let rel = normalize(path);
    match disk_root {
        Some(root) => std::fs::read(root.join(&rel)).ok().map(Cow::Owned),
        None => Assets::get(&rel).map(|f| f.data),
    }
}

/// Normalise un chemin de requête en clé d'asset : `""`/`"/"` → `index.html`,
/// et retire le slash de tête (les clés rust-embed sont relatives, sans `/`).
/// Public pour que l'appelant déduise le Content-Type sur la clé RÉELLE (sinon `/`
/// n'a pas d'extension → mauvais mime pour l'index).
pub fn normalize(path: &str) -> String {
    let p = path.trim_start_matches('/');
    if p.is_empty() {
        "index.html".to_string()
    } else {
        p.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_index_and_app() {
        // Le binaire DOIT contenir les assets front scellés (le cœur du raccourci #3).
        assert!(Assets::get("index.html").is_some(), "index.html non scellé");
        assert!(Assets::get("app.js").is_some(), "app.js non scellé");
        assert!(Assets::get("vendor/xterm.js").is_some(), "vendor/ non scellé");
    }

    #[test]
    fn resolve_embedded_returns_bytes() {
        let bytes = resolve(None, "app.js").expect("app.js scellé");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn empty_and_slash_map_to_index() {
        assert_eq!(normalize(""), "index.html");
        assert_eq!(normalize("/"), "index.html");
        assert_eq!(normalize("/app.js"), "app.js");
        assert_eq!(normalize("vendor/xterm.js"), "vendor/xterm.js");
    }

    #[test]
    fn resolve_disk_override_reads_from_dir() {
        // cwd = crate root pendant `cargo test` → public/ existe sur disque.
        // Disque et scellé doivent CONCORDER (même source).
        let disk = resolve(Some(Path::new("public")), "index.html").expect("disque");
        let sealed = resolve(None, "index.html").expect("scellé");
        assert_eq!(disk, sealed);
    }

    #[test]
    fn resolve_missing_returns_none() {
        assert!(resolve(None, "does-not-exist.xyz").is_none());
        assert!(resolve(Some(Path::new("public")), "nope.xyz").is_none());
    }
}
