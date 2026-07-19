use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Receiver;

/// Lance `program args...` dans un pty et appelle `on_bytes` pour chaque chunk lu.
/// Retourne le code de sortie du process.
///
/// `input` : canal OPTIONNEL d'entrée vers le pty (stdin). Quand présent, un thread
/// draine le receiver et écrit les octets dans le master → permet de répondre à un
/// prompt interactif (ex. sudo "Password:" lors d'un `brew uninstall` de cask GUI).
/// None = comportement historique (display-only, aucune entrée).
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

    // Écriture vers le pty (stdin du sous-process) : un thread draine le canal
    // d'entrée. Le mot de passe sudo y arrive quand l'appelant l'a obtenu du front.
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

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
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
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(status.exit_code() as i32)
}
