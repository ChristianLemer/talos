use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Receiver;

/// A working directory that cannot make `claude` refuse its own git.
///
/// ⭐ FIELD-MEASURED on Windows (2026-08-09), by extracting `claude` 2.1.226's
/// executable resolver and replaying it: it REFUSES any `git.exe` that lives under
/// the current directory, and answers *"Command 'git' not found or is in an unsafe
/// location (current directory)"*. That is an anti-hijack guard, and the message is
/// accurate — it does not mean "no git", it means "every git found is under the cwd".
///
/// The trap needs two ingredients, and a corporate estate supplies both:
///   - git installed under the user profile (`AppData\Local\Programs\Git`) — what a
///     winget install without elevation, or the installer's "just me" mode, does when
///     the user is not an administrator;
///   - Talos launched from a folder that is a common ancestor of EVERY candidate —
///     `C:\Users\clemer`, or any ancestor of it. Launching an exe from the profile or
///     from Downloads is the default gesture of someone told "copy these three things".
///
/// The pty inherited Talos's own cwd (`CommandBuilder` was never given one), so the
/// operator's PowerShell resolved git while Talos's child did not — same machine, same
/// PATH, same git, two verdicts. 6 of 12 tested directories triggered it.
///
/// `System32` is the answer rather than `C:\`: the guard compares `cwd + separator`,
/// and for the root that becomes `C:\\`, which nothing satisfies — so the root escapes
/// BY ACCIDENT of string concatenation. Relying on that would be fragile. `%TEMP%` is
/// worse than useless here: it lives under `AppData\Local`, squarely inside the trap.
///
/// POSIX keeps its inherited cwd: the guard is Windows-only in `claude`, so changing it
/// there would alter relative-path semantics for no benefit.
pub fn safe_working_dir() -> Option<std::path::PathBuf> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    let root = std::env::var_os("SYSTEMROOT")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    let sys32 = root.join("System32");
    // Never hand the pty a directory that does not exist — that fails the spawn
    // outright, which would be a worse defect than the one being fixed.
    if sys32.is_dir() {
        Some(sys32)
    } else if root.is_dir() {
        Some(root)
    } else {
        None
    }
}

/// Does `cwd` make `claude` reject `candidate`? Claude's rule, as measured: the
/// candidate is refused when its parent IS the cwd, or when it sits anywhere below it.
///
/// Pure, so the rule that drove the fix is pinned without a Windows machine. Compares
/// case-insensitively because Windows paths are.
/// ⚠️ The gate names WHAT USES this, not a platform. It read `not(target_os = "windows")`
/// and that was exactly backwards: the only callers are this module's tests, gated
/// `#[cfg(all(test, unix))]`, so the function is dead precisely ON Windows — where the old
/// gate withheld the allow and `clippy -D warnings` therefore failed, on the one platform
/// this code is about.
///
/// ⚠️ And the reason it shipped is NOT that the four verifications were skipped. They were
/// run, on a Mac, where this is green by construction; CI was green too and could not have
/// been otherwise, because CI ran on ubuntu alone. A `cfg`-gated defect is invisible to a
/// single-platform lint run — no amount of re-running the checks on one OS would have found
/// it. That is why `ci.yml` now lints on windows-latest as well.
#[cfg_attr(not(all(test, unix)), allow(dead_code))]
pub fn claude_would_refuse(candidate: &str, cwd: &str) -> bool {
    let c = candidate.to_lowercase().replace('/', "\\");
    let d = cwd.to_lowercase().replace('/', "\\");
    let d_trimmed = d.trim_end_matches('\\');
    // `dirname(candidate) == cwd`, then the `cwd + sep` prefix. A cwd of `c:\` yields
    // the prefix `c:\\`, which nothing matches — the accident that spares the root.
    match c.rsplit_once('\\') {
        Some((parent, _)) if parent == d_trimmed => true,
        _ => c.starts_with(&format!("{d}\\")),
    }
}

