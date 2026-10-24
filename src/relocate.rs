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
//! A copy that fails — a publication replacing the share's exe mid-copy — is tried once more,
//! then the newest local engine whose bytes match its name is started instead. Every other
//! failure — no engine to fall back on, the spawn (a policy refusing executables under
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

/// Two paths name the same folder. Each is canonicalized first when it exists — on Windows that
/// resolves an 8.3 name and a `\\?\` prefix, on Unix a symlink; only the filesystem knows two
/// spellings are one folder. Then case is ignored, for a path that does not exist yet and for
/// what canonicalization leaves (`current_exe` returns the spelling a shortcut launched with).
/// Ignoring case on every OS is the safe side here: it can only make a launch run in place.
fn same_path(a: &Path, b: &Path) -> bool {
    same_text(&canonical(a), &canonical(b))
}

/// `a` is `dir` or inside it (see `same_path`).
fn within(a: &Path, dir: &Path) -> bool {
    let dir = canonical(dir);
    canonical(a).ancestors().any(|p| same_text(p, &dir))
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn same_text(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    norm(a) == norm(b)
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

/// Where the last relocation came from: the kit, then the exe that relocated. A local engine
/// started WITHOUT `--kit` — pinned to the taskbar from its running window, or double-clicked in
/// AppData — reads the kit back instead of resolving content beside itself, where there is
/// none, and hands back to that exe when a publication has replaced it (`hand_back`).
fn last_kit_path(engine_dir: &Path) -> PathBuf {
    engine_dir.join("last-kit.txt")
}

/// Record the kit a relocation was made from, and the exe that made it — two lines. Best-effort.
pub fn remember_kit(engine_dir: &Path, kit: &Path, launcher: &Path) {
    let _ = std::fs::create_dir_all(engine_dir);
    let text = format!("{}\n{}\n", kit.display(), launcher.display());
    let _ = std::fs::write(last_kit_path(engine_dir), text);
}

/// Line `n` of `last-kit.txt`, if there is one.
fn remembered_line(engine_dir: &Path, n: usize) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(last_kit_path(engine_dir)).ok()?;
    let line = raw.lines().nth(n)?.trim_end();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// The kit last relocated from, if it is still a folder.
pub fn remembered_kit(engine_dir: &Path) -> Option<PathBuf> {
    remembered_line(engine_dir, 0).filter(|k| k.is_dir())
}

/// The exe the last relocation came from, if it is still a file. A file written before the
/// launcher was recorded has one line: no launcher.
pub fn remembered_launcher(engine_dir: &Path) -> Option<PathBuf> {
    remembered_line(engine_dir, 1).filter(|l| l.is_file())
}

/// A local engine started by hand: if the kit's exe is no longer this engine — a publication
/// replaced it since — start that exe instead (it relocates itself, the new version opens) and
/// return it; this process exits. `Ok(None)` = no launcher known, or the same bytes: run here.
/// `Err(why)` = the hand-back failed: run here anyway, an old version beats no window.
pub fn hand_back(
    exe: &Path,
    engine_dir: &Path,
    rest: &[String],
    spawn: impl FnOnce(&Path, &[std::ffi::OsString]) -> std::io::Result<()>,
) -> Result<Option<PathBuf>, String> {
    let Some(launcher) = remembered_launcher(engine_dir) else {
        return Ok(None);
    };
    let hash = |p: &Path| sha256_hex(p).map_err(|e| format!("cannot read {}: {e}", p.display()));
    if hash(&launcher)? == hash(exe)? {
        return Ok(None);
    }
    let args: Vec<std::ffi::OsString> = rest.iter().map(std::ffi::OsString::from).collect();
    spawn(&launcher, &args).map_err(|e| format!("{} refused to start: {e}", launcher.display()))?;
    Ok(Some(launcher))
}

/// A hand-over that happened: the engine started, and why it is not a fresh copy, if it is not.
#[derive(Debug)]
pub struct HandedOver {
    pub local: PathBuf,
    pub note: Option<String>,
}

/// Copy, prune, spawn the local engine with `--kit <kit>` and the original arguments.
/// `Ok` = handed over, this process should exit; `Err(why)` = run in place.
/// `spawn` is injected so the sequence is tested without starting anything.
pub fn hand_over(
    exe: &Path,
    engine_dir: &Path,
    tag: &str,
    kit: &Path,
    rest: &[String],
    spawn: impl FnOnce(&Path, &[std::ffi::OsString]) -> std::io::Result<()>,
) -> Result<HandedOver, String> {
    // A kit inside the engine folder is the engine pointing at itself (a path spelled so that
    // `relocation` missed it): no content there, and remembering it would empty every later
    // launch started by hand.
    if within(kit, engine_dir) {
        return Err(format!("the kit {} is the engine folder", kit.display()));
    }
    // A copy fails at exactly the moment a publication replaces the share's exe ("changed while
    // it was copied"). Running in place would then lock the share for the session at the very
    // moment the publication lands: try once more, then fall back to the newest local engine
    // whose bytes still match its name — an older version beats a locked share.
    let (local, note) = match retry_once(RETRY_PAUSE, || install(exe, engine_dir, tag)) {
        Ok(local) => (local, None),
        Err(e) => match newest_whole_engine(engine_dir) {
            Some(local) => {
                let note = format!("copy failed ({e}): the newest verified local engine instead");
                (local, Some(note))
            }
            None => return Err(format!("copy failed: {e}")),
        },
    };
    prune(engine_dir, &local);
    let mut args: Vec<std::ffi::OsString> = vec![KIT_FLAG.into(), kit.as_os_str().to_owned()];
    args.extend(rest.iter().map(std::ffi::OsString::from));
    spawn(&local, &args).map_err(|e| format!("{} refused to start: {e}", local.display()))?;
    remember_kit(engine_dir, kit, exe);
    Ok(HandedOver { local, note })
}

/// Long enough for OneDrive to finish writing an exe it is replacing.
const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);

/// `f`, and once more after `pause` if it failed.
fn retry_once<T>(
    pause: std::time::Duration,
    mut f: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    f().or_else(|_| {
        std::thread::sleep(pause);
        f()
    })
}

/// The newest `Talos-*.exe` in `engine_dir` whose sha256 still matches the 12 hex its name
/// carries (`Talos-<tag>-<hex>.exe`, or `…-<hex>-<pid>.exe` for a second name).
fn newest_whole_engine(engine_dir: &Path) -> Option<PathBuf> {
    let mut engines: Vec<_> = std::fs::read_dir(engine_dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name
                .strip_prefix("Talos-")?
                .strip_suffix(".exe")?
                .to_owned();
            let mtime = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((mtime, e.path(), stem))
        })
        .collect();
    engines.sort_by_key(|e| std::cmp::Reverse(e.0));
    engines.into_iter().find_map(|(_, path, stem)| {
        let hash = sha256_hex(&path).ok()?;
        let short = &hash[..12];
        stem.rsplit('-')
            .take(2)
            .any(|seg| seg == short)
            .then_some(path)
    })
}

