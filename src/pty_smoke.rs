#[path = "pty.rs"]
mod pty;

use std::io::Write;

fn main() {
    // Native shell per platform (POSIX login shell -lc, or PowerShell on Windows)
    // like runInPty/ptyShell — Talos is multi-platform, so is the smoke test.
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "windows") {
        (
            "powershell",
            vec![
                "-NoProfile",
                "-Command",
                "winget list --disable-interactivity",
            ],
        )
    } else if cfg!(target_os = "macos") {
        ("/bin/zsh", vec!["-lc", "brew list --versions"])
    } else {
        ("/bin/bash", vec!["-lc", "ls -la /usr/bin | head -40"])
    };
    let code = pty::run(program, &args, None, |bytes| {
        print!("{}", String::from_utf8_lossy(bytes));
        std::io::stdout().flush().ok();
    })
    .expect("pty run failed");
    eprintln!("\n[pty-smoke] exit code = {code}");
}
