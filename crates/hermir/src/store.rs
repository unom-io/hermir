//! The prefix on disk: `<root>/<id>/app/` is an emulator's home, `<root>/<id>/manifest.json`
//! lists what the release shipped there (path → sha256), `installed.json` says what is
//! installed and from where, `.tmp/` holds work in flight, `.lock` serialises writers.
//!
//! A portable emulator keeps its config and saves beside its exe, inside `app/`. The manifest
//! is what lets an update replace the release's files and nothing else, and lets a remove
//! take the release away and leave the data.
//!
//! Links are never followed. A link the release ships is a leaf like a file (its manifest
//! hash is the sha256 of `symlink:<target>`); a link the user makes in `app/`, to a ROM
//! folder say, is the user's, and nothing is written, moved or deleted through it.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::Installed;

#[derive(Clone, Debug)]
/// The managed prefix on disk: one folder per emulator (`<id>/app`, its manifest),
/// `installed.json`, the snapshots of `apply`, and the lock every write holds.
pub struct Store {
    root: PathBuf,
}

/// Held for the duration of a write to the prefix; the OS lock goes with the file.
pub struct Lock {
    _file: fs::File,
}

/// What placing a release tree did beyond copying it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// Shipped paths where the user's own file was: one they edited, or made where the release
    /// now ships one. Kept as they were, the new version written beside them as `<file>.new`.
    pub kept_modified: Vec<String>,
    /// Files of the previous release that this one no longer ships, deleted.
    pub removed: Vec<String>,
    /// Shipped files under a link or file the user put in `app/`: not placed, since that
    /// path is theirs.
    pub skipped: Vec<String>,
}

type Manifest = BTreeMap<String, String>;

