//! content.rs — the fixed half of a kit travels as ONE file.
//!
//! A kit is launched from a synced share, and a synced share is often PARTIAL: a replica that
//! lost some files, or holds conflict copies (`name (1).ext`), opened as a partial catalogue
//! that looked like the whole truth, and nobody could tell what was missing. A sync client
//! moves a single file whole, so everything the engine reads as content — `catalog/`,
//! `bundles/`, `distribution.yaml` — can travel as `.talos/content.zip`: a replica has the old
//! archive or the new one, never half of one. What people WRITE (`behaviour/`, `logs/`) stays
//! loose on the share, where it always was.
//!
//! The engine never reads the YAML inside the zip. Content commands hand REAL file paths to
//! external processes (`nu '{dir}/wt-default.nu' …`), which cannot open a file inside a zip,
//! so the archive is extracted — once per version — into a local folder named after its
//! sha256, and the rest of the engine keeps reading a folder, unaware of where it came from:
//!
//! 1. The share's archive is read into memory in ONE read, then released: never held open,
//!    so a publication replaces it while Talos runs everywhere. The hash and the extraction
//!    come from those same bytes — a publication landing between two reads cannot pair one
//!    version's hash with another's files.
//! 2. `content/<sha256[..12]>/` holding `.complete` (the full sha256) is used as is: a launch
//!    after the first extracts nothing (it only touches the marker's mtime, for pruning).
//! 3. Otherwise the archive is extracted into a per-launch `.partial` folder, `.complete` is
//!    written LAST, and the folder is renamed into place. A folder without `.complete` is
//!    never read; one that lost the rename to another launch is as good (same hash).
//!
//! An extraction is never rewritten: a new archive is a new folder beside it, so a running
//! Talos keeps reading the version it started on.
//!
//! ⚠️ `.talos/content.zip`, not `talos-content.zip`. The release asset keeps its name; a kit
//! composer writes the kit's copy into `.talos/`, the folder only composers write, so a stray
//! release download left on a share can never switch a kit to the socle by being found.
//!
//! An archive that cannot be used, or that VANISHED from a kit that had one (a sync client
//! deleting files is the very failure this exists for), is SAID, and the last extraction this
//! kit used is read instead — never the loose folders, which may be half-synced. Without any
//! record of an archive, a kit is read the way it always was (`platform::content_dirs_in`):
//! an old kit and a dev checkout are legitimate.

use crate::platform::Os;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where a kit carries its archive, relative to the kit root.
pub const ARCHIVE: &str = ".talos/content.zip";

/// The most a content archive may weigh on the share, checked from metadata BEFORE it is
/// read: an archive that holds a payload by mistake must not be read into memory and hashed
/// on every launch only to be refused afterwards.
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;

/// The most entries an archive may hold — the socle has a few dozen.
const MAX_ENTRIES: usize = 20_000;

/// The most an archive may unpack to. The socle is a few hundred KB; the large loose files a
/// kit may carry (an integrator's payload) are not content and stay out of the archive. Past
/// this, the archive is refused rather than allowed to fill the disk.
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;

/// An extraction that no resolve has touched for this long, and that no kit names, is pruned.
/// By AGE, never by count: nothing holds a handle inside an extraction between reads, and on
/// macOS and Linux a deletion never fails, so a count-based prune could delete the folder a
/// running session's `{dir}` still points to.
const UNUSED_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// A `.partial` folder older than this is litter from a launch killed mid-extraction. Younger,
/// it may belong to a launch extracting at this very moment.
const PARTIAL_LITTER: Duration = Duration::from_secs(3600);

const COMPLETE: &str = ".complete";

/// Where the content came from — and, when it is not what the kit says it should be, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Loose `catalog/` + `bundles/` folders: a kit without an archive, or a dev checkout.
    Loose,
    /// The kit's archive, verified. `extracted` = this resolve unpacked it (a new version).
    Archive { hash: String, extracted: bool },
    /// The archive is unusable or gone; the last extraction this kit used is read instead.
    Previous { hash: String, problem: String },
    /// The archive is unusable or gone, and there is nothing to fall back to.
    Missing { problem: String },
}

/// The resolved content: the two folders the engine reads, the kit root (`{kit}`), and where
/// they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub catalog: PathBuf,
    pub bundles: PathBuf,
    pub kit: PathBuf,
    pub source: Source,
}

