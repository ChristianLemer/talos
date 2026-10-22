//! relocate.rs — the engine does not run from the share.
//!
//! On Windows a running `.exe` is LOCKED. Talos is launched straight from a OneDrive share,
//! so a publication that replaced `Talos.exe` while anyone had Talos open could not land:
//! OneDrive kept both (`Talos (1).exe`), then deleted — observed on a corporate machine on
//! 2026-10-22, with no copy left at all.
//!
//! So a release build on Windows that finds itself started from anywhere but its own local
//! engine folder copies itself there and hands over: the share's exe is held open for the
//! length of a launch, not of a session. ONE binary, no second launcher and no `engine/`
//! folder: the kit layout every script draws stays exactly what it was.
//!
//! The local copy is named after its own bytes — `Talos-<tag>-<sha256[..12]>.exe` — so the
//! name IS the integrity check and no manifest is consulted: the core never learns which file
//! an integrator publishes beside the kit. A copy goes through `.partial` + rename, so a file
//! that exists under its final name is whole.
//!
//! Every failure — the copy, the rename, the spawn (a policy refusing executables under
//! `AppData\Local` is refused by CreateProcess, synchronously) — falls back to running IN
//! PLACE, which is what every build did before. Never worse than before.
//!
//! The pure parts (the decision, the name, the arguments) are tested anywhere; the IO is
//! small and tested against temp folders. The spawn itself is measured on Windows.

use crate::platform::Os;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The flag the relocated engine is started with: the folder the share's exe sat in. It
/// stands for `exe_sibling()` everywhere — content, `behaviour/`, `logs/`, the title.
pub const KIT_FLAG: &str = "--kit";

/// The escape hatch: set to anything, the engine runs where it was started.
pub const IN_PLACE_ENV: &str = "TALOS_IN_PLACE";

/// The command line, with `--kit <dir>` taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArgs {
    pub kit: Option<PathBuf>,
    pub rest: Vec<String>,
}

/// Take `--kit <dir>` out of the arguments. A `--kit` with nothing after it is dropped, not
/// read as a kit: an empty path would resolve to the cwd and quietly adopt it.
pub fn split_kit(args: &[String]) -> LaunchArgs {
    let mut kit = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == KIT_FLAG {
            kit = it.next().map(PathBuf::from);
        } else {
            rest.push(a.clone());
        }
    }
    LaunchArgs { kit, rest }
}

/// Should this process relocate? `Ok` = yes; `Err(reason)` = run in place, and why.
/// `exe` already inside `engine_dir` is a local copy started by hand: relocating it would
/// copy it onto itself.
pub fn relocation(
    os: Os,
    release: bool,
    args: &LaunchArgs,
    in_place_env: bool,
    exe: &Path,
    engine_dir: &Path,
) -> Result<(), &'static str> {
    if os != Os::Windows {
        return Err("not Windows: only an .exe is locked while it runs");
    }
    if !release {
        return Err("a debug build runs where it was built");
    }
    if args.rest.first().map(String::as_str) == Some("--check") {
        return Err("--check is a command-line read, not a session");
    }
    if in_place_env {
        return Err("TALOS_IN_PLACE is set");
    }
    if args.kit.is_some() {
        return Err("already started with --kit");
    }
    if exe.parent() == Some(engine_dir) {
        return Err("already the local engine");
    }
    Ok(())
}

/// `Talos-<tag>-<first 12 hex of the sha256>.exe`. Anything in the tag outside
/// `[A-Za-z0-9._-]` becomes `_`, so no tag can put a separator in a file name.
pub fn local_name(tag: &str, digest_hex: &str) -> String {
    let tag: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let short = &digest_hex[..digest_hex.len().min(12)];
    format!("Talos-{tag}-{short}.exe")
}

/// The sha256 of a file, lowercase hex.
pub fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Put a copy of `exe` in `engine_dir` under its content name and return its path. Already
/// there → nothing is copied. Otherwise `.partial`, re-hash the copy, rename: a file under the
/// final name is always whole. A copy whose hash differs from the source (the share's file
/// replaced mid-copy) is refused, not launched.
pub fn install(exe: &Path, engine_dir: &Path, tag: &str) -> std::io::Result<PathBuf> {
    let digest = sha256_hex(exe)?;
    let target = engine_dir.join(local_name(tag, &digest));
    if target.is_file() {
        return Ok(target);
    }
    std::fs::create_dir_all(engine_dir)?;
    let partial = target.with_extension("exe.partial");
    std::fs::copy(exe, &partial)?;
    if sha256_hex(&partial)? != digest {
        let _ = std::fs::remove_file(&partial);
        return Err(std::io::Error::other(
            "the copy does not match the source: the exe changed while it was copied",
        ));
    }
    std::fs::rename(&partial, &target)?;
    Ok(target)
}