impl Store {
    /// Opens (creating) a prefix.
    /// The store at `root`. Nothing is created until the first write, which takes
    /// [`Self::lock`]: reading a prefix that does not exist yet is reading an empty one.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store> {
        Ok(Store { root: root.into() })
    }

    /// `~/.local/share/hermir`, `%LOCALAPPDATA%\hermir`, `~/Library/Application Support/hermir`.
    pub fn default_root() -> PathBuf {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        let base = if cfg!(windows) {
            var("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            var("HOME").map(|h| h.join("Library/Application Support"))
        } else {
            var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))
        };
        base.unwrap_or_else(|| PathBuf::from(".")).join("hermir")
    }

    /// The prefix itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a managed emulator's release lives: `<prefix>/<id>/app`.
    pub fn app_dir(&self, id: &str) -> PathBuf {
        self.root.join(id).join("app")
    }

    /// Where downloads land before they are verified.
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join(".tmp")
    }

    /// Fails at once when another process holds the prefix; nothing here waits.
    pub fn lock(&self) -> Result<Lock> {
        fs::create_dir_all(&self.root).map_err(|e| Error::io("create", &self.root, e))?;
        let path = self.root.join(".lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| Error::io("open", &path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Lock { _file: file }),
            Err(fs::TryLockError::WouldBlock) => Err(Error::Locked(self.root.clone())),
            Err(fs::TryLockError::Error(e)) => Err(Error::io("lock", &path, e)),
        }
    }

    fn installed_path(&self) -> PathBuf {
        self.root.join("installed.json")
    }

    /// Every managed install, as `installed.json` records it.
    pub fn installed(&self) -> Result<Vec<Installed>> {
        Ok(read_json(&self.installed_path())?.unwrap_or_default())
    }

    /// One managed install, by emulator id.
    pub fn installed_one(&self, id: &str) -> Result<Option<Installed>> {
        Ok(self.installed()?.into_iter().find(|r| r.emulator == id))
    }

    /// Replaces the row for `row.emulator`.
    pub fn record(&self, row: Installed) -> Result<()> {
        let mut rows = self.installed()?;
        rows.retain(|r| r.emulator != row.emulator);
        rows.push(row);
        rows.sort_by(|a, b| a.emulator.cmp(&b.emulator));
        write_json(&self.installed_path(), &rows)
    }

    /// Drops an emulator's row from `installed.json`.
    pub fn forget(&self, id: &str) -> Result<()> {
        let mut rows = self.installed()?;
        rows.retain(|r| r.emulator != id);
        write_json(&self.installed_path(), &rows)
    }

    fn manifest_path(&self, id: &str) -> PathBuf {
        self.root.join(id).join("manifest.json")
    }

    /// The manifest a `place` writes before it moves anything. Left behind, it means that
    /// place stopped halfway.
    fn next_manifest_path(&self, id: &str) -> PathBuf {
        self.root.join(id).join("manifest.next.json")
    }

    /// The release's manifest, and an interrupted place's when one is left.
    fn manifests(&self, id: &str) -> Result<(Manifest, Manifest)> {
        Ok((
            read_json(&self.manifest_path(id))?.unwrap_or_default(),
            read_json(&self.next_manifest_path(id))?.unwrap_or_default(),
        ))
    }

    /// Places a freshly extracted release tree into `app/`.
    ///
    /// Files the release ships replace their previous version; where the user's own file is
    /// (one they edited, or one at a path the previous release did not ship) it is kept and the
    /// new one written beside it as `.new`; files the previous release shipped and this one
    /// does not are deleted when unmodified; everything else in `app/` — config, saves,
    /// firmware, the user's links — is not touched. `fresh` is consumed.
    ///
    /// The new manifest is written first, as `manifest.next.json`, and renamed into place
    /// last. A place that finds one left over finishes the job: a file matching either
    /// manifest is the release's.
    pub fn place(&self, id: &str, fresh: &Path) -> Result<Placed> {
        let app = self.app_dir(id);
        fs::create_dir_all(&app).map_err(|e| Error::io("create", &app, e))?;
        let (old, stopped) = self.manifests(id)?;
        let shipped = |rel: &str, hash: &str| {
            old.get(rel).is_some_and(|h| h == hash) || stopped.get(rel).is_some_and(|h| h == hash)
        };
        let mut files = Vec::new();
        walk(fresh, fresh, &mut files)?;
        let ships: BTreeSet<&str> = files.iter().map(String::as_str).collect();
        let dropped = |rel: &str| {
            !ships.contains(rel) && (old.contains_key(rel) || stopped.contains_key(rel))
        };

        // Decide everything first; nothing in `app/` changes until the manifest is written.
        let mut placed = Placed::default();
        let mut new = Manifest::new();
        let mut clear = BTreeSet::new();
        let mut moves = Vec::new();
        for rel in &files {
            let hash = leaf_hash(&fresh.join(rel))?;
            let mut cleared = false;
            match blocker(&app, rel)? {
                None => {}
                // The previous release's file or link where this one has a folder.
                Some((at, OnDisk::Leaf(h))) if dropped(&at) && shipped(&at, &h) => {
                    clear.insert(at);
                    cleared = true;
                }
                Some(_) => {
                    placed.skipped.push(rel.clone());
                    continue;
                }
            }
            let to = if cleared {
                rel.clone()
            } else {
                match on_disk(&app.join(rel))? {
                    OnDisk::Absent => rel.clone(),
                    OnDisk::Leaf(h) if h == hash || shipped(rel, &h) => rel.clone(),
                    _ => {
                        placed.kept_modified.push(rel.clone());
                        format!("{rel}.new")
                    }
                }
            };
            new.insert(rel.clone(), hash);
            moves.push((rel, to));
        }
        let next = self.next_manifest_path(id);
        write_json(&next, &new)?;

        for at in &clear {
            remove_leaf(&app.join(at), id)?;
            placed.removed.push(at.clone());
        }
        for (rel, to) in moves {
            let dest = app.join(&to);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
            }
            move_entry(&fresh.join(rel), &dest, id)?;
        }
        let gone: BTreeSet<&String> = old.keys().chain(stopped.keys()).collect();
        for rel in gone {
            if new.contains_key(rel) || blocker(&app, rel)?.is_some() {
                continue;
            }
            if let OnDisk::Leaf(h) = on_disk(&app.join(rel))?
                && shipped(rel, &h)
            {
                remove_leaf(&app.join(rel), id)?;
                placed.removed.push(rel.clone());
            }
        }
        prune_empty_dirs(&app)?;
        let _ = fs::remove_dir_all(fresh);
        let mp = self.manifest_path(id);
        fs::rename(&next, &mp).map_err(|e| Error::io("rename", &mp, e))?;
        Ok(placed)
    }

    /// Takes the release away. Without `purge`, shipped files that are unmodified go and
    /// everything else (config, saves, firmware, edited files) stays, and the manifest keeps
    /// the rows of the edited ones, so a later install still knows them for the user's; with
    /// `purge`, the emulator's whole directory goes.
    pub fn remove_files(&self, id: &str, purge: bool) -> Result<()> {
        let dir = self.root.join(id);
        if purge {
            // `remove_dir_all` removes a link, never what it points at.
            return match fs::remove_dir_all(&dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(Error::io("remove", &dir, e)),
            };
        }
        let app = self.app_dir(id);
        let (old, stopped) = self.manifests(id)?;
        let mut kept = Manifest::new();
        for (rel, hash) in stopped.iter().chain(&old) {
            if kept.contains_key(rel) || blocker(&app, rel)?.is_some() {
                continue;
            }
            match on_disk(&app.join(rel))? {
                OnDisk::Absent => {}
                OnDisk::Leaf(h) if old.get(rel) == Some(&h) || stopped.get(rel) == Some(&h) => {
                    remove_leaf(&app.join(rel), id)?;
                }
                _ => {
                    kept.insert(rel.clone(), hash.clone());
                }
            }
        }
        // The release's version written beside a kept file goes with the release.
        for rel in old.keys().chain(stopped.keys()) {
            if blocker(&app, rel)?.is_some() {
                continue;
            }
            let beside = app.join(format!("{rel}.new"));
            if let OnDisk::Leaf(h) = on_disk(&beside)?
                && (old.get(rel) == Some(&h) || stopped.get(rel) == Some(&h))
            {
                remove_leaf(&beside, id)?;
            }
        }
        let mp = self.manifest_path(id);
        if kept.is_empty() {
            match fs::remove_file(&mp) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Error::io("remove", &mp, e));
                }
                _ => {}
            }
        } else {
            write_json(&mp, &kept)?;
        }
        let _ = fs::remove_file(self.next_manifest_path(id));
        if fs::symlink_metadata(&app).is_ok_and(|m| m.is_dir()) {
            prune_empty_dirs(&app)?;
        }
        let _ = fs::remove_dir(&app);
        let _ = fs::remove_dir(&dir);
        Ok(())
    }
}