impl Content {
    /// The sentence a person reads when the content is not the kit's current archive. `None`
    /// when all is as it should be.
    pub fn problem(&self) -> Option<String> {
        match &self.source {
            Source::Loose | Source::Archive { .. } => None,
            Source::Previous { hash, problem } => Some(format!(
                "This kit's content archive {problem}. Showing the last content this machine \
                 read from it ({}); the catalogue may be out of date until the archive syncs.",
                &hash[..hash.len().min(12)]
            )),
            Source::Missing { problem } => Some(format!(
                "This kit's content archive {problem}, and this machine has no earlier copy of \
                 it: there is no catalogue to show until the archive syncs."
            )),
        }
    }

    /// A few words for the machine profile: which content this launch read.
    pub fn summary(&self) -> String {
        match &self.source {
            Source::Loose => "loose folders".to_string(),
            Source::Archive { hash, .. } => format!("archive {}", &hash[..12]),
            Source::Previous { hash, problem } => {
                format!("an earlier copy, {}: the archive {problem}", &hash[..12])
            }
            Source::Missing { problem } => format!("none: the archive {problem}"),
        }
    }

    /// One line for the log's start block: which content this session reads.
    pub fn log_line(&self) -> String {
        match &self.source {
            Source::Loose => format!("[content] loose folders in {}", self.kit.display()),
            Source::Archive { hash, extracted } => format!(
                "[content] archive {} {} into {}",
                &hash[..12],
                if *extracted { "extracted" } else { "reused" },
                self.catalog.parent().unwrap_or(&self.catalog).display()
            ),
            Source::Previous { hash, problem } => {
                format!(
                    "[content] archive {problem}: reading the previous one, {}",
                    &hash[..12]
                )
            }
            Source::Missing { problem } => format!("[content] archive {problem}: no content"),
        }
    }
}

/// The local folder extractions live in: `<local data dir>/content`. `None` when the
/// environment names no absolute data folder — a relative one would land in the cwd, which
/// under an Explorer launch is the share itself.
pub fn cache_root(os: Os) -> Option<PathBuf> {
    let root = crate::platform::local_data_dir(os).join("content");
    root.is_absolute().then_some(root)
}

/// The content for this process: the kit beside the exe (or `--kit`), extracted into the
/// local cache. ONE rule for the server, `--check` and the window title.
pub fn resolve() -> Content {
    let os = crate::platform::current_os();
    resolve_in(&crate::platform::exe_sibling(), cache_root(os).as_deref())
}

/// What the server last resolved — so the window title reads the SAME content instead of
/// reading and hashing the share's archive a second time (a publication landing between the
/// two reads would put the title and the catalogue on different versions).
static CURRENT: std::sync::Mutex<Option<Content>> = std::sync::Mutex::new(None);

/// Record what this process reads now (the server, at boot and on every Refresh).
pub fn publish(content: &Content) {
    if let Ok(mut c) = CURRENT.lock() {
        *c = Some(content.clone());
    }
}

/// What the server resolved, or a resolve of our own when it has not yet (the title thread
/// gives up waiting for the server after 5 s; the two extractions then use distinct folders).
pub fn current() -> Content {
    if let Some(c) = CURRENT.lock().ok().and_then(|c| c.clone()) {
        return c;
    }
    resolve()
}

/// The kit roots a sibling folder can belong to, in the order `content_dirs_in` looks for
/// `catalog/`: the folder itself (a flat kit), then exactly one level up (an OS folder in a
/// composed kit). Never further.
fn kit_candidates(sibling: &Path) -> Vec<PathBuf> {
    let mut v = vec![sibling.to_path_buf()];
    if let Some(up) = sibling.parent() {
        if !up.as_os_str().is_empty() {
            v.push(up.to_path_buf());
        }
    }
    v
}

/// `resolve` with its inputs named, for the tests: the sibling folder and the cache root.
///
/// Level by level — beside the sibling, then one level up — and at each level: the kit's
/// archive; else, for a kit that HAD one, the fact that it vanished (said, and its last
/// extraction read); else a loose `catalog/` at that level stops the search, so a loose kit
/// beside the exe still wins over an archive kit one level up, as `content_dirs_in` always
/// preferred beside over above. Nothing found → the loose rule, unchanged.
pub fn resolve_in(sibling: &Path, cache: Option<&Path>) -> Content {
    for kit in kit_candidates(sibling) {
        let archive = kit.join(ARCHIVE);
        if archive.is_file() {
            let Some(cache) = cache else {
                return missing(
                    &kit,
                    "cannot be extracted: this machine names no local data folder",
                );
            };
            let content = match unpack(&archive, cache) {
                Ok((dir, hash, extracted)) => {
                    remember(cache, &kit, &hash);
                    in_extraction(&kit, &dir, Source::Archive { hash, extracted })
                }
                Err(problem) => previous(cache, &kit, &format!("is unusable ({problem})")),
            };
            prune(cache);
            return content;
        }
        if let Some(cache) = cache {
            if remembered(cache, &kit).is_some() {
                let content = previous(cache, &kit, &format!("is missing ({ARCHIVE})"));
                prune(cache);
                return content;
            }
        }
        if kit.join("catalog").is_dir() {
            break;
        }
    }
    let (catalog, bundles) = crate::platform::content_dirs_in(sibling);
    let kit = catalog
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| sibling.to_path_buf());
    Content {
        catalog,
        bundles,
        kit,
        source: Source::Loose,
    }
}