/// A local engine started WITHOUT `--kit`: a taskbar pin, a double-click in AppData.
pub fn started_by_hand(args: &LaunchArgs, exe: &Path, engine_dir: &Path) -> bool {
    args.kit.is_none() && exe.parent().is_some_and(|p| same_path(p, engine_dir))
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
        if !started_by_hand(args, &exe, &engine_dir) {
            return Launched::Here(None);
        }
        let (line, launched) = match hand_back(&exe, &engine_dir, &args.rest, spawn_launcher) {
            Ok(None) => return Launched::Here(remembered_kit(&engine_dir)),
            Ok(Some(launcher)) => (
                format!(
                    "by hand: the kit's exe changed, started {}",
                    launcher.display()
                ),
                Launched::HandedOver,
            ),
            Err(why) => (
                format!("by hand: running {} ({why})", exe.display()),
                Launched::Here(remembered_kit(&engine_dir)),
            ),
        };
        record(&engine_dir, &line);
        return launched;
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
        Ok(h) => {
            let mut line = format!("relocated: {} started {}", exe.display(), h.local.display());
            if let Some(note) = &h.note {
                line.push_str(&format!(" ({note})"));
            }
            line
        }
        Err(why) => format!("in place: {} ({why})", exe.display()),
    };
    record(&engine_dir, &line);
    match outcome {
        Ok(_) => Launched::HandedOver,
        Err(_) => Launched::Here(None),
    }
}