/// JSON from `path`; `None` when there is no file. A file that does not parse is an error,
/// never an empty value: an empty manifest would make every file the user's.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| Error::Place {
                what: path.display().to_string(),
                why: format!("unreadable: {e}"),
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io("read", path, e)),
    }
}

/// Writes `value` to `path` whole or not at all: a temporary file, synced, renamed over it.
fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let json = serde_json::to_vec_pretty(value).expect("plain data serializes");
    let mut f = fs::File::create(&tmp).map_err(|e| Error::io("create", &tmp, e))?;
    f.write_all(&json)
        .and_then(|()| f.sync_all())
        .map_err(|e| Error::io("write", &tmp, e))?;
    drop(f);
    fs::rename(&tmp, path).map_err(|e| Error::io("rename", path, e))
}

/// Relative paths of every file and link under `dir`, `/`-separated. A link is a leaf: never
/// followed, whatever it points at.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)
        .map_err(|e| Error::io("read", dir, e))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| Error::io("stat", &path, e))?;
        if kind.is_dir() {
            walk(root, &path, out)?;
        } else if kind.is_file() || kind.is_symlink() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(rel);
        } else {
            return Err(Error::Place {
                what: path.display().to_string(),
                why: "not a file, folder or link".into(),
            });
        }
    }
    Ok(())
}

/// What is at a path, looked at without following a link.
enum OnDisk {
    Absent,
    /// A file or a link, with its manifest hash.
    Leaf(String),
    /// A real folder, or something that is neither file nor link.
    Other,
}

fn on_disk(path: &Path) -> Result<OnDisk> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() || m.file_type().is_symlink() => Ok(OnDisk::Leaf(leaf_hash(path)?)),
        Ok(_) => Ok(OnDisk::Other),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OnDisk::Absent),
        Err(e) => Err(Error::io("stat", path, e)),
    }
}