/// Runs `program args...` in a pty and calls `on_bytes` for each chunk read.
/// Returns the process exit code.
///
/// `input`: OPTIONAL input channel to the pty (stdin). When present, a thread
/// drains the receiver and writes the bytes to the master → allows responding to an
/// interactive prompt (e.g. sudo "Password:" during a `brew uninstall` of a GUI cask).
/// None = historical behavior (display-only, no input).
///
/// `on_spawn`: called ONCE, right after the child exists, with a killer for it. This
/// is how a caller cancels a step: `run` blocks until the process ends, so it cannot
/// RETURN a handle — the handle has to be pushed out while the call is still running.
/// The killer is `Send + Sync` because the caller kills from another task/thread.
///
/// Killing the child needs nothing else from us: the child-wait thread below already
/// drops the master on exit, which closes the pseudo-console and forces EOF on the
/// reader — so a kill takes the SAME exit path as a normal finish.
pub fn run<F: FnMut(&[u8])>(
    program: &str,
    args: &[&str],
    input: Option<Receiver<Vec<u8>>>,
    on_spawn: impl FnOnce(Box<dyn ChildKiller + Send + Sync>),
    mut on_bytes: F,
) -> std::io::Result<i32> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    // Windows: never let the child inherit Talos's own cwd — see `safe_working_dir`.
    // The child would otherwise run wherever the exe was launched from, and `claude`
    // refuses a git that lives under the cwd.
    if let Some(dir) = safe_working_dir() {
        cmd.cwd(dir);
    }
    // ⭐ Force plugin marketplace clones over HTTPS.
    //
    // MEASURED in claude 2.1.226: `owner/repo` becomes `git@github.com:owner/repo.git`
    // unless CLAUDE_CODE_PLUGIN_PREFER_HTTPS (or CLAUDE_CODE_REMOTE) is set — SSH is the
    // DEFAULT. So cloning a PUBLIC marketplace solicited the ssh agent, and 1Password
    // opened its own window asking for a key the repo never needed. claude already
    // hardens the clone against interactive prompts (GIT_TERMINAL_PROMPT=0, GIT_ASKPASS,
    // BatchMode=yes), but BatchMode stops ssh ASKING — it does not stop the agent being
    // QUERIED, and the agent's window lives outside git's channel entirely.
    //
    // Set in the pty ENVIRONMENT rather than prefixed onto the command line: only
    // `platform::shell_probe` / `pty_shell` may build a shell line (memory
    // single-shell-wrapping), and a third place doing it is how the fixes get bypassed.
    // The cost is that it does not appear in the row's terminal — accepted, because
    // `admin doctor` can report it and a shell-quoting bug here would break every step.
    cmd.env("CLAUDE_CODE_PLUGIN_PREFER_HTTPS", "1");
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    drop(pair.slave);

    // Hand the caller its killer BEFORE the read loop starts. Doing it here (and not
    // after the reader is set up) means a command that fails instantly still gives the
    // caller a handle — a row that could not be cancelled because the register came
    // too late would be a hang with extra steps.
    on_spawn(child.clone_killer());

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // Writing to the pty (the subprocess's stdin): a thread drains the input
    // channel. The sudo password arrives here when the caller has obtained it from the front.
    if let Some(rx) = input {
        if let Ok(mut writer) = pair.master.take_writer() {
            std::thread::spawn(move || {
                while let Ok(bytes) = rx.recv() {
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
            });
        }
    }

    // ConPTY (Windows): the reader does NOT always receive EOF at the end of the command
    // if a grandchild inherited the console handle — the `claude` case, a Node process.
    // The read loop would then stay blocked → the caller would never see the exit
    // code → the UI frozen on "installing". Fix: we wait for the CHILD to finish on a thread;
    // on its exit we DROP the master, which closes the pseudo-console and unblocks the reader.
    // On macOS/Linux EOF already arrives on its own → this drop is harmless (same result).
    let (code_tx, code_rx) = std::sync::mpsc::channel::<i32>();
    let master = pair.master;
    std::thread::spawn(move || {
        let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
        drop(master); // closes the pseudo-console → forces EOF on the reader side
        let _ = code_tx.send(code);
    });

    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => on_bytes(&buf[..n]),
            Err(_) => break,
        }
    }
    // The child has finished (otherwise the reader would not have EOF): its code awaits us.
    Ok(code_rx.recv().unwrap_or(-1))
}

