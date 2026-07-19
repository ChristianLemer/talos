//! Front assets SEALED in the binary (shortcut #3 of the spike).
//!
//! The spike served `public/` via a RELATIVE path (`TALOS_PUBLIC`, default `"public"`),
//! read at runtime by `ServeDir`/`read_to_string`. Launched by Finder/Explorer, the cwd
//! is NOT the project root → `public/` not found → `<h1>index.html introuvable</h1>`.
//!
//! Here we embed `public/` INTO the binary at compile time (like
//! `deno compile --include public`), relative to the crate root — thus independent of the
//! cwd at runtime. `TALOS_PUBLIC` survives as a DEV escape hatch: present → we read the
//! disk (live editing of `app.js` without recompiling); absent → sealed assets (release).

use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::path::Path;

/// `public/` frozen into the binary at compile time (relative to the crate root).
#[derive(RustEmbed)]
#[folder = "public/"]
pub struct Assets;

/// Resolves an asset by HTTP request path.
/// - `disk_root = Some(dir)` (dev, `TALOS_PUBLIC` set) → reads the disk under `dir`.
/// - `disk_root = None` (release) → serves the asset SEALED in the binary.
///   A `None` return = asset absent (→ the caller responds 404).
pub fn resolve(disk_root: Option<&Path>, path: &str) -> Option<Cow<'static, [u8]>> {
    let rel = normalize(path);
    match disk_root {
        Some(root) => std::fs::read(root.join(&rel)).ok().map(Cow::Owned),
        None => Assets::get(&rel).map(|f| f.data),
    }
}

/// Normalizes a request path into an asset key: `""`/`"/"` → `index.html`,
/// and strips the leading slash (rust-embed keys are relative, without `/`).
/// Public so the caller can deduce the Content-Type from the REAL key (otherwise `/`
/// has no extension → wrong mime for the index).
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
        // The binary MUST contain the sealed front assets (the heart of shortcut #3).
        assert!(Assets::get("index.html").is_some(), "index.html not sealed");
        assert!(Assets::get("app.js").is_some(), "app.js not sealed");
        assert!(Assets::get("vendor/xterm.js").is_some(), "vendor/ not sealed");
    }

    #[test]
    fn resolve_embedded_returns_bytes() {
        let bytes = resolve(None, "app.js").expect("app.js sealed");
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
        // cwd = crate root during `cargo test` → public/ exists on disk.
        // Disk and sealed must MATCH (same source).
        let disk = resolve(Some(Path::new("public")), "index.html").expect("disk");
        let sealed = resolve(None, "index.html").expect("sealed");
        assert_eq!(disk, sealed);
    }

    #[test]
    fn resolve_missing_returns_none() {
        assert!(resolve(None, "does-not-exist.xyz").is_none());
        assert!(resolve(Some(Path::new("public")), "nope.xyz").is_none());
    }
}