fn in_extraction(kit: &Path, dir: &Path, source: Source) -> Content {
    Content {
        catalog: dir.join("catalog"),
        bundles: dir.join("bundles"),
        kit: kit.to_path_buf(),
        source,
    }
}

/// No content at all: two paths that do not exist, so the catalogue loads empty. Inside
/// `.talos/` — never a relative name, which would resolve against the cwd and could adopt a
/// dev checkout's catalogue.
fn missing(kit: &Path, problem: &str) -> Content {
    let none = kit.join(".talos").join("no-content");
    Content {
        catalog: none.join("catalog"),
        bundles: none.join("bundles"),
        kit: kit.to_path_buf(),
        source: Source::Missing {
            problem: problem.to_string(),
        },
    }
}

/// The last extraction this kit used, or nothing — with the problem either way.
fn previous(cache: &Path, kit: &Path, problem: &str) -> Content {
    if let Some(hash) = remembered(cache, kit) {
        let dir = cache.join(&hash[..12]);
        if is_complete(&dir, &hash) {
            touch(&dir);
            return in_extraction(
                kit,
                &dir,
                Source::Previous {
                    hash,
                    problem: problem.to_string(),
                },
            );
        }
    }
    missing(kit, problem)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn is_complete(dir: &Path, hash: &str) -> bool {
    marker(dir, hash) == Marker::Matches
}

/// What an extraction's `.complete` says. `Unreadable` is NOT `Missing`: a transient read
/// failure must never get a valid extraction — perhaps a running session's `{dir}` — deleted.
#[derive(Debug, PartialEq, Eq)]
enum Marker {
    Matches,
    Missing,
    Differs,
    Unreadable,
}

fn marker(dir: &Path, hash: &str) -> Marker {
    match std::fs::read_to_string(dir.join(COMPLETE)) {
        Ok(h) if h.trim() == hash => Marker::Matches,
        Ok(_) => Marker::Differs,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Marker::Missing,
        Err(_) => Marker::Unreadable,
    }
}

/// Distinguishes two extractions in ONE process (the title thread and the server, two
/// windows clicking Refresh): a pid alone would hand them the same `.partial` folder.
static EXTRACTION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times a rename into place is tried, the pause doubling from 50 ms (capped at
/// 1 s). On Windows a directory cannot be renamed while anything holds a file inside it
/// without sharing delete — an antivirus scanning the files just written is the classic
/// case, and it lets go within a second or two.
const RENAME_TRIES: u32 = 10;

/// Mark an extraction as used now — what keeps it from being pruned. Best-effort.
fn touch(dir: &Path) {
    if let Ok(f) = std::fs::File::options()
        .write(true)
        .open(dir.join(COMPLETE))
    {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// The archive's local extraction: `(folder, sha256, extracted now)`, or why it cannot be.
fn unpack(archive: &Path, cache: &Path) -> Result<(PathBuf, String, bool), String> {
    let len = std::fs::metadata(archive)
        .map_err(|e| format!("cannot be read: {e}"))?
        .len();
    if len > MAX_ARCHIVE {
        return Err(format!(
            "weighs {len} bytes, more than the {MAX_ARCHIVE} a content archive may"
        ));
    }
    let bytes = std::fs::read(archive).map_err(|e| format!("cannot be read: {e}"))?;
    let hash = sha256_hex(&bytes);
    let target = cache.join(&hash[..12]);
    if is_complete(&target, &hash) {
        touch(&target);
        return Ok((target, hash, false));
    }
    std::fs::create_dir_all(cache).map_err(|e| format!("cannot be extracted: {e}"))?;
    let n = EXTRACTION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let partial = cache.join(format!(
        "{}.{}-{n}.partial",
        &hash[..12],
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&partial);
    let written = extract(&bytes, &partial).and_then(|_| {
        std::fs::write(partial.join(COMPLETE), &hash)
            .map_err(|e| format!("cannot be extracted: {e}"))
    });
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    let mut pause = std::time::Duration::from_millis(50);
    for attempt in 1..=RENAME_TRIES {
        // A folder under the target name whose marker is MISSING or names another archive is
        // never valid (an extraction only gets that name by a rename AFTER its marker is
        // written): it is in the way, not in use. An unreadable marker is left alone.
        if target.exists() && matches!(marker(&target, &hash), Marker::Missing | Marker::Differs) {
            let _ = std::fs::remove_dir_all(&target);
        }
        if std::fs::rename(&partial, &target).is_ok() {
            return Ok((target, hash, true));
        }
        // Another launch got there first: same name, same hash, as good as ours.
        if is_complete(&target, &hash) {
            let _ = std::fs::remove_dir_all(&partial);
            return Ok((target, hash, false));
        }
        if attempt < RENAME_TRIES {
            std::thread::sleep(pause);
            pause = (pause * 2).min(std::time::Duration::from_secs(1));
        }
    }
    let _ = std::fs::remove_dir_all(&partial);
    Err(format!(
        "cannot be extracted: {} could not be put in place",
        target.display()
    ))
}

/// An entry name as a relative path inside the extraction, or why the archive is refused.
/// Backslashes are separators (PowerShell 5.1's `Compress-Archive` writes them). An absolute
/// name, a drive or stream (`:`), or a `..` component would write outside the extraction —
/// the whole archive is refused, not just the entry. `None` = the root itself (nothing to do).
fn entry_path(raw: &str) -> Result<Option<PathBuf>, String> {
    let name = raw.replace('\\', "/");
    if name.starts_with('/') || name.contains(':') {
        return Err(format!("an entry names a path outside the archive: {raw}"));
    }
    let mut path = PathBuf::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(format!("an entry names a path outside the archive: {raw}")),
            p => path.push(p),
        }
    }
    Ok((!path.as_os_str().is_empty()).then_some(path))
}

/// Unpack `bytes` into `dest`. Every entry name is checked and the declared sizes summed
/// BEFORE anything is written; the bytes actually written are capped too (a declared size can
/// lie). A CRC mismatch surfaces as a read error. The archive must hold a `catalog/`.
fn extract(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("is not a readable zip: {e}"))?;
    if zip.len() > MAX_ENTRIES {
        return Err(format!(
            "holds {} entries, more than the {MAX_ENTRIES} a content archive may",
            zip.len()
        ));
    }
    let mut declared: u64 = 0;
    let mut has_catalog = false;
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| format!("is not a readable zip: {e}"))?;
        let name = entry
            .name()
            .map_err(|e| format!("is not a readable zip: {e}"))?
            .into_owned();
        // A FILE under `catalog/` — an empty `catalog/` entry, or a file named `catalog`, is
        // not a catalogue.
        if let Some(p) = entry_path(&name)? {
            has_catalog |= !entry.is_dir()
                && p.components().count() > 1
                && p.components()
                    .next()
                    .is_some_and(|c| c.as_os_str() == "catalog");
        }
        declared = declared.saturating_add(entry.size());
    }
    if declared > MAX_UNPACKED {
        return Err(format!(
            "unpacks to {declared} bytes, more than the {MAX_UNPACKED} a content archive may"
        ));
    }
    if !has_catalog {
        return Err("holds no catalog/".to_string());
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("cannot be extracted: {e}"))?;
    let mut written: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("is not a readable zip: {e}"))?;
        let name = entry
            .name()
            .map_err(|e| format!("is not a readable zip: {e}"))?
            .into_owned();
        let Some(rel) = entry_path(&name)? else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("cannot be extracted: {e}"))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot be extracted: {e}"))?;
        }
        let mut data = Vec::new();
        (&mut entry)
            .take(MAX_UNPACKED - written + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("is damaged ({name}: {e})"))?;
        written += data.len() as u64;
        if written > MAX_UNPACKED {
            return Err(format!(
                "unpacks to more than the {MAX_UNPACKED} bytes a content archive may"
            ));
        }
        std::fs::File::create(&out)
            .and_then(|mut f| f.write_all(&data))
            .map_err(|e| format!("cannot be extracted: {e}"))?;
        // The exec bit, where the notion exists: a content command that runs `'{dir}/x.sh'`
        // directly worked from a loose folder and must work from the extraction.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            if mode & 0o111 != 0 {
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    Ok(())
}