/// Keep `current` and the newest other engine; remove older engines and any `.partial` left
/// by an interrupted copy. Best-effort: an engine still running on this machine refuses to be
/// deleted, and it goes at a later launch. Touches `Talos-*` files only.
pub fn prune(engine_dir: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(engine_dir) else {
        return;
    };
    let mut engines = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("Talos-") || path == current {
            continue;
        }
        if name.ends_with(".partial") {
            let _ = std::fs::remove_file(&path);
        } else if name.ends_with(".exe") {
            let mtime = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            engines.push((mtime, path));
        }
    }
    engines.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (_, path) in engines.into_iter().skip(1) {
        let _ = std::fs::remove_file(path);
    }
}

/// Copy, prune, spawn the local engine with `--kit <kit>` and the original arguments.
/// `Ok(local)` = handed over, this process should exit; `Err(why)` = run in place.
/// `spawn` is injected so the sequence is tested without starting anything.
pub fn hand_over(
    exe: &Path,
    engine_dir: &Path,
    tag: &str,
    kit: &Path,
    rest: &[String],
    spawn: impl FnOnce(&Path, &[String]) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    let local = install(exe, engine_dir, tag).map_err(|e| format!("copy failed: {e}"))?;
    prune(engine_dir, &local);
    let mut args = vec![KIT_FLAG.to_string(), kit.display().to_string()];
    args.extend(rest.iter().cloned());
    spawn(&local, &args).map_err(|e| format!("{} refused to start: {e}", local.display()))?;
    Ok(local)
}

/// `main`'s entry: relocate if this launch should, and say whether this process must now
/// exit. The outcome lands in `<engine dir>/last-launch.txt` as well as on stdout, because a
/// release build on Windows has no console — that file is how a launch on a locked-down
/// machine says which way it went.
pub fn launch(os: Os, args: &LaunchArgs) -> bool {
    let engine_dir = crate::platform::local_data_dir(os).join("engine");
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let in_place_env = std::env::var_os(IN_PLACE_ENV).is_some();
    let release = !cfg!(debug_assertions);
    if let Err(why) = relocation(os, release, args, in_place_env, &exe, &engine_dir) {
        println!("[launch] in place: {why}");
        return false;
    }
    // From here on a relocation was ATTEMPTED, and only that is worth the file: the engine
    // it starts comes back through here with `--kit` and must not overwrite the outcome.
    let kit = crate::platform::exe_sibling_dir(&exe);
    let outcome = hand_over(
        &exe,
        &engine_dir,
        crate::build_info::TAG,
        &kit,
        &args.rest,
        spawn_detached,
    );
    let line = match &outcome {
        Ok(local) => format!("relocated: {} started {}", exe.display(), local.display()),
        Err(why) => format!("in place: {} ({why})", exe.display()),
    };
    println!("[launch] {line}");
    let _ = std::fs::create_dir_all(&engine_dir);
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _ = std::fs::write(
        engine_dir.join("last-launch.txt"),
        format!("{stamp} {line}\n"),
    );
    outcome.is_ok()
}

