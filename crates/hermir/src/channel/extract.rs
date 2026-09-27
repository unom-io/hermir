//! Archive extraction into a fresh directory. Zip and 7z, no shelling out. When an archive
//! wraps everything in one top-level directory, that directory is stripped, so `exe` in the
//! catalog is always relative to the emulator's own root.
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| Error::io("create", dest, e))?;
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        unzip(archive, dest)?;
    } else if name.ends_with(".7z") {
        sevenz_rust2::decompress_file(archive, dest).map_err(|e| Error::Place {
            what: archive.display().to_string(),
            why: format!("7z: {e}"),
        })?;
    } else {
        return Err(Error::Place {
            what: archive.display().to_string(),
            why: "not a .zip or .7z".into(),
        });
    }
    strip_single_top_dir(dest)
}

fn unzip(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive).map_err(|e| Error::io("open", archive, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Place {
        what: archive.display().to_string(),
        why: format!("zip: {e}"),
    })?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| Error::Place {
            what: archive.display().to_string(),
            why: format!("zip entry {i}: {e}"),
        })?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(Error::Place {
                what: archive.display().to_string(),
                why: format!("entry {} escapes the archive", entry.name()),
            });
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| Error::io("create", &out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
        }
        let mut f = fs::File::create(&out).map_err(|e| Error::io("create", &out, e))?;
        io::copy(&mut entry, &mut f).map_err(|e| Error::io("write", &out, e))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// `dest/Only-Dir/…` becomes `dest/…`.
fn strip_single_top_dir(dest: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = fs::read_dir(dest)
        .map_err(|e| Error::io("read", dest, e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    let [only] = entries.as_slice() else {
        return Ok(());
    };
    if !only.is_dir() {
        return Ok(());
    }
    let staging = dest.with_file_name(format!(
        "{}.strip",
        dest.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    let _ = fs::remove_dir_all(&staging);
    fs::rename(only, &staging).map_err(|e| Error::io("rename", only, e))?;
    fs::remove_dir(dest).map_err(|e| Error::io("remove", dest, e))?;
    fs::rename(&staging, dest).map_err(|e| Error::io("rename", &staging, e))
}

/// True when `rel` stays inside its root: no `..`, no absolute prefix.
pub fn is_safe_relative(rel: &Path) -> bool {
    rel.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, bytes) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn strips_a_single_top_dir_and_keeps_flat_archives() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.zip");
        fs::write(&a, zip_with(&[("Top/x.exe", b"1"), ("Top/sub/y", b"2")])).unwrap();
        let out = dir.path().join("out");
        extract(&a, &out).unwrap();
        assert!(out.join("x.exe").exists() && out.join("sub/y").exists());

        let b = dir.path().join("b.zip");
        fs::write(&b, zip_with(&[("x.exe", b"1"), ("sub/y", b"2")])).unwrap();
        let out2 = dir.path().join("out2");
        extract(&b, &out2).unwrap();
        assert!(out2.join("x.exe").exists());
    }

    #[test]
    fn refuses_unknown_and_escaping() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("a.tar");
        fs::write(&t, b"").unwrap();
        assert!(matches!(
            extract(&t, &dir.path().join("o")),
            Err(Error::Place { .. })
        ));
        assert!(!is_safe_relative(Path::new("../x")));
        assert!(is_safe_relative(Path::new("Dolphin-x64/Dolphin.exe")));
    }
}
