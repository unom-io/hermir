//! The prefix on disk: `<root>/<id>/app/` is an emulator's home, `<root>/<id>/manifest.json`
//! lists what the release shipped there (path → sha256), `installed.json` says what is
//! installed and from where, `.tmp/` holds work in flight, `.lock` serialises writers.
//!
//! A portable emulator keeps its config and saves beside its exe, inside `app/`. The manifest
//! is what lets an update replace the release's files and nothing else, and lets a remove
//! take the release away and leave the data.
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::Installed;

#[derive(Clone, Debug)]
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
    /// Shipped files the user had modified: kept as they were, the new version written beside
    /// them as `<file>.new`.
    pub kept_modified: Vec<String>,
    /// Files of the previous release that this one no longer ships, deleted.
    pub removed: Vec<String>,
}

type Manifest = BTreeMap<String, String>;

impl Store {
    /// Opens (creating) a prefix.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| Error::io("create", &root, e))?;
        Ok(Store { root })
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

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn app_dir(&self, id: &str) -> PathBuf {
        self.root.join(id).join("app")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join(".tmp")
    }

    /// Fails at once when another process holds the prefix; nothing here waits.
    pub fn lock(&self) -> Result<Lock> {
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

    pub fn installed(&self) -> Result<Vec<Installed>> {
        let path = self.installed_path();
        match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| Error::Place {
                what: path.display().to_string(),
                why: format!("unreadable: {e}"),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(Error::io("read", &path, e)),
        }
    }

    pub fn installed_one(&self, id: &str) -> Result<Option<Installed>> {
        Ok(self.installed()?.into_iter().find(|r| r.emulator == id))
    }

    /// Replaces the row for `row.emulator`.
    pub fn record(&self, row: Installed) -> Result<()> {
        let mut rows = self.installed()?;
        rows.retain(|r| r.emulator != row.emulator);
        rows.push(row);
        rows.sort_by(|a, b| a.emulator.cmp(&b.emulator));
        self.write_installed(&rows)
    }

    pub fn forget(&self, id: &str) -> Result<()> {
        let mut rows = self.installed()?;
        rows.retain(|r| r.emulator != id);
        self.write_installed(&rows)
    }

    fn write_installed(&self, rows: &[Installed]) -> Result<()> {
        let path = self.installed_path();
        let tmp = self.root.join("installed.json.tmp");
        let json = serde_json::to_string_pretty(rows).expect("rows serialize");
        fs::write(&tmp, json).map_err(|e| Error::io("write", &tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| Error::io("rename", &path, e))
    }

    fn manifest_path(&self, id: &str) -> PathBuf {
        self.root.join(id).join("manifest.json")
    }

    fn manifest(&self, id: &str) -> Result<Manifest> {
        let path = self.manifest_path(id);
        match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| Error::Place {
                what: path.display().to_string(),
                why: format!("unreadable: {e}"),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::new()),
            Err(e) => Err(Error::io("read", &path, e)),
        }
    }

    /// Places a freshly extracted release tree into `app/`.
    ///
    /// Files the release ships replace their previous version; a shipped file the user had
    /// modified since is kept and the new one written beside it as `.new`; files the previous
    /// release shipped and this one does not are deleted when unmodified; everything else in
    /// `app/` — config, saves, firmware — is not touched. `fresh` is consumed.
    pub fn place(&self, id: &str, fresh: &Path) -> Result<Placed> {
        let app = self.app_dir(id);
        fs::create_dir_all(&app).map_err(|e| Error::io("create", &app, e))?;
        let old = self.manifest(id)?;
        let mut new = Manifest::new();
        let mut files = Vec::new();
        walk(fresh, fresh, &mut files)?;
        for rel in &files {
            new.insert(rel.clone(), sha256_of(&fresh.join(rel))?);
        }
        let mut placed = Placed::default();

        for (rel, shipped_hash) in &old {
            if new.contains_key(rel) {
                continue;
            }
            let path = app.join(rel);
            if path.is_file() && sha256_of(&path)? == *shipped_hash {
                fs::remove_file(&path).map_err(|e| Error::io("remove", &path, e))?;
                placed.removed.push(rel.clone());
            }
        }
        for rel in &files {
            let src = fresh.join(rel);
            let dest = app.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
            }
            let modified = match old.get(rel) {
                Some(h) if dest.is_file() => sha256_of(&dest)? != *h,
                _ => false,
            };
            if modified {
                let beside = app.join(format!("{rel}.new"));
                move_file(&src, &beside)?;
                placed.kept_modified.push(rel.clone());
            } else {
                move_file(&src, &dest)?;
            }
        }
        prune_empty_dirs(&app)?;
        let _ = fs::remove_dir_all(fresh);
        let mp = self.manifest_path(id);
        let json = serde_json::to_string_pretty(&new).expect("manifest serializes");
        fs::write(&mp, json).map_err(|e| Error::io("write", &mp, e))?;
        Ok(placed)
    }