/// Start the local engine and do not wait for it. DETACHED + new process group on Windows,
/// so the engine outlives this process and owns no console.
fn spawn_detached(local: &Path, args: &[String]) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(local);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("talos-relocate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plain(rest: &[&str]) -> LaunchArgs {
        LaunchArgs {
            kit: None,
            rest: s(rest),
        }
    }

    #[test]
    fn the_kit_flag_is_taken_out_of_the_command_line() {
        let a = split_kit(&s(&["--kit", r"C:\Share\Kit", "--foo"]));
        assert_eq!(a.kit, Some(PathBuf::from(r"C:\Share\Kit")));
        assert_eq!(a.rest, s(&["--foo"]));
    }

    #[test]
    fn no_kit_flag_leaves_the_command_line_whole() {
        let a = split_kit(&s(&["--check", "c", "b"]));
        assert_eq!(a.kit, None);
        assert_eq!(a.rest, s(&["--check", "c", "b"]));
    }

    #[test]
    fn a_dangling_kit_flag_is_not_a_kit() {
        let a = split_kit(&s(&["--kit"]));
        assert_eq!(a.kit, None);
        assert!(a.rest.is_empty());
    }

    #[test]
    fn a_release_on_windows_launched_from_the_share_relocates() {
        let r = relocation(
            Os::Windows,
            true,
            &plain(&[]),
            false,
            Path::new(r"S:\Kit\Talos.exe"),
            Path::new(r"C:\Local\Talos\engine"),
        );
        assert_eq!(r, Ok(()));
    }

    #[test]
    fn every_other_case_runs_in_place_and_says_why() {
        let exe = Path::new(r"S:\Kit\Talos.exe");
        let eng = Path::new(r"C:\Local\Talos\engine");
        let w = Os::Windows;
        assert!(relocation(Os::Darwin, true, &plain(&[]), false, exe, eng).is_err());
        assert!(relocation(Os::Linux, true, &plain(&[]), false, exe, eng).is_err());
        assert!(relocation(w, false, &plain(&[]), false, exe, eng).is_err());
        assert!(relocation(w, true, &plain(&["--check"]), false, exe, eng).is_err());
        assert!(relocation(w, true, &plain(&[]), true, exe, eng).is_err());
        let started = LaunchArgs {
            kit: Some(PathBuf::from(r"S:\Kit")),
            rest: vec![],
        };
        assert!(relocation(w, true, &started, false, exe, eng).is_err());
        let local = eng.join("Talos-v1-abc.exe");
        assert!(relocation(w, true, &plain(&[]), false, &local, eng).is_err());
    }

    #[test]
    fn the_local_name_carries_the_tag_and_twelve_hex_of_the_hash() {
        let h = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            local_name("v0.0.1-beta.51", h),
            "Talos-v0.0.1-beta.51-0123456789ab.exe"
        );
    }

    #[test]
    fn a_tag_never_puts_a_path_separator_in_the_name() {
        let n = local_name(r"v1/dirty\x y", "ffffffffffffffff");
        assert_eq!(n, "Talos-v1_dirty_x_y-ffffffffffff.exe");
    }

    #[test]
    fn sha256_of_a_known_content() {
        let d = tmp("sha");
        let f = d.join("abc");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_hex(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn install_copies_once_under_the_content_name() {
        let d = tmp("install");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine bytes").unwrap();
        let eng = d.join("engine");
        let first = install(&exe, &eng, "v1").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"engine bytes");
        assert!(first
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Talos-v1-"));
        // No partial left behind.
        let names: Vec<_> = std::fs::read_dir(&eng).unwrap().collect();
        assert_eq!(names.len(), 1);
        // Same bytes → same file, untouched.
        let again = install(&exe, &eng, "v1").unwrap();
        assert_eq!(first, again);
    }

    #[test]
    fn new_bytes_under_the_same_tag_get_a_new_file() {
        let d = tmp("dirty");
        let exe = d.join("Talos.exe");
        let eng = d.join("engine");
        std::fs::write(&exe, b"one").unwrap();
        let a = install(&exe, &eng, "v1-dirty").unwrap();
        std::fs::write(&exe, b"two").unwrap();
        let b = install(&exe, &eng, "v1-dirty").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn prune_keeps_the_current_and_the_newest_other() {
        let d = tmp("prune");
        let t0 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let mk = |name: &str, age: u64| {
            let p = d.join(name);
            std::fs::write(&p, b"x").unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(t0 + std::time::Duration::from_secs(age))
                .unwrap();
            p
        };
        let oldest = mk("Talos-a-1.exe", 1);
        let older = mk("Talos-b-2.exe", 2);
        let newer = mk("Talos-c-3.exe", 3);
        let current = mk("Talos-d-0.exe", 0); // just launched, but an old mtime: kept anyway
        let stale = mk("Talos-e-9.exe.partial", 9);
        let other = mk("last-launch.txt", 0);
        prune(&d, &current);
        assert!(current.exists());
        assert!(newer.exists());
        assert!(!older.exists());
        assert!(!oldest.exists());
        assert!(!stale.exists(), "an interrupted copy is litter");
        assert!(other.exists(), "prune touches engines only");
    }

    #[test]
    fn the_local_engine_is_started_with_the_kit_then_the_original_arguments() {
        let d = tmp("handover");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine").unwrap();
        let eng = d.join("engine");
        let kit = d.join("share");
        let mut seen = None;
        let local = hand_over(&exe, &eng, "v1", &kit, &s(&["--x"]), |p, a| {
            seen = Some((p.to_path_buf(), a.to_vec()));
            Ok(())
        })
        .unwrap();
        let (p, a) = seen.unwrap();
        assert_eq!(p, local);
        assert!(local.starts_with(&eng));
        assert_eq!(
            a,
            vec!["--kit".to_string(), kit.display().to_string(), "--x".into()]
        );
    }

    #[test]
    fn a_refused_spawn_means_run_in_place_and_says_why() {
        let d = tmp("refused");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine").unwrap();
        let r = hand_over(&exe, &d.join("engine"), "v1", &d, &[], |_, _| {
            Err(std::io::Error::other("blocked by policy"))
        });
        assert!(r.unwrap_err().contains("blocked by policy"));
    }

    #[test]
    fn a_failed_copy_never_spawns() {
        let d = tmp("nocopy");
        let mut spawned = false;
        let r = hand_over(
            &d.join("absent.exe"),
            &d.join("engine"),
            "v1",
            &d,
            &[],
            |_, _| {
                spawned = true;
                Ok(())
            },
        );
        assert!(r.is_err());
        assert!(!spawned);
    }
}
