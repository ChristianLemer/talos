use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Receiver;

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