// `#[cfg(unix)]` because both tests spawn `/bin/sh`, which does not exist on Windows.
// Not laziness: this project is developed against a real Windows VM, so a test that can
// only ever fail there would train the reader to ignore red — and the ConPTY behaviour
// these tests probe is exactly what needs a REAL Windows check (see the smoke-test doc),
// not a synthetic one. CI runs Linux, where these do run.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    // The real candidates and directories from the field measurement: git installed
    // under the user profile, with NO candidate outside it.
    const GIT_CMD: &str = r"C:\Users\clemer\AppData\Local\Programs\Git\cmd\git.exe";
    const GIT_MINGW: &str = r"C:\Users\clemer\AppData\Local\Programs\Git\mingw64\bin\git.exe";

    #[test]
    fn the_cwd_that_makes_claude_refuse_its_own_git() {
        // ⭐ Every one of these was MEASURED by replaying claude 2.1.226's resolver on
        // the machine — they are outputs, not deductions. A cwd that is a common
        // ancestor of ALL candidates leaves the resolver with nothing to return.
        for trap in [
            r"C:\Users",
            r"C:\Users\clemer",
            r"C:\Users\clemer\AppData",
            r"C:\Users\clemer\AppData\Local",
            r"C:\Users\clemer\AppData\Local\Programs",
            r"C:\Users\clemer\AppData\Local\Programs\Git",
        ] {
            assert!(
                claude_would_refuse(GIT_CMD, trap) && claude_would_refuse(GIT_MINGW, trap),
                "both candidates must be refused from {trap} — that is the reported bug"
            );
        }
        // And the directories that are safe, for the same reason reversed.
        for ok in [
            r"C:\Windows\System32",
            r"C:\Users\clemer\Downloads",
            r"C:\Talos",
        ] {
            assert!(
                !claude_would_refuse(GIT_CMD, ok),
                "{ok} must not trap the candidate"
            );
        }
    }

    #[test]
    fn descending_into_the_git_install_is_saved_by_a_sibling() {
        // ⚠️ Counter-intuitive, and measured: from `…\Git\mingw64\bin` the local
        // candidate IS refused, but the SIBLING `cmd\git.exe` is not — and the resolver
        // returns the first survivor. So the trap is not "being under the Git install",
        // it is being a common ancestor of EVERY candidate. Pinned because a fix aimed
        // at the wrong formulation would look right and miss.
        let cwd = r"C:\Users\clemer\AppData\Local\Programs\Git\mingw64\bin";
        assert!(
            claude_would_refuse(GIT_MINGW, cwd),
            "the local one is refused"
        );
        assert!(
            !claude_would_refuse(GIT_CMD, cwd),
            "the sibling survives, which is why this cwd resolves"
        );
    }

    #[test]
    fn the_drive_root_escapes_by_accident_so_it_is_not_the_answer() {
        // The guard tests the `cwd + separator` prefix; for `C:\` that is `C:\\`, which
        // nothing matches. Documented so nobody "simplifies" the fix to `C:\` — it works
        // for a reason that is a string-concatenation artefact, not a guarantee.
        assert!(!claude_would_refuse(GIT_CMD, r"C:\"));
    }

    #[test]
    fn probes_and_actions_agree_on_where_they_run() {
        // ⭐ A row that ACTS correctly and DETECTS wrong is worse than one that fails
        // outright: it reports absent, Apply installs, and the next scan reports absent
        // again — a loop with no error message. That happens the moment the probe path
        // and the pty path disagree about the cwd, so the two are pinned together here.
        //
        // Text-level, deliberately: the defect is the ABSENCE of a call in a function
        // whose whole job is to build a Command, which the source shows exactly. Same
        // technique as server.rs's `every_presence_observation_is_recorded`.
        let src = include_str!("platform.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let at = code
            .find("pub fn quiet_command")
            .expect("quiet_command must exist");
        let body = &code[at..(at + 700).min(code.len())];
        assert!(
            body.contains("safe_working_dir"),
            "the probe path must use the SAME safe cwd as the pty, or a package can read \
             absent while installing perfectly"
        );
    }

    #[test]
    fn the_safe_dir_is_outside_any_user_profile() {
        let dir = safe_working_dir();
        // POSIX imposes no cwd — claude's guard is Windows-only, so overriding it there
        // would change what a relative path means for no benefit. That `None` is the
        // CONTRACT, not an accident, so it is asserted rather than merely allowed.
        assert_eq!(
            dir.is_some(),
            cfg!(target_os = "windows"),
            "a cwd is imposed on Windows and only there"
        );
        if let Some(d) = dir {
            let s = d.to_string_lossy().to_lowercase();
            assert!(s.contains("system32") || s.contains("windows"), "got {s}");
            assert!(
                !claude_would_refuse(GIT_CMD, &d.to_string_lossy()),
                "the chosen cwd must not be an ancestor of a user-profile git: {s}"
            );
            // ⚠️ %TEMP% would be a natural guess and is WRONG: it lives under
            // AppData\Local, squarely inside the trap.
            assert!(
                !s.contains("appdata"),
                "must never be under the profile: {s}"
            );
        }
    }

    /// The load-bearing test of this whole feature: a killed child must let `run`
    /// RETURN rather than block forever. That only works because the child-wait
    /// thread drops the master on exit, which forces EOF on the reader — so this
    /// test guards that chain, not just the kill call.
    #[test]
    fn a_killed_child_lets_run_return() {
        // Annotated: the slot is filled from one thread and drained in another, so
        // inference has nothing to latch onto at the `take()` site.
        type Slot = std::sync::Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>;
        let killer_slot: std::sync::Arc<Slot> = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot = killer_slot.clone();
        // Kill from another thread once the killer has been handed over, exactly as
        // the server will (the killer crosses a thread boundary — hence Send + Sync).
        std::thread::spawn(move || {
            for _ in 0..100 {
                if let Some(mut k) = slot.lock().unwrap().take() {
                    let _: Box<dyn ChildKiller + Send + Sync> = {
                        let _ = k.kill();
                        k
                    };
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let started = std::time::Instant::now();
        // `sleep 30` would outlast any sane test timeout: if run() blocks, we know.
        let code = run(
            "/bin/sh",
            &["-c", "sleep 30"],
            None,
            |k| {
                *killer_slot.lock().unwrap() = Some(k);
            },
            |_bytes| {},
        )
        .expect("run must not error");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "run() blocked after the kill — the EOF chain is broken (elapsed {:?})",
            started.elapsed()
        );
        assert_ne!(code, 0, "a killed process must not report success");
    }

    /// on_spawn must fire even for a command that exits instantly — the caller
    /// registers its killer there, and a missed call would leave a row uncancellable.
    #[test]
    fn on_spawn_is_called_even_for_a_fast_command() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        let code = run(
            "/bin/sh",
            &["-c", "true"],
            None,
            |_k| {
                c.store(true, std::sync::atomic::Ordering::SeqCst);
            },
            |_b| {},
        )
        .expect("run must not error");
        assert!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            "on_spawn never fired"
        );
        assert_eq!(code, 0);
    }
}
