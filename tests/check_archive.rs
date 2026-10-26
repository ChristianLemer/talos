//! `Talos --check` with no folders named, on a kit that carries `.talos/content.zip`: the
//! command get-talos runs after composing, so it must PROVE the archive — 0 on a good one,
//! 1 with a stable `error: content archive:` line on an unusable or vanished one (never a
//! pass on the previous extraction). `--kit` stands for the folder beside the exe, and the
//! local data folder is pointed at a scratch directory on every OS.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("talos-check-archive-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The repo's own socle, zipped the way the release does (catalog/, bundles/,
/// distribution.yaml at the root).
fn socle_zip(out: &Path) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let file = std::fs::File::create(out).unwrap();
    let mut w = zip::ZipWriter::new(file);
    let opt = zip::write::SimpleFileOptions::default();
    let mut add = |rel: &Path| {
        let name = rel.to_string_lossy().replace('\\', "/");
        w.start_file(name, opt).unwrap();
        std::io::Write::write_all(&mut w, &std::fs::read(root.join(rel)).unwrap()).unwrap();
    };
    fn walk(root: &Path, rel: &Path, add: &mut dyn FnMut(&Path)) {
        for e in std::fs::read_dir(root.join(rel)).unwrap() {
            let e = e.unwrap();
            let r = rel.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                walk(root, &r, add);
            } else {
                add(&r);
            }
        }
    }
    walk(root, Path::new("catalog"), &mut add);
    walk(root, Path::new("bundles"), &mut add);
    add(Path::new("distribution.yaml"));
    w.finish().unwrap();
}

fn check(kit: &Path, data: &Path) -> (i32, String) {
    let empty = data.join("cwd");
    std::fs::create_dir_all(&empty).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_Talos"))
        .args(["--kit", kit.to_str().unwrap(), "--check"])
        .current_dir(&empty)
        .env("XDG_DATA_HOME", data)
        .env("LOCALAPPDATA", data)
        .env("HOME", data)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn check_proves_the_archive_and_fails_on_a_broken_or_vanished_one() {
    let root = scratch("cli");
    let (kit, data) = (root.join("kit"), root.join("data"));
    std::fs::create_dir_all(kit.join(".talos")).unwrap();
    let archive = kit.join(".talos").join("content.zip");
    socle_zip(&archive);

    let (code, out) = check(&kit, &data);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(" 0 errors"), "{out}");

    std::fs::write(&archive, b"not a zip").unwrap();
    let (code, out) = check(&kit, &data);
    assert_eq!(code, 1, "{out}");
    assert!(out.starts_with("error: content archive: "), "{out}");
    assert!(out.contains("unusable"), "{out}");

    std::fs::remove_file(&archive).unwrap();
    let (code, out) = check(&kit, &data);
    assert_eq!(code, 1, "{out}");
    assert!(out.starts_with("error: content archive: "), "{out}");
    assert!(out.contains("missing"), "{out}");

    let _ = std::fs::remove_dir_all(&root);
}