/// The record of which archive a kit last used — what lets a vanished or unusable archive be
/// told from a kit that never had one, and what the fallback reads. One small file per kit
/// root, so kits opened on the same machine never evict each other's. Keyed by the kit's
/// absolute path, case folded: Windows paths are case-insensitive, and folding everywhere can
/// only merge two records, never lose one.
fn record_path(cache: &Path, kit: &Path) -> PathBuf {
    let abs = std::path::absolute(kit).unwrap_or_else(|_| kit.to_path_buf());
    let key = sha256_hex(abs.to_string_lossy().to_lowercase().as_bytes());
    cache.join("kits").join(format!("{}.txt", &key[..16]))
}

fn remember(cache: &Path, kit: &Path, hash: &str) {
    let path = record_path(cache, kit);
    if remembered(cache, kit).as_deref() == Some(hash) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, hash);
}

fn remembered(cache: &Path, kit: &Path) -> Option<String> {
    let h = std::fs::read_to_string(record_path(cache, kit)).ok()?;
    let h = h.trim();
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_string())
}

/// Remove extractions no kit names and no resolve touched for `UNUSED_FOR`, and `.partial`
/// litter older than `PARTIAL_LITTER`. Best-effort; never touches anything else.
fn prune(cache: &Path) {
    prune_at(cache, SystemTime::now());
}

