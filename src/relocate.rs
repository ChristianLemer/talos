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
//! The local copy is named after its own bytes — `Talos-<tag>-<sha256[..12]>.exe` — and an
//! existing one is re-hashed before it runs, so no manifest is consulted: the core never learns
//! which file an integrator publishes beside the kit. A copy goes through a per-launch
//! `.partial` + rename.
//!
//! Every failure — the copy, the rename, the spawn (a policy refusing executables under
//! `AppData\Local` is refused by CreateProcess, synchronously), an engine that exits within
//! two seconds — falls back to running IN PLACE, which is what every build did before. What
//! it cannot see is an engine that dies LATER than that; `last-launch.txt` then says
//! "relocated" and the person relaunches.
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
/// read as a kit: an empty path would resolve to the cwd and quietly adopt it. A relative kit
/// is made absolute against the cwd at launch: a sidecar's `{dir}` and a step terminal's cwd
/// are built from it, and a terminal resolving a relative folder lands in System32.
pub fn split_kit(args: &[String]) -> LaunchArgs {
    let mut kit = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == KIT_FLAG {
            kit = it
                .next()
                .map(|k| std::path::absolute(k).unwrap_or_else(|_| PathBuf::from(k)));
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
    if exe.parent().is_some_and(|p| same_path(p, engine_dir)) {
        return Err("already the local engine");
    }
    // Neither LOCALAPPDATA nor USERPROFILE: the folder is relative, and under an Explorer
    // launch the cwd is the share — the copy would land there and sync to everyone.
    if !engine_dir.is_absolute() {
        return Err("no local data folder: the environment names none");
    }
    Ok(())
}

/// Two paths name the same folder, ignoring case — Windows paths are case-insensitive, and
/// `current_exe` returns the spelling a shortcut or an 8.3 name launched with. Ignoring case
/// on every OS is the safe side here: it can only make a launch run in place.
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    norm(a) == norm(b)
}

