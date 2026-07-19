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

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| std::io::Error::other(e.to_string()))?;

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

    // ConPTY (Windows) : le reader ne reçoit PAS toujours l'EOF à la fin de la commande
    // si un petit-enfant a hérité du handle de console — cas `claude`, un process Node.
    // La boucle de lecture resterait alors bloquée → l'appelant ne verrait jamais l'exit
    // code → l'UI figée sur "installing". Fix : on attend la fin du CHILD sur un thread ;
    // à sa sortie on DROP le master, ce qui ferme la pseudo-console et débloque le reader.
    // Sur macOS/Linux l'EOF arrive déjà tout seul → ce drop est inoffensif (même résultat).
    let (code_tx, code_rx) = std::sync::mpsc::channel::<i32>();
    let master = pair.master;
    std::thread::spawn(move || {
        let code = child
            .wait()
            .map(|s| s.exit_code() as i32)
            .unwrap_or(-1);
        drop(master); // ferme la pseudo-console → force l'EOF côté reader
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
    // Le child a fini (sinon le reader n'aurait pas EOF) : son code nous attend.
    Ok(code_rx.recv().unwrap_or(-1))
}
