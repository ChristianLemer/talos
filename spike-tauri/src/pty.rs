use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;

/// Lance `program args...` dans un pty et appelle `on_bytes` pour chaque chunk lu.
/// Retourne le code de sortie du process. Miroir de runInPty (src/server.ts) sans
/// le watcher Windows ni la détection 403 — le spike ne prouve QUE le streaming.
pub fn run<F: FnMut(&[u8])>(
    program: &str,
    args: &[&str],
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
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => on_bytes(&buf[..n]),
            Err(_) => break,
        }
    }
    let status = child
        .wait()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    Ok(status.exit_code() as i32)
}