/// `a` is `dir` or inside it, ignoring case (see `same_path`).
fn within(a: &Path, dir: &Path) -> bool {
    a.ancestors().any(|p| same_path(p, dir))
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

/// Put a copy of `exe` in `engine_dir` under its content name and return its path.
///
/// A file already under that name is RE-HASHED before it is trusted (one local read): the
/// name says what the bytes should be, and only the hash says they are. Otherwise the copy
/// goes to `<name>.<pid>.partial` — one per launch, so two double-clicks never write the same
/// file — is re-hashed, and renamed. A copy whose hash differs from the source (the share's
/// file replaced mid-copy) is refused, not launched. If the rename loses to another launch
/// that got there first, its file is as good as ours: same name, same hash.
pub fn install(exe: &Path, engine_dir: &Path, tag: &str) -> std::io::Result<PathBuf> {
    let digest = sha256_hex(exe)?;
    let target = engine_dir.join(local_name(tag, &digest));
    let whole = |p: &Path| p.is_file() && sha256_hex(p).is_ok_and(|h| h == digest);
    if whole(&target) {
        return Ok(target);
    }
    std::fs::create_dir_all(engine_dir)?;
    let partial = target.with_extension(format!("exe.{}.partial", std::process::id()));
    let copied = std::fs::copy(exe, &partial)
        .and_then(|_| writable(&partial))
        .and_then(|_| sha256_hex(&partial));
    match copied {
        Ok(h) if h == digest => {}
        Ok(_) => {
            let _ = std::fs::remove_file(&partial);
            return Err(std::io::Error::other(
                "the copy does not match the source: the exe changed while it was copied",
            ));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            return Err(e);
        }
    }
    if std::fs::rename(&partial, &target).is_err() {
        if whole(&target) {
            let _ = std::fs::remove_file(&partial);
            return Ok(target);
        }
        // The target can be neither verified nor replaced — on Windows, a running exe that an
        // antivirus holds while it scans. A second name beats falling back to the share, which
        // is the very thing this module exists to avoid. `prune` collects it like any engine.
        let stem = target.file_stem().unwrap_or_default().to_string_lossy();
        let alt = target.with_file_name(format!("{stem}-{}.exe", std::process::id()));
        if let Err(e) = std::fs::rename(&partial, &alt) {
            let _ = std::fs::remove_file(&partial);
            return Err(e);
        }
        return Ok(alt);
    }
    Ok(target)
}

/// Clear a read-only attribute the copy carried over from the share (a view-only library may
/// set one): a read-only engine could not be pruned or replaced later.
fn writable(path: &Path) -> std::io::Result<()> {
    let mut perm = std::fs::metadata(path)?.permissions();
    if perm.readonly() {
        // Owner write only: `set_readonly(false)` on Unix would make it world-writable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perm.set_mode(perm.mode() | 0o200);
        }
        #[cfg(not(unix))]
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

/// Keep `current` and the newest other engine (by modification time, which a Windows copy
/// carries over from the source: the newest BUILD, not the latest use); remove older engines.
/// Best-effort: an engine still running on this machine refuses to be deleted, and it goes at
/// a later launch. Touches `Talos-*.exe` files only.
///
/// ⚠️ `.partial` files are NOT pruned: one may belong to a launch copying at this very moment,
/// and every failure path in `install` removes its own. Only a launch killed mid-copy leaves
/// one behind, and that is litter, not harm. Two launches of DIFFERENT versions at the same
/// moment can still prune each other's freshly verified engine before it starts: that launch's
/// spawn fails and it runs in place — rare, and never unsafe.
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
        if name.ends_with(".exe") {
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

/// Where the last relocation came from. A local engine started WITHOUT `--kit` — pinned to the
/// taskbar from its running window, or double-clicked in AppData — reads it back, instead of
/// resolving content beside itself, where there is none.
fn last_kit_path(engine_dir: &Path) -> PathBuf {
    engine_dir.join("last-kit.txt")
}

/// Record the kit a relocation was made from. Best-effort.
pub fn remember_kit(engine_dir: &Path, kit: &Path) {
    let _ = std::fs::create_dir_all(engine_dir);
    let _ = std::fs::write(
        last_kit_path(engine_dir),
        kit.as_os_str().to_string_lossy().as_bytes(),
    );
}

/// The kit last relocated from, if it is still a folder.
pub fn remembered_kit(engine_dir: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(last_kit_path(engine_dir)).ok()?;
    let kit = PathBuf::from(raw.trim_end());
    kit.is_dir().then_some(kit)
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
    spawn: impl FnOnce(&Path, &[std::ffi::OsString]) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    // A kit inside the engine folder is the engine pointing at itself (a path spelled so that
    // `relocation` missed it): no content there, and remembering it would empty every later
    // launch started by hand.
    if within(kit, engine_dir) {
        return Err(format!("the kit {} is the engine folder", kit.display()));
    }
    let local = install(exe, engine_dir, tag).map_err(|e| format!("copy failed: {e}"))?;
    prune(engine_dir, &local);
    let mut args: Vec<std::ffi::OsString> = vec![KIT_FLAG.into(), kit.as_os_str().to_owned()];
    args.extend(rest.iter().map(std::ffi::OsString::from));
    spawn(&local, &args).map_err(|e| format!("{} refused to start: {e}", local.display()))?;
    remember_kit(engine_dir, kit);
    Ok(local)
}

/// What `main` does after `launch`.
#[derive(Debug, PartialEq, Eq)]
pub enum Launched {
    /// A local engine was started: this process exits.
    HandedOver,
    /// Run here. `Some(kit)` = a local engine started by hand, pointed back at the kit it was
    /// last relocated from.
    Here(Option<PathBuf>),
}

/// `main`'s entry: relocate if this launch should. The outcome of an attempt lands in
/// `<engine dir>/last-launch.txt` as well as on stdout, because a release build on Windows has
/// no console — that file is how a launch on a locked-down machine says which way it went.
pub fn launch(os: Os, args: &LaunchArgs) -> Launched {
    let engine_dir = crate::platform::local_data_dir(os).join("engine");
    let Ok(exe) = std::env::current_exe() else {
        return Launched::Here(None);
    };
    let in_place_env = std::env::var_os(IN_PLACE_ENV).is_some();
    let release = !cfg!(debug_assertions);
    if let Err(why) = relocation(os, release, args, in_place_env, &exe, &engine_dir) {
        println!("[launch] in place: {why}");
        let by_hand = args.kit.is_none() && exe.parent() == Some(engine_dir.as_path());
        return Launched::Here(by_hand.then(|| remembered_kit(&engine_dir)).flatten());
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
    match outcome {
        Ok(_) => Launched::HandedOver,
        Err(_) => Launched::Here(None),
    }
}

/// Start the local engine, DETACHED + new process group on Windows so it outlives this
/// process and owns no console, in its own folder so it holds no directory handle on the share
/// (the kit comes from `--kit`; the cwd is only `content_dirs_in`'s last fallback).
///
/// Then watch it for `ALIVE_CHECK`: an engine that exits at once — a security product killing a
/// new unsigned exe after it started, a crash only the local copy hits — is a failure, and the
/// launch runs in place instead of leaving nothing on screen. The share's exe is held that much
/// longer; the engine is starting in parallel, so the person waits for nothing.
fn spawn_detached(local: &Path, args: &[std::ffi::OsString]) -> std::io::Result<()> {
    const ALIVE_CHECK: std::time::Duration = std::time::Duration::from_secs(2);
    let mut cmd = std::process::Command::new(local);
    cmd.args(args);
    if let Some(dir) = local.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = cmd.spawn()?;
    let deadline = std::time::Instant::now() + ALIVE_CHECK;
    while std::time::Instant::now() < deadline {
        // A `try_wait` that errors says nothing about the child, which may well be running:
        // treat it as alive rather than open a second Talos in place.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(std::io::Error::other(format!(
                "it exited at once ({status})"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
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

    fn abs_engine() -> PathBuf {
        std::env::temp_dir().join("Talos").join("engine")
    }

    fn plain(rest: &[&str]) -> LaunchArgs {
        LaunchArgs {
            kit: None,
            rest: s(rest),
        }
    }

    #[test]
    fn the_kit_flag_is_taken_out_of_the_command_line() {
        let kit = std::env::temp_dir().join("Share").join("Kit");
        let a = split_kit(&s(&["--kit", &kit.to_string_lossy(), "--foo"]));
        assert_eq!(a.kit, Some(kit));
        assert_eq!(a.rest, s(&["--foo"]));
    }

    #[test]
    fn no_kit_flag_leaves_the_command_line_whole() {
        let a = split_kit(&s(&["--check", "c", "b"]));
        assert_eq!(a.kit, None);
        assert_eq!(a.rest, s(&["--check", "c", "b"]));
    }

    #[test]
    fn a_relative_kit_is_made_absolute() {
        // Every consumer of the kit — a sidecar's `{dir}`, a step terminal's cwd — needs an
        // absolute folder: a relative one resolves against whatever cwd a terminal starts in.
        let a = split_kit(&s(&["--kit", "share"]));
        let kit = a.kit.unwrap();
        assert!(kit.is_absolute(), "{}", kit.display());
        assert!(kit.ends_with("share"));
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
            &abs_engine(),
        );
        assert_eq!(r, Ok(()));
    }

    #[test]
    fn every_other_case_runs_in_place_and_says_why() {
        let exe = Path::new(r"S:\Kit\Talos.exe");
        let eng = &abs_engine();
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
        // No LOCALAPPDATA nor USERPROFILE → a RELATIVE engine dir, which under an Explorer
        // launch is the share itself: the copy would land there and sync to everyone.
        let relative = Path::new(r"AppData\Local\Talos\engine");
        assert!(relocation(w, true, &plain(&[]), false, exe, relative).is_err());
    }

    #[test]
    fn the_local_engine_is_recognised_whatever_the_case_of_its_path() {
        // Windows paths are case-insensitive; `current_exe` returns the spelling it was
        // launched with (a shortcut, an 8.3 name), `engine_dir` the one LOCALAPPDATA gives.
        let eng = abs_engine();
        let upper = PathBuf::from(eng.to_string_lossy().to_uppercase()).join("Talos-v1-abc.exe");
        assert!(relocation(Os::Windows, true, &plain(&[]), false, &upper, &eng).is_err());
    }

    #[test]
    fn a_kit_inside_the_engine_folder_is_refused_and_never_remembered() {
        let d = tmp("selfkit");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine").unwrap();
        let eng = d.join("engine");
        let mut spawned = false;
        let r = hand_over(&exe, &eng, "v1", &eng, &[], |_, _| {
            spawned = true;
            Ok(())
        });
        assert!(r.is_err());
        assert!(!spawned);
        assert_eq!(remembered_kit(&eng), None);
    }

    #[test]
    fn a_target_that_cannot_be_replaced_gets_a_second_name() {
        // A local engine that cannot be verified AND cannot be replaced — on Windows, a running
        // exe an antivirus holds while it scans. A directory stands in for it here: no rename
        // lands on it. The launch must still relocate, never fall back to holding the share.
        let d = tmp("unreplaceable");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine bytes").unwrap();
        let eng = d.join("engine");
        let digest = sha256_hex(&exe).unwrap();
        let blocked = eng.join(local_name("v1", &digest));
        std::fs::create_dir_all(blocked.join("busy")).unwrap();
        let local = install(&exe, &eng, "v1").unwrap();
        assert_ne!(local, blocked);
        assert_eq!(std::fs::read(&local).unwrap(), b"engine bytes");
    }

    #[test]
    fn a_read_only_source_gives_a_writable_local_engine() {
        let d = tmp("readonly");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine bytes").unwrap();
        let mut perm = std::fs::metadata(&exe).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&exe, perm).unwrap();
        let local = install(&exe, &d.join("engine"), "v1").unwrap();
        assert!(!std::fs::metadata(&local).unwrap().permissions().readonly());
    }

    #[test]
    fn a_corrupted_local_engine_is_copied_again() {
        let d = tmp("corrupt");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine bytes").unwrap();
        let eng = d.join("engine");
        let local = install(&exe, &eng, "v1").unwrap();
        std::fs::write(&local, b"tampered").unwrap();
        let again = install(&exe, &eng, "v1").unwrap();
        assert_eq!(again, local);
        assert_eq!(std::fs::read(&again).unwrap(), b"engine bytes");
    }

    #[test]
    fn another_launch_copying_at_the_same_moment_is_left_alone() {
        let d = tmp("concurrent");
        let exe = d.join("Talos.exe");
        std::fs::write(&exe, b"engine bytes").unwrap();
        let eng = d.join("engine");
        std::fs::create_dir_all(&eng).unwrap();
        let digest = sha256_hex(&exe).unwrap();
        let theirs = eng.join(format!("{}.1.partial", local_name("v1", &digest)));
        std::fs::write(&theirs, b"half").unwrap();
        let local = install(&exe, &eng, "v1").unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), b"engine bytes");
        assert_eq!(
            std::fs::read(&theirs).unwrap(),
            b"half",
            "not ours to touch"
        );
    }

    #[test]
    fn the_kit_is_remembered_for_a_local_engine_started_by_hand() {
        let d = tmp("remember");
        let eng = d.join("engine");
        let kit = d.join("share");
        std::fs::create_dir_all(&kit).unwrap();
        assert_eq!(remembered_kit(&eng), None);
        remember_kit(&eng, &kit);
        assert_eq!(remembered_kit(&eng), Some(kit.clone()));
        // A kit that has gone away is not adopted.
        std::fs::remove_dir_all(&kit).unwrap();
        assert_eq!(remembered_kit(&eng), None);
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
        let inflight = mk("Talos-e-9.exe.42.partial", 9);
        let other = mk("last-launch.txt", 0);
        prune(&d, &current);
        assert!(current.exists());
        assert!(newer.exists());
        assert!(!older.exists());
        assert!(!oldest.exists());
        assert!(
            inflight.exists(),
            "a copy may be in flight in another launch"
        );
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
        let want: Vec<std::ffi::OsString> =
            vec!["--kit".into(), kit.clone().into_os_string(), "--x".into()];
        assert_eq!(a, want);
        std::fs::create_dir_all(&kit).unwrap();
        assert_eq!(remembered_kit(&eng), Some(kit));
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