/// The outcome of a launch, on stdout and in `<engine dir>/last-launch.txt`.
fn record(engine_dir: &Path, line: &str) {
    println!("[launch] {line}");
    let _ = std::fs::create_dir_all(engine_dir);
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _ = std::fs::write(
        engine_dir.join("last-launch.txt"),
        format!("{stamp} {line}\n"),
    );
}

/// Windows' ERROR_ACCESS_DENIED: what CreateProcess answers when the caller's job forbids
/// `CREATE_BREAKAWAY_FROM_JOB`.
const ACCESS_DENIED: i32 = 5;

/// Start with breakaway from the caller's job; if the job forbids it, start without.
/// `start(true)` = with breakaway. Any other failure is the answer.
fn with_breakaway<T>(mut start: impl FnMut(bool) -> std::io::Result<T>) -> std::io::Result<T> {
    match start(true) {
        Err(e) if e.raw_os_error() == Some(ACCESS_DENIED) => start(false),
        r => r,
    }
}

/// Start the local engine, DETACHED + new process group + out of the caller's job on Windows so
/// it outlives this process and owns no console, in its own folder so it holds no directory
/// handle on the share (the kit comes from `--kit`; the cwd is only `content_dirs_in`'s last
/// fallback).
///
/// Then watch it for `ALIVE_CHECK`: an engine that exits at once — a security product killing a
/// new unsigned exe after it started, a crash only the local copy hits — is a failure, and the
/// launch runs in place instead of leaving nothing on screen. The share's exe is held that much
/// longer; the engine is starting in parallel, so the person waits for nothing.
fn spawn_detached(local: &Path, args: &[std::ffi::OsString]) -> std::io::Result<()> {
    const ALIVE_CHECK: std::time::Duration = std::time::Duration::from_secs(2);
    let mut child = detached_child(local, args)?;
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

/// Start the kit's exe for a hand-back. No alive check: that exe relocates itself and EXITS
/// after its own, so an early exit is its normal course, not a failure.
fn spawn_launcher(launcher: &Path, args: &[std::ffi::OsString]) -> std::io::Result<()> {
    detached_child(launcher, args).map(drop)
}

/// `exe` started detached, with no standard handle, in its own folder.
fn detached_child(exe: &Path, args: &[std::ffi::OsString]) -> std::io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    // No inherited handle: `Talos.exe > \\share\log.txt` would keep the share open for the
    // session, a piped caller would wait for it. A release build has no console to write to.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    // Out of the caller's job when it allows it: a launcher started from a kill-on-close job
    // (an IDE terminal, a deployment agent) would otherwise take the engine down when it exits,
    // AFTER the alive check — "relocated" in the log, nothing on screen.
    with_breakaway(|breakaway| {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
            let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
            cmd.creation_flags(if breakaway {
                flags | CREATE_BREAKAWAY_FROM_JOB
            } else {
                flags
            });
        }
        #[cfg(not(windows))]
        let _ = breakaway;
        cmd.spawn()
    })
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
    fn a_local_engine_started_by_hand_is_recognised_whatever_the_case_of_its_path() {
        let eng = abs_engine();
        let upper = PathBuf::from(eng.to_string_lossy().to_uppercase()).join("Talos-v1-abc.exe");
        assert!(started_by_hand(&plain(&[]), &upper, &eng));
        assert!(!started_by_hand(
            &plain(&[]),
            Path::new(r"S:\Kit\Talos.exe"),
            &eng
        ));
        let relocated = LaunchArgs {
            kit: Some(PathBuf::from(r"S:\Kit")),
            rest: vec![],
        };
        assert!(!started_by_hand(&relocated, &upper, &eng));
    }

    #[cfg(unix)]
    #[test]
    fn the_engine_folder_is_recognised_under_another_spelling() {
        // On Windows an 8.3 name or a `\\?\` prefix spells the same folder differently; a
        // symlink is the same trap on Unix. Only the filesystem can say they are one folder.
        let d = tmp("spelling");
        let eng = d.join("engine");
        std::fs::create_dir_all(&eng).unwrap();
        let alias = d.join("alias");
        std::os::unix::fs::symlink(&eng, &alias).unwrap();
        let exe = alias.join("Talos-v1-abc.exe");
        std::fs::write(&exe, b"engine").unwrap();
        assert!(relocation(Os::Windows, true, &plain(&[]), false, &exe, &eng).is_err());
        assert!(started_by_hand(&plain(&[]), &exe, &eng));
        let r = hand_over(&exe, &eng, "v1", &alias, &[], |_, _| Ok(()));
        assert!(
            r.is_err(),
            "the kit is the engine folder under another name"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_local_engine_inherits_no_standard_handle() {
        // `Talos.exe > \\share\log.txt`, or a caller reading a pipe: an inherited handle keeps
        // the share open, or the caller waiting, for the whole session. The child stays alive
        // only if all three of its standard handles are /dev/null.
        let script = r#"for f in 0 1 2; do [ "$(readlink /proc/$$/fd/$f)" = /dev/null ] || exit 1; done; sleep 3"#;
        let args: Vec<std::ffi::OsString> = vec!["-c".into(), script.into()];
        let r = spawn_detached(Path::new("/bin/sh"), &args);
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn a_job_that_forbids_breakaway_gets_a_second_try_without_it() {
        let mut tries = Vec::new();
        let r = with_breakaway(|b| {
            tries.push(b);
            if b {
                Err(std::io::Error::from_raw_os_error(ACCESS_DENIED))
            } else {
                Ok("started")
            }
        });
        assert_eq!(r.unwrap(), "started");
        assert_eq!(tries, vec![true, false]);
    }

    #[test]
    fn any_other_spawn_failure_is_not_retried() {
        let mut tries = 0;
        let r: std::io::Result<()> = with_breakaway(|_| {
            tries += 1;
            Err(std::io::Error::other("blocked by policy"))
        });
        assert!(r.is_err());
        assert_eq!(tries, 1);
    }

    #[test]
    fn a_failed_install_is_tried_once_more() {
        let mut tries = 0;
        let r = retry_once(std::time::Duration::ZERO, || {
            tries += 1;
            if tries == 1 {
                Err(std::io::Error::other("the exe changed while it was copied"))
            } else {
                Ok("copied")
            }
        });
        assert_eq!(r.unwrap(), "copied");
        assert_eq!(tries, 2);
    }

    #[test]
    fn a_copy_that_cannot_be_made_starts_the_newest_verified_local_engine() {
        // A publication replacing the share's exe mid-copy: running in place would lock the
        // share for the session at exactly the moment a publication is landing.
        let d = tmp("publishing");
        let eng = d.join("engine");
        let old = d.join("old.exe");
        std::fs::write(&old, b"the previous engine").unwrap();
        let verified = install(&old, &eng, "v1").unwrap();
        let mut seen = None;
        let r = hand_over(
            &d.join("being-replaced.exe"),
            &eng,
            "v2",
            &d,
            &[],
            |p, _| {
                seen = Some(p.to_path_buf());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(r.local, verified);
        assert_eq!(seen, Some(verified));
        assert!(r.note.unwrap().contains("copy failed"));
    }

    #[test]
    fn a_local_engine_whose_bytes_do_not_match_its_name_is_never_the_fallback() {
        let d = tmp("nofallback");
        let eng = d.join("engine");
        let old = d.join("old.exe");
        std::fs::write(&old, b"the previous engine").unwrap();
        let local = install(&old, &eng, "v1").unwrap();
        std::fs::write(&local, b"tampered").unwrap();
        let r = hand_over(&d.join("absent.exe"), &eng, "v2", &d, &[], |_, _| Ok(()));
        assert!(r.is_err());
    }

    #[test]
    fn the_launcher_a_relocation_came_from_is_remembered_with_its_kit() {
        let d = tmp("launcher");
        let eng = d.join("engine");
        let kit = d.join("share");
        std::fs::create_dir_all(&kit).unwrap();
        let launcher = kit.join("Talos.exe");
        std::fs::write(&launcher, b"v1").unwrap();
        let local = hand_over(&launcher, &eng, "v1", &kit, &[], |_, _| Ok(()))
            .unwrap()
            .local;
        assert_eq!(remembered_launcher(&eng), Some(launcher));
        assert_eq!(remembered_kit(&eng), Some(kit));
        assert!(local.starts_with(&eng));
    }

    #[test]
    fn a_kit_remembered_before_the_launcher_was_still_reads_back() {
        let d = tmp("oldformat");
        let eng = d.join("engine");
        let kit = d.join("share");
        std::fs::create_dir_all(&kit).unwrap();
        std::fs::create_dir_all(&eng).unwrap();
        std::fs::write(eng.join("last-kit.txt"), kit.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(remembered_kit(&eng), Some(kit));
        assert_eq!(remembered_launcher(&eng), None);
    }

    /// An engine relocated from `kit/Talos.exe` holding `old`; the share's exe now holds `now`.
    fn pinned(name: &str, old: &[u8], now: &[u8]) -> (PathBuf, PathBuf, PathBuf) {
        let d = tmp(name);
        let eng = d.join("engine");
        let kit = d.join("share");
        std::fs::create_dir_all(&kit).unwrap();
        let launcher = kit.join("Talos.exe");
        std::fs::write(&launcher, old).unwrap();
        let local = hand_over(&launcher, &eng, "v1", &kit, &[], |_, _| Ok(()))
            .unwrap()
            .local;
        std::fs::write(&launcher, now).unwrap();
        (local, eng, launcher)
    }

    #[test]
    fn an_engine_started_by_hand_hands_back_to_a_kit_exe_that_changed() {
        // A pin on the old engine would run an old version against the new content.
        let (local, eng, launcher) = pinned("handback", b"v1", b"v2");
        let mut seen = None;
        let r = hand_back(&local, &eng, &s(&["--x"]), |p, a| {
            seen = Some((p.to_path_buf(), a.to_vec()));
            Ok(())
        });
        assert_eq!(r, Ok(Some(launcher.clone())));
        let want: Vec<std::ffi::OsString> = vec!["--x".into()];
        assert_eq!(
            seen,
            Some((launcher, want)),
            "no --kit: the launcher relocates itself"
        );
    }

    #[test]
    fn an_engine_started_by_hand_runs_when_the_kit_exe_is_the_same() {
        let (local, eng, _) = pinned("same", b"v1", b"v1");
        let mut spawned = false;
        let r = hand_back(&local, &eng, &[], |_, _| {
            spawned = true;
            Ok(())
        });
        assert_eq!(r, Ok(None));
        assert!(!spawned);
    }

    #[test]
    fn a_hand_back_that_fails_says_why_and_the_engine_runs() {
        let (local, eng, _) = pinned("handbackfail", b"v1", b"v2");
        let r = hand_back(&local, &eng, &[], |_, _| {
            Err(std::io::Error::other("blocked by policy"))
        });
        assert!(r.unwrap_err().contains("blocked by policy"));
    }

    #[test]
    fn an_engine_with_no_remembered_launcher_runs() {
        let d = tmp("nolauncher");
        let exe = d.join("Talos-v1-abc.exe");
        std::fs::write(&exe, b"v1").unwrap();
        let r = hand_back(&exe, &d, &[], |_, _| Ok(()));
        assert_eq!(r, Ok(None));
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
        remember_kit(&eng, &kit, &kit.join("Talos.exe"));
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
        .unwrap()
        .local;
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
