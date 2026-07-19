//! build.rs — calls tauri_build AND bakes the BUILD STAMP into the binary (the Rust
//! counterpart of scripts/gen-build-info.ts). Answers "which binary is actually
//! running?" — the question that cost a debugging detour when a stale exe lingered
//! on the share. Queries the VCS AT COMPILE TIME and exposes 3 env vars the code
//! reads via env!(): TALOS_BUILD_CHANGE, TALOS_BUILD_SHA, TALOS_BUILD_AT.
//!
//! Resolution order (graceful — deployment does `git pull`, a GIT clone, not jj):
//!   jj (change_id + commit_id)  →  git (sha, change="git")  →  "unknown".
//! Re-stamps on the cargo default: when a crate file changes (so after any real
//! rebuild). An amend/squash WITHOUT an edit does not re-stamp → rebuild (the
//! talos-build-stamp memory says so). The jj sha moves on every edit snapshot;
//! the change id stays stable across amends — hence both.

use std::process::Command;

fn main() {
    tauri_build::build();

    let (change, sha) = resolve_vcs();
    // ISO 8601 UTC build timestamp (like the TS builtAt). SOURCE_DATE_EPOCH honored
    // if present (reproducible builds), otherwise the current time.
    let built_at = build_timestamp();

    println!("cargo:rustc-env=TALOS_BUILD_CHANGE={change}");
    println!("cargo:rustc-env=TALOS_BUILD_SHA={sha}");
    println!("cargo:rustc-env=TALOS_BUILD_AT={built_at}");
}

/// Queries the VCS for (change, sha). jj first (stable change_id + commit_id);
/// else git (sha only, change="git"); else ("unknown","unknown").
fn resolve_vcs() -> (String, String) {
    if let Some(pair) = try_jj() {
        return pair;
    }
    if let Some(sha) = try_git() {
        return ("git".to_string(), sha);
    }
    ("unknown".to_string(), "unknown".to_string())
}

/// jj: two short templates. change_id = id stable across amends (leads, like
/// `jj log`); commit_id = git-sha (changes on every edit snapshot).
fn try_jj() -> Option<(String, String)> {
    let change = run("jj", &["log", "-r", "@", "--no-graph", "-T", "change_id.shortest(8)"])?;
    let sha = run("jj", &["log", "-r", "@", "--no-graph", "-T", "commit_id.shortest(8)"])?;
    let (change, sha) = (change.trim().to_string(), sha.trim().to_string());
    if change.is_empty() || sha.is_empty() {
        return None;
    }
    Some((change, sha))
}

/// git fallback: short HEAD sha (a machine that has `git pull` but not jj).
fn try_git() -> Option<String> {
    let sha = run("git", &["rev-parse", "--short", "HEAD"])?;
    let sha = sha.trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// Runs a command, returns stdout if exit 0, None otherwise (binary absent, outside
/// a repo…). NEVER panics — a missing stamp must not sink the build.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if out.status.success() {
        String::from_utf8(out.stdout).ok()
    } else {
        None
    }
}

/// ISO 8601 UTC build timestamp. Honors SOURCE_DATE_EPOCH (Unix seconds) if set —
/// reproducible-build systems inject it — otherwise the current time.
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