fn prune_at(cache: &Path, now: SystemTime) {
    let named: std::collections::HashSet<String> = std::fs::read_dir(cache.join("kits"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|h| h.trim().get(..12).map(str::to_string))
        .collect();
    let age = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .unwrap_or_default()
    };
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !path.is_dir() {
            continue;
        }
        if name.ends_with(".partial") {
            if age(&path) > PARTIAL_LITTER {
                let _ = std::fs::remove_dir_all(&path);
            }
            continue;
        }
        let is_extraction = name.len() == 12 && name.bytes().all(|b| b.is_ascii_hexdigit());
        if is_extraction && !named.contains(&name) && age(&path.join(COMPLETE)) > UNUSED_FOR {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("talos-content-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A zip holding `files` (name → text), deflated.
    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opt = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in files {
            w.start_file(*name, opt).unwrap();
            w.write_all(text.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    const SOCLE: &[(&str, &str)] = &[
        ("catalog/git.yaml", "name: Git\nbrew: git\n"),
        ("catalog/side.nu", "print hi\n"),
        ("bundles/base/bundle.yaml", "name: Base\n"),
        ("distribution.yaml", "name: Ops\n"),
    ];

    fn put_archive(kit: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(kit.join(".talos")).unwrap();
        std::fs::write(kit.join(ARCHIVE), bytes).unwrap();
    }

    /// The archive wins: content is read from a local extraction named after the archive's
    /// hash, and `{kit}` is the folder holding `.talos/`.
    #[test]
    fn an_archive_is_read_from_its_local_extraction() {
        let root = tmp("read");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(SOCLE));
        let c = resolve_in(&kit, Some(&cache));
        let Source::Archive { hash, extracted } = &c.source else {
            panic!("{c:?}")
        };
        assert!(*extracted);
        assert_eq!(c.catalog, cache.join(&hash[..12]).join("catalog"));
        assert_eq!(c.kit, kit);
        assert!(c.catalog.join("git.yaml").is_file());
        assert!(c.catalog.join("side.nu").is_file());
        assert!(c
            .catalog
            .parent()
            .unwrap()
            .join("distribution.yaml")
            .is_file());
        assert!(c.bundles.join("base/bundle.yaml").is_file());
        assert_eq!(c.problem(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Once per version: a second launch on the same archive writes nothing.
    #[test]
    fn a_second_launch_reuses_the_extraction() {
        let root = tmp("reuse");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(SOCLE));
        let first = resolve_in(&kit, Some(&cache));
        // A hand-made marker inside the extraction survives: nothing was re-extracted.
        std::fs::write(first.catalog.join("marker"), "x").unwrap();
        let second = resolve_in(&kit, Some(&cache));
        assert!(
            matches!(
                second.source,
                Source::Archive {
                    extracted: false,
                    ..
                }
            ),
            "{second:?}"
        );
        assert_eq!(second.catalog, first.catalog);
        assert!(second.catalog.join("marker").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A new archive is a NEW folder beside the old one, which stays untouched — a running
    /// session keeps reading the version it started on.
    #[test]
    fn a_new_archive_never_rewrites_the_old_extraction() {
        let root = tmp("new");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(SOCLE));
        let old = resolve_in(&kit, Some(&cache));
        put_archive(&kit, &zip_of(&[("catalog/node.yaml", "name: Node\n")]));
        let new = resolve_in(&kit, Some(&cache));
        assert_ne!(old.catalog, new.catalog);
        assert!(old.catalog.join("git.yaml").is_file());
        assert!(new.catalog.join("node.yaml").is_file());
        assert!(!new.catalog.join("git.yaml").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The archive wins over loose folders beside it — the transition copy, or a half-synced
    /// leftover, is never read once an archive is there.
    #[test]
    fn an_archive_wins_over_loose_folders() {
        let root = tmp("wins");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        std::fs::create_dir_all(kit.join("catalog")).unwrap();
        std::fs::write(kit.join("catalog/stale.yaml"), "name: Stale\n").unwrap();
        put_archive(&kit, &zip_of(SOCLE));
        let c = resolve_in(&kit, Some(&cache));
        assert!(matches!(c.source, Source::Archive { .. }));
        assert!(!c.catalog.join("stale.yaml").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The composed kit: the launcher in its OS folder, the archive one level up.
    #[test]
    fn the_archive_is_found_one_level_up_and_never_two() {
        let root = tmp("levels");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(SOCLE));
        let c = resolve_in(&kit.join("Windows"), Some(&cache));
        assert!(matches!(c.source, Source::Archive { .. }), "{c:?}");
        assert_eq!(c.kit, kit);
        let deep = resolve_in(&kit.join("a/b"), Some(&cache));
        assert_eq!(deep.source, Source::Loose, "{deep:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stray release download at the kit root is NOT the kit's archive.
    #[test]
    fn a_release_asset_at_the_root_is_not_adopted() {
        let root = tmp("stray");
        let kit = root.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        std::fs::write(kit.join("talos-content.zip"), zip_of(SOCLE)).unwrap();
        let c = resolve_in(&kit, Some(&root.join("cache")));
        assert_eq!(c.source, Source::Loose);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No archive and no record: exactly today's loose rule, and `{kit}` is the catalog's
    /// parent — so `{kit}` equals `{dir}/..` in every loose kit.
    #[test]
    fn without_an_archive_a_kit_reads_as_it_always_did() {
        let root = tmp("loose");
        let kit = root.join("kit");
        std::fs::create_dir_all(kit.join("catalog")).unwrap();
        std::fs::create_dir_all(kit.join("bundles")).unwrap();
        let c = resolve_in(&kit.join("Windows"), Some(&root.join("cache")));
        assert_eq!(c.source, Source::Loose);
        assert_eq!(c.catalog, kit.join("catalog"));
        assert_eq!(c.kit, kit);
        assert_eq!(c.problem(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A damaged archive is said, and the last extraction this kit used is read — never the
    /// loose folders beside it.
    #[test]
    fn a_damaged_archive_falls_back_to_the_previous_extraction() {
        let root = tmp("damaged");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        std::fs::create_dir_all(kit.join("catalog")).unwrap();
        put_archive(&kit, &zip_of(SOCLE));
        let good = resolve_in(&kit, Some(&cache));
        let bytes = zip_of(&[("catalog/node.yaml", "name: Node\n")]);
        put_archive(&kit, &bytes[..bytes.len() / 2]); // truncated: the central directory is gone
        let c = resolve_in(&kit, Some(&cache));
        let Source::Previous { problem, .. } = &c.source else {
            panic!("{c:?}")
        };
        assert!(problem.starts_with("is unusable"), "{problem}");
        assert_eq!(c.catalog, good.catalog);
        assert!(
            c.problem().unwrap().contains("last content"),
            "{:?}",
            c.problem()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A damaged archive on a machine that never read this kit: nothing, said — and the
    /// folders it points at do not exist, so the catalogue loads empty.
    #[test]
    fn a_damaged_archive_with_nothing_before_is_missing() {
        let root = tmp("nothing");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        std::fs::create_dir_all(kit.join("catalog")).unwrap();
        put_archive(&kit, b"not a zip at all");
        let c = resolve_in(&kit, Some(&cache));
        assert!(matches!(c.source, Source::Missing { .. }), "{c:?}");
        assert!(!c.catalog.exists());
        assert_ne!(c.catalog, kit.join("catalog"));
        assert!(c.problem().unwrap().contains("no catalogue"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The failure this module exists for: a sync client deletes the archive. A kit that had
    /// one says so and reads its last extraction, instead of falling back to loose folders.
    #[test]
    fn a_vanished_archive_is_said() {
        let root = tmp("vanished");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        std::fs::create_dir_all(kit.join("catalog")).unwrap();
        std::fs::write(kit.join("catalog/half.yaml"), "name: Half\n").unwrap();
        put_archive(&kit, &zip_of(SOCLE));
        let good = resolve_in(&kit.join("Windows"), Some(&cache));
        std::fs::remove_file(kit.join(ARCHIVE)).unwrap();
        let c = resolve_in(&kit.join("Windows"), Some(&cache));
        let Source::Previous { problem, .. } = &c.source else {
            panic!("{c:?}")
        };
        assert!(problem.starts_with("is missing"), "{problem}");
        assert_eq!(c.catalog, good.catalog);
        assert_eq!(c.kit, kit);
        assert!(
            c.summary().starts_with("an earlier copy, "),
            "{}",
            c.summary()
        );
        assert!(good.summary().starts_with("archive "), "{}", good.summary());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No local data folder: never extract into a relative path (the cwd may be the share).
    #[test]
    fn no_cache_root_means_no_extraction() {
        let root = tmp("nocache");
        let kit = root.join("kit");
        put_archive(&kit, &zip_of(SOCLE));
        let c = resolve_in(&kit, None);
        assert!(matches!(c.source, Source::Missing { .. }), "{c:?}");
        assert!(!c.catalog.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Zip-slip and friends: the WHOLE archive is refused, and nothing lands outside.
    #[test]
    fn an_entry_outside_the_archive_refuses_it() {
        for evil in [
            "../evil.yaml",
            "/etc/evil.yaml",
            "C:/evil.yaml",
            "catalog\\..\\..\\evil",
        ] {
            let root = tmp("slip");
            let (kit, cache) = (root.join("kit"), root.join("cache"));
            put_archive(
                &kit,
                &zip_of(&[("catalog/git.yaml", "name: Git\n"), (evil, "x")]),
            );
            let c = resolve_in(&kit, Some(&cache));
            assert!(matches!(c.source, Source::Missing { .. }), "{evil}: {c:?}");
            assert!(!root.join("evil.yaml").exists(), "{evil}");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// PowerShell 5.1's `Compress-Archive` writes backslash names: they are separators.
    #[test]
    fn backslash_names_are_separators() {
        let root = tmp("backslash");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(&[("catalog\\git.yaml", "name: Git\n")]));
        let c = resolve_in(&kit, Some(&cache));
        assert!(c.catalog.join("git.yaml").is_file(), "{c:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_archive_without_a_catalog_is_refused() {
        let root = tmp("nocat");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(&[("bundles/x.yaml", "name: X\n")]));
        let c = resolve_in(&kit, Some(&cache));
        let Source::Missing { problem } = &c.source else {
            panic!("{c:?}")
        };
        assert!(problem.contains("no catalog/"), "{problem}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder without `.complete` (a launch killed mid-extraction) is never read.
    #[test]
    fn an_incomplete_extraction_is_redone() {
        let root = tmp("incomplete");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        let bytes = zip_of(SOCLE);
        put_archive(&kit, &bytes);
        let short = &sha256_hex(&bytes)[..12];
        std::fs::create_dir_all(cache.join(short).join("catalog")).unwrap();
        let c = resolve_in(&kit, Some(&cache));
        assert!(
            matches!(
                c.source,
                Source::Archive {
                    extracted: true,
                    ..
                }
            ),
            "{c:?}"
        );
        assert!(c.catalog.join("git.yaml").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two kits on one machine keep their own record: each falls back to its OWN content.
    #[test]
    fn two_kits_keep_their_own_fallback() {
        let root = tmp("twokits");
        let cache = root.join("cache");
        let (a, b) = (root.join("a"), root.join("b"));
        put_archive(&a, &zip_of(SOCLE));
        put_archive(&b, &zip_of(&[("catalog/node.yaml", "name: Node\n")]));
        let ca = resolve_in(&a, Some(&cache));
        let cb = resolve_in(&b, Some(&cache));
        std::fs::remove_file(a.join(ARCHIVE)).unwrap();
        std::fs::remove_file(b.join(ARCHIVE)).unwrap();
        assert_eq!(resolve_in(&a, Some(&cache)).catalog, ca.catalog);
        assert_eq!(resolve_in(&b, Some(&cache)).catalog, cb.catalog);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Prune by age, and never what a kit names.
    #[test]
    fn prune_keeps_named_and_recent_extractions() {
        let root = tmp("prune");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(SOCLE));
        let named = resolve_in(&kit, Some(&cache));
        // An unnamed extraction, and a stale `.partial`.
        let orphan = cache.join("0123456789ab");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join(COMPLETE), "x").unwrap();
        let litter = cache.join("0123456789ab.42.partial");
        std::fs::create_dir_all(&litter).unwrap();
        // Today: everything is recent, nothing goes.
        prune_at(&cache, SystemTime::now());
        assert!(orphan.is_dir() && litter.is_dir());
        // Two months later: the orphan and the litter go, the named extraction stays.
        let later = SystemTime::now() + Duration::from_secs(60 * 24 * 3600);
        prune_at(&cache, later);
        assert!(!orphan.exists());
        assert!(!litter.exists());
        assert!(named.catalog.is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A loose kit beside the exe still wins over an archive kit one level up — the old
    /// "beside over above" precedence, kept.
    #[test]
    fn a_loose_catalog_beside_wins_over_an_archive_above() {
        let root = tmp("beside");
        let (outer, cache) = (root.join("outer"), root.join("cache"));
        put_archive(&outer, &zip_of(SOCLE));
        let inner = outer.join("Old");
        std::fs::create_dir_all(inner.join("catalog")).unwrap();
        let c = resolve_in(&inner, Some(&cache));
        assert_eq!(c.source, Source::Loose);
        assert_eq!(c.catalog, inner.join("catalog"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An archive too heavy for content is refused from its SIZE, before it is read.
    #[test]
    fn an_oversized_archive_is_refused_unread() {
        let root = tmp("heavy");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        std::fs::create_dir_all(kit.join(".talos")).unwrap();
        // Sparse: the length is there, the bytes are not.
        std::fs::File::create(kit.join(ARCHIVE))
            .unwrap()
            .set_len(MAX_ARCHIVE + 1)
            .unwrap();
        let c = resolve_in(&kit, Some(&cache));
        let Source::Missing { problem } = &c.source else {
            panic!("{c:?}")
        };
        assert!(problem.contains("weighs"), "{problem}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An empty `catalog/` entry is not a catalogue.
    #[test]
    fn an_empty_catalog_entry_is_not_a_catalogue() {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.add_directory("catalog/", SimpleFileOptions::default())
            .unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let root = tmp("emptycat");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &bytes);
        let c = resolve_in(&kit, Some(&cache));
        assert!(matches!(c.source, Source::Missing { .. }), "{c:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The exec bit survives the extraction where the notion exists.
    #[cfg(unix)]
    #[test]
    fn the_exec_bit_survives() {
        use std::os::unix::fs::PermissionsExt;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("catalog/git.yaml", SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"name: Git\n").unwrap();
        w.start_file(
            "catalog/run.sh",
            SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
        w.write_all(b"#!/bin/sh\n").unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let root = tmp("exec");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &bytes);
        let c = resolve_in(&kit, Some(&cache));
        let mode = std::fs::metadata(c.catalog.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "{mode:o}");
        let yaml = std::fs::metadata(c.catalog.join("git.yaml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(yaml & 0o111, 0, "{yaml:o}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An old extraction a kit's record still names is kept, however old — it is that
    /// kit's fallback.
    #[test]
    fn prune_keeps_an_old_extraction_a_kit_names() {
        let root = tmp("prune-named");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(SOCLE));
        let c = resolve_in(&kit, Some(&cache));
        let later = SystemTime::now() + Duration::from_secs(365 * 24 * 3600);
        prune_at(&cache, later);
        assert!(c.catalog.is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two extractions in one process never share a `.partial` folder.
    #[test]
    fn two_extractions_in_one_process_do_not_collide() {
        let root = tmp("samepid");
        let (kit, cache) = (root.join("kit"), root.join("cache"));
        put_archive(&kit, &zip_of(SOCLE));
        let (a, b) = std::thread::scope(|s| {
            let a = s.spawn(|| resolve_in(&kit, Some(&cache)));
            let b = s.spawn(|| resolve_in(&kit, Some(&cache)));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert!(matches!(a.source, Source::Archive { .. }), "{a:?}");
        assert!(matches!(b.source, Source::Archive { .. }), "{b:?}");
        assert_eq!(a.catalog, b.catalog);
        assert!(a.catalog.join("git.yaml").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn entry_paths() {
        assert_eq!(
            entry_path("catalog/").unwrap(),
            Some(PathBuf::from("catalog"))
        );
        assert_eq!(
            entry_path("./catalog//x.yaml").unwrap(),
            Some(PathBuf::from("catalog/x.yaml"))
        );
        assert_eq!(entry_path("").unwrap(), None);
        assert!(entry_path("a/../../b").is_err());
        assert!(entry_path("file.txt:stream").is_err());
    }
}