/// The manifest hash of a file or a link: a link's is the sha256 of `symlink:<target>`.
fn leaf_hash(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let m = fs::symlink_metadata(path).map_err(|e| Error::io("stat", path, e))?;
    if m.file_type().is_symlink() {
        let target = fs::read_link(path).map_err(|e| Error::io("readlink", path, e))?;
        let mut h = Sha256::new();
        h.update(b"symlink:");
        h.update(target.to_string_lossy().as_bytes());
        Ok(hex(&h.finalize()))
    } else {
        sha256_file(path)
    }
}

/// The first ancestor of `rel` in `app` that is not a real folder — a link, or a file — with
/// what is there. `None` when every ancestor is a folder or missing.
fn blocker(app: &Path, rel: &str) -> Result<Option<(String, OnDisk)>> {
    let parts: Vec<&str> = rel.split('/').collect();
    for i in 1..parts.len() {
        let at = parts[..i].join("/");
        let path = app.join(&at);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => continue,
            Ok(_) => return Ok(Some((at, on_disk(&path)?))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io("stat", &path, e)),
        }
    }
    Ok(None)
}

/// A file or link that cannot be moved or deleted because a running program holds it
/// (Windows locks a running exe and its DLLs), or because it is not writable.
fn in_use(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{ExecutableFileBusy, PermissionDenied, ResourceBusy};
    matches!(e.kind(), PermissionDenied | ResourceBusy | ExecutableFileBusy)
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        || (cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)))
}

fn busy(path: &Path, id: &str, e: std::io::Error) -> Error {
    Error::Place {
        what: path.display().to_string(),
        why: format!("{e}: close {id} and retry"),
    }
}

/// Moves a file or link; a link moves as a link.
fn move_entry(src: &Path, dest: &Path, id: &str) -> Result<()> {
    match fs::rename(src, dest) {
        Ok(()) => return Ok(()),
        Err(e) if in_use(&e) => return Err(busy(dest, id, e)),
        Err(_) => {}
    }
    // Another file system: copy, then remove.
    let m = fs::symlink_metadata(src).map_err(|e| Error::io("stat", src, e))?;
    if m.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(|e| Error::io("readlink", src, e))?;
        let _ = fs::remove_file(dest);
        symlink(&target, dest).map_err(|e| Error::io("link", dest, e))?;
    } else {
        fs::copy(src, dest).map_err(|e| {
            if in_use(&e) {
                busy(dest, id, e)
            } else {
                Error::io("copy", dest, e)
            }
        })?;
    }
    fs::remove_file(src).map_err(|e| Error::io("remove", src, e))
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(not(any(unix, windows)))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Deletes a file or a link (on Windows a folder link is removed as a folder).
fn remove_leaf(path: &Path, id: &str) -> Result<()> {
    let e = match fs::remove_file(path) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if cfg!(windows) && fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return fs::remove_dir(path).map_err(|e| Error::io("remove", path, e));
    }
    Err(if in_use(&e) {
        busy(path, id, e)
    } else {
        Error::io("remove", path, e)
    })
}

