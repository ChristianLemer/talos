//! build.rs — appelle tauri_build ET bake le STAMP DE BUILD dans le binaire
//! (pendant Rust de scripts/gen-build-info.ts). Répond à "quel binaire tourne
//! vraiment ?" — la question qui a coûté un détour de debug quand un exe périmé
//! traînait sur le partage. Interroge le VCS À LA COMPILATION et expose 3 variables
//! d'env que le code lit via env!() : TALOS_BUILD_CHANGE, TALOS_BUILD_SHA, TALOS_BUILD_AT.
//!
//! Ordre de résolution (gracieux — la a corporate network fait `git pull`, un clone GIT, pas jj) :
//!   jj (change_id + commit_id)  →  git (sha, change="git")  →  "unknown".
//! Re-stampe au défaut cargo : quand un fichier du crate change (donc après tout
//! vrai rebuild). Un amend/squash SANS édition ne re-stampe pas → rebuild (la
//! mémoire talos-build-stamp le dit). Le sha jj bouge à chaque snapshot d'édition ;
//! le change id reste stable across amends — d'où les deux.

use std::process::Command;

fn main() {
    tauri_build::build();

    let (change, sha) = resolve_vcs();
    // Timestamp de build ISO 8601 UTC (comme builtAt du TS). SOURCE_DATE_EPOCH
    // respecté si présent (builds reproductibles), sinon l'heure courante.
    let built_at = build_timestamp();

    println!("cargo:rustc-env=TALOS_BUILD_CHANGE={change}");
    println!("cargo:rustc-env=TALOS_BUILD_SHA={sha}");
    println!("cargo:rustc-env=TALOS_BUILD_AT={built_at}");
}

/// Interroge le VCS pour (change, sha). jj d'abord (change_id stable + commit_id) ;
/// à défaut git (sha seul, change="git") ; à défaut ("unknown","unknown").
fn resolve_vcs() -> (String, String) {
    if let Some(pair) = try_jj() {
        return pair;
    }
    if let Some(sha) = try_git() {
        return ("git".to_string(), sha);
    }
    ("unknown".to_string(), "unknown".to_string())
}

/// jj : deux templates courts. change_id = id stable across amends (mène, comme
/// `jj log`) ; commit_id = git-sha (change à chaque snapshot d'édition).
fn try_jj() -> Option<(String, String)> {
    let change = run("jj", &["log", "-r", "@", "--no-graph", "-T", "change_id.shortest(8)"])?;
    let sha = run("jj", &["log", "-r", "@", "--no-graph", "-T", "commit_id.shortest(8)"])?;
    let (change, sha) = (change.trim().to_string(), sha.trim().to_string());
    if change.is_empty() || sha.is_empty() {
        return None;
    }
    Some((change, sha))
}

/// git fallback : sha court de HEAD (une machine qui a `git pull` mais pas jj).
fn try_git() -> Option<String> {
    let sha = run("git", &["rev-parse", "--short", "HEAD"])?;
    let sha = sha.trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// Lance une commande, retourne stdout si exit 0, None sinon (binaire absent,
/// hors dépôt…). Ne panique JAMAIS — un stamp manquant ne doit pas couler le build.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if out.status.success() {
        String::from_utf8(out.stdout).ok()
    } else {
        None
    }
}

/// Timestamp de build ISO 8601 UTC. Respecte SOURCE_DATE_EPOCH (secondes Unix) si
/// posé — les systèmes de build reproductibles l'injectent — sinon l'heure courante.
fn build_timestamp() -> String {
    if let Some(iso) = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
    {
        return iso.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    }
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
