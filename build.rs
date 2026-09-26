//! build.rs — calls tauri_build AND bakes the BUILD STAMP into the binary. Answers
//! "which binary is actually running?" — the question that cost a debugging detour
//! when a stale exe lingered on the share. Queries git AT COMPILE TIME and exposes
//! 3 env vars the code reads via env!(): TALOS_BUILD_TAG, TALOS_BUILD_SHA, TALOS_BUILD_AT.
//!
//! Git only. The stamp used to lead with a jj change id (stable across amends) and fall
//! back to change="git" when jj was absent — which is exactly what every release showed,
//! since CI builds from a plain git clone: the title printed the name of the fallback.
//! The repo left jj on 2026-09-05; the git-native answer to "is this build the commit
//! it claims?" is `--dirty` on the describe string, so that is what the tag carries now.
//! Re-stamps on the cargo default: when a crate file changes (so after any real
//! rebuild). A commit WITHOUT an edit does not re-stamp → rebuild.

use std::process::Command;

fn main() {
    tauri_build::build();

    let sha = resolve_sha();
    let tag = resolve_tag();
    // ISO 8601 UTC build timestamp. SOURCE_DATE_EPOCH honored if present
    // (reproducible builds), otherwise the current time.
    let built_at = build_timestamp();

    println!("cargo:rustc-env=TALOS_BUILD_SHA={sha}");
    println!("cargo:rustc-env=TALOS_BUILD_TAG={tag}");
    println!("cargo:rustc-env=TALOS_BUILD_AT={built_at}");
}

/// Resolves the RELEASE TAG this binary was built from — the human-facing version
/// (v0.0.1-beta.4), distinct from the exact snapshot (sha).
///   1. Release CI checks out the tag itself → GITHUB_REF_NAME is authoritative.
///   2. Local / branch build → `git describe --tags --dirty` = nearest tag, with a
///      `-<n>-g<sha>` suffix when HEAD is ahead of it and `-dirty` when the working
///      tree has uncommitted edits. "untagged" if no tag is reachable.
fn resolve_tag() -> String {
    if std::env::var("GITHUB_REF_TYPE").as_deref() == Ok("tag") {
        if let Ok(t) = std::env::var("GITHUB_REF_NAME") {
            if !t.trim().is_empty() {
                return t.trim().to_string();
            }
        }
    }
    if let Some(desc) = run("git", &["describe", "--tags", "--dirty"]) {
        let desc = desc.trim().to_string();
        if !desc.is_empty() {
            return desc;
        }
    }
    "untagged".to_string()
}

/// Short HEAD sha; "unknown" outside a git checkout (a source tarball, say).
fn resolve_sha() -> String {
    run("git", &["rev-parse", "--short", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
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