/// Removes folders left empty under `dir`, not `dir` itself, and never looks inside a link.
fn prune_empty_dirs(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)
        .map_err(|e| Error::io("read", dir, e))?
        .flatten()
    {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let path = entry.path();
            prune_empty_dirs(&path)?;
            let _ = fs::remove_dir(&path);
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hex sha256 of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path).map_err(|e| Error::io("open", path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| Error::io("read", path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// UTC now as RFC 3339, without pulling in a time crate.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    // Civil-from-days (Howard Hinnant), valid for every date this code will see.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Exe, Verified};

    fn row(id: &str) -> Installed {
        Installed {
            emulator: id.into(),
            channel: "github".into(),
            exe: Exe::Path(PathBuf::from("x")),
            version: None,
            release: None,
            sha256: None,
            verified: Verified::None,
            kept: Vec::new(),
            installed_at: now_rfc3339(),
        }
    }

    fn tree(dir: &Path, files: &[(&str, &str)]) {
        for (rel, content) in files {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
    }

    fn read(p: impl AsRef<Path>) -> String {
        fs::read_to_string(p).unwrap()
    }

    #[test]
    fn sha256_of_known_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn records_replace_by_emulator() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        assert!(s.installed().unwrap().is_empty());
        s.record(row("b")).unwrap();
        s.record(row("a")).unwrap();
        s.record(row("b")).unwrap();
        let ids: Vec<_> = s
            .installed()
            .unwrap()
            .into_iter()
            .map(|r| r.emulator)
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
        s.forget("a").unwrap();
        assert_eq!(s.installed().unwrap().len(), 1);
        assert!(!dir.path().join("installed.json.tmp").exists());
    }

    #[test]
    fn a_row_from_before_verification_reads_as_unverified() {
        let r: Installed = serde_json::from_str(
            r#"{"emulator":"x","channel":"github","exe":{"path":"x"},"digest":"ab","installed_at":"t"}"#,
        )
        .unwrap();
        assert_eq!(r.verified, Verified::None);
        assert_eq!(r.sha256, None);
        assert!(r.kept.is_empty());
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let held = s.lock().unwrap();
        assert!(matches!(s.lock(), Err(Error::Locked(_))));
        drop(held);
        assert!(s.lock().is_ok());
    }

    #[test]
    fn update_replaces_shipped_files_and_keeps_data_and_edits() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let fresh = dir.path().join("fresh");
        tree(
            &fresh,
            &[
                ("emu.exe", "v1"),
                ("cfg/default.ini", "d1"),
                ("old.dll", "o"),
            ],
        );
        let p = s.place("emu", &fresh).unwrap();
        assert_eq!(p, Placed::default());
        assert!(!fresh.exists());
        let app = s.app_dir("emu");
        // The emulator runs and creates data; the user edits a shipped file.
        tree(
            &app,
            &[
                ("saves/game.sav", "S"),
                ("portable.txt", ""),
                ("cfg/default.ini", "edited"),
            ],
        );

        let fresh2 = dir.path().join("fresh2");
        tree(
            &fresh2,
            &[
                ("emu.exe", "v2"),
                ("cfg/default.ini", "d2"),
                ("new.dll", "n"),
            ],
        );
        let p = s.place("emu", &fresh2).unwrap();
        assert_eq!(read(app.join("emu.exe")), "v2");
        assert_eq!(read(app.join("saves/game.sav")), "S");
        assert!(app.join("portable.txt").exists());
        assert_eq!(read(app.join("cfg/default.ini")), "edited");
        assert_eq!(read(app.join("cfg/default.ini.new")), "d2");
        assert!(
            !app.join("old.dll").exists(),
            "no longer shipped, unmodified"
        );
        assert!(app.join("new.dll").exists());
        assert_eq!(p.kept_modified, vec!["cfg/default.ini"]);
        assert_eq!(p.removed, vec!["old.dll"]);
        assert!(!dir.path().join("emu/manifest.next.json").exists());

        s.remove_files("emu", false).unwrap();
        assert!(!app.join("emu.exe").exists());
        assert!(
            app.join("saves/game.sav").exists(),
            "data survives a remove"
        );
        assert!(
            app.join("cfg/default.ini").exists(),
            "edits survive a remove"
        );
        assert!(
            !app.join("cfg/default.ini.new").exists(),
            "the release's own copy goes with it"
        );
        s.remove_files("emu", true).unwrap();
        assert!(!dir.path().join("emu").exists());
    }

    #[test]
    fn an_edit_survives_remove_and_reinstall() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let release = [("emu.exe", "v1"), ("cfg/default.ini", "d1")];
        let fresh = dir.path().join("fresh");
        tree(&fresh, &release);
        s.place("emu", &fresh).unwrap();
        let app = s.app_dir("emu");
        fs::write(app.join("cfg/default.ini"), "edited").unwrap();

        s.remove_files("emu", false).unwrap();
        assert!(!app.join("emu.exe").exists());
        let (kept, _) = s.manifests("emu").unwrap();
        assert_eq!(
            kept.keys().collect::<Vec<_>>(),
            vec!["cfg/default.ini"],
            "the edited file's row stays"
        );

        tree(&fresh, &release);
        let p = s.place("emu", &fresh).unwrap();
        assert_eq!(read(app.join("cfg/default.ini")), "edited");
        assert_eq!(read(app.join("cfg/default.ini.new")), "d1");
        assert_eq!(read(app.join("emu.exe")), "v1");
        assert_eq!(p.kept_modified, vec!["cfg/default.ini"]);

        // Restored by hand to what shipped: the manifest empties and goes, `.new` with it.
        fs::write(app.join("cfg/default.ini"), "d1").unwrap();
        s.remove_files("emu", false).unwrap();
        assert!(!dir.path().join("emu").exists());
    }

    #[test]
    fn a_file_the_user_made_at_a_shipped_path_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let app = s.app_dir("emu");
        // No manifest: nothing in `app/` is the release's yet.
        tree(&app, &[("keys.txt", "mine"), ("same.txt", "s")]);
        let fresh = dir.path().join("fresh");
        tree(
            &fresh,
            &[("emu.exe", "v1"), ("keys.txt", "theirs"), ("same.txt", "s")],
        );
        let p = s.place("emu", &fresh).unwrap();
        assert_eq!(read(app.join("keys.txt")), "mine");
        assert_eq!(read(app.join("keys.txt.new")), "theirs");
        assert!(
            !app.join("same.txt.new").exists(),
            "an identical file is simply the release's"
        );
        assert_eq!(p.kept_modified, vec!["keys.txt"]);
        s.remove_files("emu", false).unwrap();
        assert_eq!(read(app.join("keys.txt")), "mine");
        assert!(!app.join("same.txt").exists());
    }

    #[test]
    fn an_interrupted_place_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let app = s.app_dir("emu");
        let fresh = dir.path().join("fresh");
        tree(
            &fresh,
            &[("emu.exe", "A"), ("gone.dll", "g"), ("same.txt", "s")],
        );
        s.place("emu", &fresh).unwrap();
        fs::write(app.join("cfg.ini"), "user").unwrap();

        // Release B: its manifest was written and `emu.exe` moved, then the place stopped.
        let b = [("emu.exe", "B"), ("new.dll", "n"), ("same.txt", "s")];
        let staged = dir.path().join("staged");
        tree(&staged, &b);
        let mut next = Manifest::new();
        for (rel, _) in b {
            next.insert(rel.into(), sha256_file(&staged.join(rel)).unwrap());
        }
        write_json(&s.next_manifest_path("emu"), &next).unwrap();
        fs::write(app.join("emu.exe"), "B").unwrap();

        let p = s.place("emu", &staged).unwrap();
        assert_eq!(read(app.join("emu.exe")), "B");
        assert!(
            !app.join("emu.exe.new").exists(),
            "B's file is the release's"
        );
        assert_eq!(read(app.join("new.dll")), "n");
        assert!(!app.join("gone.dll").exists());
        assert_eq!(read(app.join("cfg.ini")), "user");
        assert!(p.kept_modified.is_empty());
        assert_eq!(p.removed, vec!["gone.dll"]);
        assert!(!s.next_manifest_path("emu").exists());
        assert_eq!(s.manifests("emu").unwrap().0, next);
    }

    #[test]
    fn a_folder_where_a_file_was_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let app = s.app_dir("emu");
        let fresh = dir.path().join("fresh");
        tree(&fresh, &[("emu.exe", "1"), ("lib", "a file")]);
        s.place("emu", &fresh).unwrap();
        tree(&fresh, &[("emu.exe", "2"), ("lib/x.so", "x")]);
        let p = s.place("emu", &fresh).unwrap();
        assert_eq!(read(app.join("lib/x.so")), "x");
        assert_eq!(p.removed, vec!["lib"]);
    }

    #[test]
    fn corrupt_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        fs::write(dir.path().join("installed.json"), "{ not json").unwrap();
        assert!(matches!(s.installed(), Err(Error::Place { .. })));
        assert!(s.record(row("a")).is_err());

        fs::create_dir_all(dir.path().join("emu")).unwrap();
        fs::write(s.manifest_path("emu"), "[1, 2").unwrap();
        let fresh = dir.path().join("fresh");
        tree(&fresh, &[("emu.exe", "1")]);
        assert!(matches!(s.place("emu", &fresh), Err(Error::Place { .. })));
        assert!(s.remove_files("emu", false).is_err());

        fs::remove_file(s.manifest_path("emu")).unwrap();
        fs::write(s.next_manifest_path("emu"), "").unwrap();
        assert!(s.place("emu", &fresh).is_err());
    }

    #[test]
    fn a_held_file_says_what_to_close() {
        let e = busy(
            Path::new("app/emu.exe"),
            "cemu",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(e.to_string().contains("close cemu and retry"), "{e}");
        assert_eq!(e.exit_code(), 5);
        assert!(in_use(&std::io::Error::from(
            std::io::ErrorKind::ResourceBusy
        )));
        assert!(!in_use(&std::io::Error::from(std::io::ErrorKind::NotFound)));
    }

    #[cfg(unix)]
    #[test]
    fn a_release_link_is_a_leaf() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let app = s.app_dir("emu");
        let fresh = dir.path().join("fresh");
        tree(&fresh, &[("usr/bin/emu", "elf")]);
        symlink("usr/bin/emu", fresh.join("AppRun")).unwrap();
        symlink("bin", fresh.join("usr/sbin")).unwrap();
        s.place("emu", &fresh).unwrap();
        assert_eq!(
            fs::read_link(app.join("AppRun")).unwrap(),
            Path::new("usr/bin/emu")
        );
        assert!(
            fs::symlink_metadata(app.join("usr/sbin"))
                .unwrap()
                .is_symlink()
        );
        let (m, _) = s.manifests("emu").unwrap();
        assert_eq!(
            m.keys().collect::<Vec<_>>(),
            vec!["AppRun", "usr/bin/emu", "usr/sbin"]
        );

        // A link the user pointed elsewhere is theirs, like an edited file.
        fs::remove_file(app.join("AppRun")).unwrap();
        symlink("usr/bin/other", app.join("AppRun")).unwrap();
        s.remove_files("emu", false).unwrap();
        assert!(!app.join("usr").exists());
        assert_eq!(
            fs::read_link(app.join("AppRun")).unwrap(),
            Path::new("usr/bin/other")
        );
    }

    /// A ROM folder linked into `app/` (here over a folder the release ships) is never
    /// entered: not by an update, not by a remove, not by a purge.
    #[cfg(unix)]
    #[test]
    fn a_linked_folder_survives_update_and_remove() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        tree(
            outside.path(),
            &[("readme.txt", "R"), ("game.iso", "G"), ("data/x", "X")],
        );
        let snapshot = |p: &Path| {
            let mut files = Vec::new();
            walk(p, p, &mut files).unwrap();
            files
                .into_iter()
                .map(|f| (read(p.join(&f)), f))
                .collect::<Vec<_>>()
        };
        let before = snapshot(outside.path());

        let s = Store::open(dir.path()).unwrap();
        let app = s.app_dir("emu");
        let fresh = dir.path().join("fresh");
        tree(
            &fresh,
            &[("emu.exe", "1"), ("roms/readme.txt", "R"), ("data/x", "X")],
        );
        s.place("emu", &fresh).unwrap();
        fs::remove_dir_all(app.join("roms")).unwrap();
        symlink(outside.path(), app.join("roms")).unwrap();
        symlink(outside.path(), app.join("shared")).unwrap();

        tree(
            &fresh,
            &[("emu.exe", "2"), ("roms/readme.txt", "R2"), ("data/x", "X")],
        );
        let p = s.place("emu", &fresh).unwrap();
        assert_eq!(p.skipped, vec!["roms/readme.txt"]);
        assert_eq!(read(app.join("emu.exe")), "2");
        assert_eq!(snapshot(outside.path()), before);

        s.remove_files("emu", false).unwrap();
        assert!(!app.join("emu.exe").exists());
        assert!(fs::symlink_metadata(app.join("roms")).unwrap().is_symlink());
        assert!(
            fs::symlink_metadata(app.join("shared"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(snapshot(outside.path()), before);

        s.remove_files("emu", true).unwrap();
        assert!(!dir.path().join("emu").exists());
        assert_eq!(snapshot(outside.path()), before);
    }

    #[test]
    fn rfc3339_shape() {
        let t = now_rfc3339();
        assert_eq!(t.len(), 20);
        assert!(t.starts_with("20") && t.ends_with('Z'));
    }
}