    /// Takes the release away. Without `purge`, shipped files that are unmodified go and
    /// everything else (config, saves, firmware, edited files) stays; with `purge`, the
    /// emulator's whole directory goes.
    pub fn remove_files(&self, id: &str, purge: bool) -> Result<()> {
        let dir = self.root.join(id);
        if purge {
            return match fs::remove_dir_all(&dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(Error::io("remove", &dir, e)),
            };
        }
        let app = self.app_dir(id);
        for (rel, hash) in self.manifest(id)? {
            let path = app.join(&rel);
            if path.is_file() && sha256_of(&path)? == hash {
                fs::remove_file(&path).map_err(|e| Error::io("remove", &path, e))?;
            }
        }
        let _ = fs::remove_file(self.manifest_path(id));
        if app.is_dir() {
            prune_empty_dirs(&app)?;
        }
        let _ = fs::remove_dir(&app);
        let _ = fs::remove_dir(&dir);
        Ok(())
    }
}

/// Relative paths of every file under `dir`, `/`-separated.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| Error::io("read", dir, e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(rel);
        }
    }
    Ok(())
}

fn move_file(src: &Path, dest: &Path) -> Result<()> {
    if fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    fs::copy(src, dest).map_err(|e| Error::io("copy", dest, e))?;
    fs::remove_file(src).map_err(|e| Error::io("remove", src, e))
}

/// Removes directories left empty under `dir`, not `dir` itself.
fn prune_empty_dirs(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)
        .map_err(|e| Error::io("read", dir, e))?
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            prune_empty_dirs(&path)?;
            let _ = fs::remove_dir(&path);
        }
    }
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String> {
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
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
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
    use crate::model::Exe;

    fn row(id: &str) -> Installed {
        Installed {
            emulator: id.into(),
            channel: "github".into(),
            exe: Exe::Path(PathBuf::from("x")),
            version: None,
            release: None,
            digest: None,
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
        assert_eq!(fs::read_to_string(app.join("emu.exe")).unwrap(), "v2");
        assert_eq!(fs::read_to_string(app.join("saves/game.sav")).unwrap(), "S");
        assert!(app.join("portable.txt").exists());
        assert_eq!(
            fs::read_to_string(app.join("cfg/default.ini")).unwrap(),
            "edited"
        );
        assert_eq!(
            fs::read_to_string(app.join("cfg/default.ini.new")).unwrap(),
            "d2"
        );
        assert!(
            !app.join("old.dll").exists(),
            "no longer shipped, unmodified"
        );
        assert!(app.join("new.dll").exists());
        assert_eq!(p.kept_modified, vec!["cfg/default.ini"]);
        assert_eq!(p.removed, vec!["old.dll"]);

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
        s.remove_files("emu", true).unwrap();
        assert!(!dir.path().join("emu").exists());
    }

    #[test]
    fn rfc3339_shape() {
        let t = now_rfc3339();
        assert_eq!(t.len(), 20);
        assert!(t.starts_with("20") && t.ends_with('Z'));
    }
}
