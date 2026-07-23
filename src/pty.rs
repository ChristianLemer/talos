use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Receiver;

/// Runs `program args...` in a pty and calls `on_bytes` for each chunk read.
/// Returns the process exit code.
///
/// `input`: OPTIONAL input channel to the pty (stdin). When present, a thread
/// drains the receiver and writes the bytes to the master → allows responding to an
/// interactive prompt (e.g. sudo "Password:" during a `brew uninstall` of a GUI cask).
/// None = historical behavior (display-only, no input).
pub fn run<F: FnMut(&[u8])>(
    program: &str,
    args: &[&str],
    input: Option<Receiver<Vec<u8>>>,
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
